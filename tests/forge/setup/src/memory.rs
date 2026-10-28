//! Host-only metering; peers, item store and world records stay outside.
use super::{
    Answer, Down, Env, Event, Fake, Heard, Host, HostWorld, Item, Limits, Outcome, Plan, Queue, Request, Settings,
    Time, Up, fixtures, github,
};
use skein_heap::{Measured, Span};
use skein_tls_world::{meter::Steps, pki};

pub(crate) struct Metered {
    host: Host,
    measurement: Option<Measurement>,
    above: Queue<Event>,
    below: Queue<skein_io::Request>,
}
struct Measurement {
    steps: Steps,
    outside: Option<Span>,
}
impl Metered {
    pub fn new(host: Host) -> Self {
        Self { host, measurement: None, above: Queue::with_capacity(16), below: Queue::with_capacity(16) }
    }
    fn start(&mut self, input: usize) {
        if let Some(m) = &mut self.measurement {
            m.steps.outside(m.outside.take().expect("between steps").end().net);
            m.steps.outside(-i64::try_from(input).expect("input"));
            m.steps.start();
        }
    }
    fn finish(&mut self, above: &mut Queue<Event>, below: &mut Queue<skein_io::Request>) {
        let Some(m) = &mut self.measurement else {
            while let Some(event) = self.above.pop() {
                above.push(event);
            }
            while let Some(request) = self.below.pop() {
                below.push(request);
            }
            return;
        };
        let measured: Measured = m.steps.end();
        let mut outside = 0;
        // Copy handoffs for the world, drop originals before checking the step.
        while let Some(event) = self.above.pop() {
            let span = Span::start();
            above.push(event.clone());
            outside += span.end().net;
            drop(event);
        }
        while let Some(request) = self.below.pop() {
            let span = Span::start();
            below.push(copy_io(&request));
            outside += span.end().net;
            drop(request);
        }
        m.steps.check(measured, 0, &"host step");
        m.steps.outside(outside);
        m.outside = Some(Span::start());
    }
    pub fn down(
        &mut self,
        env: &Env<Limits>,
        request: Request,
        above: &mut Queue<Event>,
        below: &mut Queue<skein_io::Request>,
    ) {
        self.start(request_bytes(&request));
        self.host.down(env, request, &mut self.above, &mut self.below);
        self.finish(above, below);
    }
    pub fn up(
        &mut self,
        env: &Env<Limits>,
        event: skein_io::Event,
        above: &mut Queue<Event>,
        below: &mut Queue<skein_io::Request>,
    ) {
        let input = match &event {
            skein_io::Event::Stream { up: Up::Bytes(bytes), .. } => bytes.len(),
            skein_io::Event::Stream { up: Up::End | Up::Room | Up::Failed(_), .. }
            | skein_io::Event::Listening { .. }
            | skein_io::Event::Accepted { .. }
            | skein_io::Event::Connecting { .. }
            | skein_io::Event::Connected { .. }
            | skein_io::Event::Output { .. }
            | skein_io::Event::Spawned { .. }
            | skein_io::Event::Exited { .. }
            | skein_io::Event::Usage { .. }
            | skein_io::Event::Resized
            | skein_io::Event::Window { .. }
            | skein_io::Event::Shutdown { .. }
            | skein_io::Event::Failed { .. }
            | skein_io::Event::Closed { .. } => 0,
        };
        self.start(input);
        self.host.up(env, event, &mut self.above, &mut self.below);
        self.finish(above, below);
    }
    pub fn fire(&mut self, env: &Env<Limits>, above: &mut Queue<Event>, below: &mut Queue<skein_io::Request>) {
        self.start(0);
        self.host.fire(env, &mut self.above, &mut self.below);
        self.finish(above, below);
    }
    pub fn next_deadline(&self) -> Option<Time> {
        self.host.next_deadline()
    }
}
fn copy_io(request: &skein_io::Request) -> skein_io::Request {
    match request {
        skein_io::Request::Connect { owner, addr } => skein_io::Request::Connect { owner: *owner, addr: *addr },
        skein_io::Request::Stream { stream, down } => skein_io::Request::Stream {
            stream: *stream,
            down: match down {
                Down::Demand { read, room } => Down::Demand { read: *read, room: *room },
                Down::Send(bytes) => Down::Send(bytes.clone()),
                Down::Finish => Down::Finish,
            },
        },
        skein_io::Request::Close { entity } => skein_io::Request::Close { entity: *entity },
        skein_io::Request::Abort { entity } => skein_io::Request::Abort { entity: *entity },
        skein_io::Request::Listen { .. }
        | skein_io::Request::Bind { .. }
        | skein_io::Request::Reject { .. }
        | skein_io::Request::Output { .. }
        | skein_io::Request::Spawn { .. }
        | skein_io::Request::Signal { .. }
        | skein_io::Request::Usage { .. }
        | skein_io::Request::Window { .. } => panic!("host socket request"),
    }
}
fn request_bytes(request: &Request) -> usize {
    match request {
        Request::Answer { answer: Answer::Read(Some(bytes)), .. } => bytes.len(),
        Request::Run { plan, .. } => match plan {
            Plan::Reconcile { adopted, .. } => adopted.len() * size_of::<u64>(),
            Plan::TakeOver { admin_token, .. } => admin_token.len(),
            Plan::Create { org, repository, files, .. } => {
                org.len()
                    + repository.len()
                    + files.len() * size_of::<skein_forge_setup::host::File>()
                    + files.iter().map(|f| f.path.len() + f.content.len()).sum::<usize>()
            }
            Plan::Convert { code, state, .. } => code.len() + state.len(),
            Plan::Accept { state, .. } => state.as_ref().map_or(0, |state| state.len()),
            Plan::Manifest { .. }
            | Plan::Paste { .. }
            | Plan::Install { .. }
            | Plan::Check { .. }
            | Plan::Adopt { .. }
            | Plan::Reachable { .. } => 0,
        },
        Request::Answer { answer: Answer::Read(None) | Answer::Written, .. } | Request::Close => 0,
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Kinds {
    Forgejo,
    GitHub,
    Both,
}
/// Peak of the host's own heap, with simultaneous full run/connection slots.
#[derive(Debug)]
pub struct Full {
    pub owned: u64,
    pub bound: u64,
    pub runs: u32,
    pub connections: u32,
    pub states: u32,
}
fn settings(kinds: Kinds, tls: bool) -> Settings {
    let mut settings = fixtures::host_settings();
    settings.forges.clear();
    if matches!(kinds, Kinds::Forgejo | Kinds::Both) {
        let kind = fixtures::host_settings().forges.get(0).expect("Forgejo").kind.clone();
        settings.forges.push(forge(kind)).expect("forge");
    }
    if matches!(kinds, Kinds::GitHub | Kinds::Both) {
        settings.forges.push(fixtures::github_forge()).expect("forge");
    }
    if tls {
        let trust = pki::client(&[]);
        for at in 0..settings.forges.len() {
            let forge = settings.forges.get_mut(at).expect("forge");
            forge.transport =
                skein_forge_setup::host::Transport::Tls { server_name: pki::name(), trust: trust.clone() };
        }
    }
    settings
}

fn forge(kind: skein_forge_setup::host::Kind) -> skein_forge_setup::host::Forge {
    let address = match &kind {
        skein_forge_setup::host::Kind::Forgejo(_) => fixtures::host_settings().forges.get(0).expect("Forgejo").address,
        skein_forge_setup::host::Kind::GitHub(_) => fixtures::github_forge().address,
    };
    skein_forge_setup::host::Forge {
        kind,
        address,
        transport: skein_forge_setup::host::Transport::Plaintext { name: None },
    }
}

/// Runs the configured host with every run and connection slot occupied at
/// once, then completes through its in-memory peers and releases every state.
#[must_use]
pub fn full_heap(kinds: Kinds, tls: bool) -> Full {
    let l = fixtures::host_limits();
    let configured = settings(kinds, tls);
    let bound = skein_forge_setup::host::worst_case(&l, &configured).expect("bound");
    let numbers: Vec<_> = configured.forges.as_slice().iter().map(skein_forge_setup::host::Forge::number).collect();
    // Trust is shared with an owner outside the measured host.
    let trust = configured.forges.as_slice().iter().find_map(|f| match &f.transport {
        skein_forge_setup::host::Transport::Tls { trust, .. } => Some(trust.clone()),
        skein_forge_setup::host::Transport::Plaintext { .. } => None,
    });
    let mut w = HostWorld::new_with(7, &[], configured, l);
    if matches!(kinds, Kinds::GitHub | Kinds::Both) {
        w.github = Some(Fake::bare(7));
        w.put(Item::AppId, fixtures::app_id(7));
        w.put(Item::AppKey, fixtures::secret(github::app_key()));
        w.put(Item::AppSlug, fixtures::secret(b"test-app"));
        w.put(Item::Accepted, fixtures::accepted(&[1, 2]));
    }
    w.wall_base = Some(pki::VALID);
    w.set_time(Time::ZERO);
    let mut steps = Steps::new(bound);
    steps.start();
    let mut configured = settings(kinds, false);
    if let Some(trust) = &trust {
        for at in 0..configured.forges.len() {
            let f = configured.forges.get_mut(at).expect("forge");
            f.transport = skein_forge_setup::host::Transport::Tls { server_name: pki::name(), trust: trust.clone() };
        }
    }
    let host = Host::new(&l, configured, fixtures::crypto()).expect("admitted host");
    let measured = steps.end();
    steps.check(measured, 0, &"construction");
    let outside = Span::start();
    w.host.host = host;
    w.host.measurement = Some(Measurement { steps, outside: Some(outside) });

    let mut states = Vec::new();
    if matches!(kinds, Kinds::GitHub | Kinds::Both) {
        for n in 0..l.states {
            w.run_plan(
                100 + u64::from(n),
                Plan::Manifest { forge: 4, target: skein_forge_setup::github::manifest::Target::Personal },
            );
            w.settle();
            let Some(Heard::Done { outcome: Outcome::Manifest { state, .. }, .. }) = w.heard.last() else {
                panic!("manifest issued its state: {:?}", w.heard);
            };
            assert!(!states.contains(state));
            states.push(state.clone());
        }
        assert_eq!(states.len(), usize::try_from(l.states).expect("states"));
    }
    let ended = w.heard.len();
    for (at, number) in numbers.iter().copied().enumerate() {
        w.run_plan(1 + u64::try_from(at).expect("run"), Plan::Reconcile { forge: number, adopted: Box::new([]) });
    }
    if numbers.len() == 1 {
        w.run_plan(
            2,
            Plan::Reachable {
                forge: numbers[0],
                installation: matches!(kinds, Kinds::GitHub).then_some(1),
                page: core::num::NonZeroU32::new(1).expect("page"),
            },
        );
    }
    // Both plans are waiting on their owner, then open both connections in
    // the same iteration before either peer can return an HTTP answer.
    w.tick();
    assert_eq!(w.heard.len(), ended, "all runs live at the same time");
    assert_eq!(w.peers.len(), usize::try_from(l.runs).expect("runs"));
    assert_eq!(w.connects.len(), usize::try_from(l.runs).expect("runs"));
    w.finish();
    assert_eq!(w.heard.len(), ended + usize::try_from(l.runs).expect("runs"));
    assert!(w.heard[ended..].iter().all(|event| matches!(event, Heard::Done { .. })), "{:?}", w.heard);
    // Each held state remains authoritative after the full live-run phase.
    // An invalid conversion code fails after taking the state, Unexpected,
    // rather than Unknown. No connection or owner item is needed for it.
    for (at, state) in states.iter().enumerate() {
        w.run_plan(
            200 + u64::try_from(at).expect("run"),
            Plan::Convert { forge: 4, code: fixtures::secret(b""), state: state.clone() },
        );
        w.settle();
        assert!(
            matches!(w.heard.last(), Some(Heard::Stop { failure, .. }) if failure.why == skein_forge_setup::reconcile::Why::Unexpected),
            "{:?}",
            w.heard.last()
        );
    }
    w.close();
    w.finish();
    assert!(matches!(w.heard.last(), Some(Heard::Closed { .. })));
    let mut m = w.host.measurement.take().expect("meter");
    m.steps.outside(m.outside.take().expect("outside").end().net);
    // Empty output buffers grew inside host steps, so release them with the host.
    let Metered { host, above, below, measurement: _ } = w.host;
    assert!(above.is_empty() && below.is_empty());
    let owned = m.steps.finish((host, above, below), 0, &"host releases its own heap");
    Full { owned, bound, runs: l.runs, connections: l.runs, states: u32::try_from(states.len()).expect("states") }
}
