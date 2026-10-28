use super::fixtures;
use crate::{
    call::{Auth, Call},
    host::{Answer, Event, Forge, Host, Kind, Limits, Need, Plan, Request, Settings, Stacked, Transport, Unusable},
    item::Item,
    reconcile::{Failure, Outcome, Stage, Why},
};
use skein_lib::{CryptoRng, Duration, Env, List, Queue, Time, Token, Wall};

#[test]
fn a_github_call_on_a_forgejo_forge_is_refused_unexpected() {
    for auth in [
        Auth::Token(fixtures::secret(b"forgejo-token")),
        Auth::App { id: 7, key: fixtures::secret(b"app-key") },
        Auth::Bearer(fixtures::secret(b"installation-token")),
        Auth::None,
    ] {
        assert_eq!(
            crate::host::run::forgejo_pair(Call::GitHub(skein_forge::github::request::Request::App), auth),
            Err(Why::Unexpected)
        );
    }
}

#[test]
fn a_forgejo_call_on_an_app_or_bearer_or_no_auth_is_refused_unexpected() {
    for auth in [
        Auth::App { id: 7, key: fixtures::secret(b"app-key") },
        Auth::Bearer(fixtures::secret(b"installation-token")),
        Auth::None,
    ] {
        assert_eq!(
            crate::host::run::forgejo_pair(Call::Forgejo(skein_forge::forgejo::request::Request::User), auth),
            Err(Why::Unexpected)
        );
    }
}

#[test]
fn a_forgejo_call_on_a_token_passes_through_forgejo_pair() {
    let request = skein_forge::forgejo::request::Request::User;
    let token = fixtures::secret(b"forgejo-token");
    assert_eq!(
        crate::host::run::forgejo_pair(request.clone().into(), Auth::Token(token.clone())),
        Ok((request, token))
    );
}
pub(crate) fn limits() -> Limits {
    Limits {
        forges: 2,
        runs: 2,
        states: 8,
        token_bytes: 64,
        head_bytes: 4096,
        headers: 32,
        body_bytes_in: 65536,
        depth: 16,
        chunk: 1024,
        string_bytes: 512,
        body_bytes: 16384,
        inner: 32,
        target_bytes: 4096,
        tls: skein_tls::client::Limits { read: 16384, send: 16384, records: skein_tls::client::MAX_RECORD },
        io: skein_io::Limits {
            sockets: 8,
            names: 0,
            refusals: 2,
            intake: 32768,
            receive: 4096,
            output: 32768,
            sends: 4,
            accepts: 1,
            backlog: 1,
            close_timeout: Duration::from_secs(1),
            retry: Duration::from_millis(10),
        },
    }
}
fn forge(number: u16) -> Forge {
    let mut given = fixtures::given();
    given.forge = number;
    Forge {
        kind: Kind::Forgejo(given),
        address: skein_io::Addr::Ip(([127, 0, 0, 1], 3000).into()),
        transport: Transport::Plaintext { name: None },
    }
}
fn forgejo_mut(settings: &mut Settings) -> &mut crate::given::Given {
    match &mut settings.forges.get_mut(0).expect("one forge").kind {
        Kind::Forgejo(given) => given,
        Kind::GitHub(_) => panic!("Forgejo fixture"),
    }
}
pub(crate) fn settings(numbers: &[u16]) -> Settings {
    let mut forges = List::with_capacity(u32::try_from(numbers.len()).expect("small table"));
    for number in numbers {
        forges.push(forge(*number)).expect("forge count");
    }
    Settings { forges, user_agent: b"kit/0.0.0".as_slice().into() }
}
pub(crate) fn crypto() -> CryptoRng {
    CryptoRng::new(&fixtures::secret(&[42; 32]), 0).expect("32-byte seed")
}
struct World {
    host: Host,
    env: Env<Limits>,
    up: Queue<Event>,
    io: Queue<skein_io::Request>,
}
impl World {
    fn new() -> Self {
        Self::with(limits(), settings(&[1]))
    }
    fn with(limits: Limits, settings: Settings) -> Self {
        Self {
            host: Host::new(&limits, settings, crypto()).expect("host fixtures"),
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            up: Queue::uncapped(),
            io: Queue::uncapped(),
        }
    }
    fn down(&mut self, request: Request) {
        self.host.down(&self.env, request, &mut self.up, &mut self.io);
    }
    fn plan(&mut self, run: u64, plan: Plan) {
        self.down(Request::Run { run: Token::new(run), plan });
    }
    fn run(&mut self, run: u64, forge: u16) {
        self.down(Request::Run { run: Token::new(run), plan: Plan::Reconcile { forge, adopted: Box::new([]) } });
    }
    fn answer(&mut self, run: u64, answer: Answer) {
        self.down(Request::Answer { run: Token::new(run), answer });
    }
    fn event(&mut self, event: skein_io::Event) {
        self.host.up(&self.env, event, &mut self.up, &mut self.io);
    }
    fn fire(&mut self, millis: u64) {
        self.env.now = Time::ZERO.saturating_add(Duration::from_millis(millis));
        self.host.fire(&self.env, &mut self.up, &mut self.io);
    }
    fn read_admin(&mut self) {
        self.run(1, 1);
        assert_eq!(self.up.pop(), Some(Event::Need { run: Token::new(1), need: Need::Read(Item::AdminToken) }));
    }
    fn send(&mut self) {
        self.read_admin();
        self.answer(1, Answer::Read(Some(fixtures::secret(fixtures::ADMIN))));
    }
    fn connect(&mut self) -> Token {
        let Some(skein_io::Request::Connect { owner, addr }) = self.io.pop() else {
            panic!("Connect");
        };
        assert_eq!(addr, skein_io::Addr::Ip(([127, 0, 0, 1], 3000).into()));
        assert_eq!(self.io.len(), 0);
        owner
    }
    fn refused(&mut self) {
        let owner = self.connect();
        self.event(skein_io::Event::Connecting {
            owner,
            socket: Token::new(owner.raw().checked_add(100).expect("small connection count")),
        });
        self.event(skein_io::Event::Failed { owner, error: skein_io::Error::Refused });
        // io closes a failed connect without waiting for this request.
        let _close = self.io.pop();
        self.event(skein_io::Event::Closed { owner });
        assert_eq!(self.up.len(), 0);
    }
}
fn stop(run: u64, forge: u16, why: Why) -> Event {
    Event::Stop {
        run: Token::new(run),
        failure: Failure { forge, step: Stage::Admin, subject: b"kit-admin".as_slice().into(), why },
    }
}
#[test]
fn a_runs_given_takes_the_plans_adopted_and_the_rest_from_the_forge() {
    let mut forge = fixtures::given();
    forge.adopted = Box::new([99]);
    forge.orgs = Box::new([b"acme".as_slice().into()]);
    forge.org_owner = Some(b"ada".as_slice().into());
    forge.listener = Some(([127, 0, 0, 1], 9898).into());
    let plan = [7, 3];
    let given = crate::host::run::given_for(&forge, &plan);
    let mut expected = forge.clone();
    expected.adopted = plan.into();
    assert_eq!(given, expected);
    assert_eq!(forge.adopted.as_ref(), &[99]);
    assert!(crate::host::run::given_for(&forge, &[]).adopted.is_empty());
}
#[test]
fn settings_with_too_many_adopted_repositories_are_refused() {
    use crate::given::{Place, Refused};
    let mut s = settings(&[1]);
    let given = forgejo_mut(&mut s);
    given.limits.repositories = 1;
    given.adopted = Box::new([1, 2]);
    assert_eq!(
        Host::new(&limits(), s, crypto()).err(),
        Some(Unusable::Given { forge: 1, refused: Refused { place: Place::Repositories, other: None } })
    );
}
#[test]
fn a_run_without_an_admin_token_stops_before_any_connection() {
    let mut w = World::new();
    w.read_admin();
    assert_eq!(w.io.len(), 0);
    w.answer(1, Answer::Read(None));
    assert_eq!(w.up.pop(), Some(stop(1, 1, Why::Missing)));
    assert_eq!(w.io.len(), 0);
}
#[test]
fn an_admin_token_that_breaks_its_item_is_absent() {
    let mut w = World::new();
    w.read_admin();
    w.answer(1, Answer::Read(Some(fixtures::secret(b"invalid token"))));
    assert_eq!(w.up.pop(), Some(stop(1, 1, Why::Missing)));
    assert_eq!(w.io.len(), 0);
}
#[test]
fn a_second_run_for_a_forge_with_one_live_stops_busy() {
    let mut w = World::new();
    w.read_admin();
    w.run(2, 1);
    assert_eq!(w.up.pop(), Some(stop(2, 1, Why::Busy)));
    w.answer(1, Answer::Read(None));
    assert_eq!(w.up.pop(), Some(stop(1, 1, Why::Missing)));
    w.run(3, 1);
    assert_eq!(w.up.pop(), Some(Event::Need { run: Token::new(3), need: Need::Read(Item::AdminToken) }));
}
#[test]
fn a_run_past_runs_stops_busy() {
    let mut l = limits();
    l.runs = 1;
    let mut w = World::with(l, settings(&[1, 2]));
    w.read_admin();
    w.run(2, 2);
    assert_eq!(w.up.pop(), Some(stop(2, 2, Why::Busy)));
}
#[test]
fn an_answer_with_no_need_waiting_is_ignored() {
    let mut w = World::new();
    w.answer(99, Answer::Written);
    assert_eq!(w.up.len(), 0);
    assert_eq!(w.io.len(), 0);
    w.send();
    let _owner = w.connect();
    w.answer(1, Answer::Written);
    assert_eq!(w.up.len(), 0);
    assert_eq!(w.io.len(), 0);
}
#[test]
fn a_run_waiting_on_the_owner_at_its_deadline_stops_down_at_its_step() {
    let mut w = World::new();
    w.read_admin();
    assert_eq!(w.host.next_deadline(), Some(Time::ZERO.saturating_add(Duration::from_secs(120))));
    w.fire(120_000);
    assert_eq!(w.up.pop(), Some(stop(1, 1, Why::Down)));
    w.answer(1, Answer::Read(None));
    assert_eq!(w.up.len(), 0);
    assert_eq!(w.host.next_deadline(), None);
}
#[test]
fn the_first_try_waits_double_from_backoff_to_backoff_max() {
    let mut w = World::new();
    w.send();
    for at in [0, 100, 300, 700, 1500, 3100, 6300, 11300, 16300] {
        if at != 0 {
            w.fire(at - 1);
            assert_eq!(w.io.len(), 0);
            w.fire(at);
        }
        w.refused();
    }
}
#[test]
fn the_deadline_cuts_the_last_wait_and_stops_the_run_down() {
    let mut s = settings(&[1]);
    let given = forgejo_mut(&mut s);
    given.limits.deadline = Duration::from_secs(1);
    given.limits.backoff_max = Duration::from_millis(400);
    let mut w = World::with(limits(), s);
    w.send();
    for at in [0, 100, 300, 700] {
        w.fire(at);
        w.refused();
    }
    assert_eq!(w.host.next_deadline(), Some(Time::ZERO.saturating_add(Duration::from_secs(1))));
    w.fire(1000);
    assert_eq!(w.up.pop(), Some(stop(1, 1, Why::Down)));
    assert_eq!(w.io.len(), 0);
    assert_eq!(w.host.next_deadline(), None);
}
#[test]
fn close_says_closed_only_once_io_has_closed_every_connection() {
    let mut w = World::new();
    w.send();
    let owner = w.connect();
    w.down(Request::Close);
    assert_eq!(w.up.len(), 0);
    assert_eq!(w.io.len(), 0);
    let socket = Token::new(100);
    w.event(skein_io::Event::Connecting { owner, socket });
    assert_eq!(w.io.pop(), Some(skein_io::Request::Abort { entity: socket }));
    assert_eq!(w.up.len(), 0);
    w.event(skein_io::Event::Closed { owner });
    assert_eq!(w.up.pop(), Some(Event::Closed));
    w.event(skein_io::Event::Closed { owner });
    w.run(2, 1);
    assert_eq!(w.up.len(), 0);
    assert_eq!(w.io.len(), 0);
    assert_eq!(w.host.next_deadline(), None);
}
#[test]
#[expect(clippy::disallowed_macros, reason = "the redaction test inspects the host's Debug rendering")]
fn the_hosts_debug_shows_no_token_in_flight() {
    let mut w = World::new();
    w.send();
    let rendered = format!("{:?}", w.host);
    for bytes in rendered.as_bytes().windows(fixtures::ADMIN.len()) {
        assert_ne!(bytes, fixtures::ADMIN, "credentials stay out of Debug");
    }
}
#[test]
fn the_host_refuses_unusable_settings_naming_each() {
    use crate::given::{Place, Refused};
    use skein_forge::forgejo::limits::Problem;
    let mut cases = List::with_capacity(10);
    let mut l = limits();
    l.runs = 0;
    cases.push((l, settings(&[1]), Unusable::Zero)).unwrap();
    let mut l = limits();
    l.token_bytes = 63;
    cases.push((l, settings(&[1]), Unusable::TokenBytes { token_bytes: 63 })).unwrap();
    let mut l = limits();
    l.depth = 7;
    cases.push((l, settings(&[1]), Unusable::Documents(Problem::Depth))).unwrap();
    let mut l = limits();
    l.io.intake = 16;
    cases.push((l, settings(&[1]), Unusable::Stack { stacked: Stacked::HttpOverIo, demand: 4096, cap: 16 })).unwrap();
    cases.push((limits(), settings(&[1, 2, 3]), Unusable::Forges { forges: 2 })).unwrap();
    cases.push((limits(), settings(&[1, 1]), Unusable::Duplicate { forge: 1 })).unwrap();
    let mut s = settings(&[1]);
    s.forges.get_mut(0).unwrap().address = skein_io::Addr::Ip(([192, 0, 2, 1], 3000).into());
    cases.push((limits(), s, Unusable::Address { forge: 1 })).unwrap();
    let mut s = settings(&[1]);
    s.forges.get_mut(0).unwrap().transport = Transport::Plaintext { name: Some(b"".as_slice().into()) };
    cases.push((limits(), s, Unusable::PlaintextName { forge: 1 })).unwrap();
    let mut s = settings(&[1]);
    s.user_agent = b"kit\r\n".as_slice().into();
    cases.push((limits(), s, Unusable::Request { forge: 1 })).unwrap();
    let mut s = settings(&[1]);
    forgejo_mut(&mut s).bot_login = b"".as_slice().into();
    cases
        .push((limits(), s, Unusable::Given { forge: 1, refused: Refused { place: Place::BotLogin, other: None } }))
        .unwrap();
    for (l, s, expected) in cases.into_boxed() {
        assert_eq!(Host::new(&l, s, crypto()).err(), Some(expected));
    }
}

#[test]
fn a_connect_without_a_socket_retries_from_its_closed_time() {
    let mut w = World::new();
    w.send();
    let owner = w.connect();
    w.event(skein_io::Event::Failed { owner, error: skein_io::Error::Busy });
    assert_eq!(w.io.len(), 0);
    w.fire(200);
    assert_eq!(w.io.len(), 0);
    w.event(skein_io::Event::Closed { owner });
    assert_eq!(w.host.next_deadline(), Some(Time::ZERO.saturating_add(Duration::from_millis(300))));
    w.fire(299);
    assert_eq!(w.io.len(), 0);
    w.fire(300);
    let next = w.connect();
    assert_ne!(owner, next);
    assert_eq!(w.up.len(), 0);
}
#[test]
fn a_wrong_answer_kind_stops_the_waiting_plan_unexpected() {
    let mut w = World::new();
    w.read_admin();
    w.answer(1, Answer::Written);
    assert_eq!(w.up.pop(), Some(stop(1, 1, Why::Unexpected)));
    assert_eq!(w.io.len(), 0);
}
#[test]
fn an_empty_host_closes_once_and_ignores_later_requests() {
    let mut w = World::new();
    w.down(Request::Close);
    assert_eq!(w.up.pop(), Some(Event::Closed));
    w.down(Request::Close);
    w.run(1, 1);
    w.answer(1, Answer::Written);
    w.fire(120_000);
    assert_eq!(w.up.len(), 0);
    assert_eq!(w.io.len(), 0);
    assert_eq!(w.host.next_deadline(), None);
}
#[test]
fn one_timer_turn_stops_every_run_whose_deadline_passed() {
    let mut l = limits();
    l.runs = 10;
    l.forges = 10;
    let mut w = World::with(l, settings(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]));
    for number in 1_u16..=10 {
        w.run(u64::from(number), number);
        assert_eq!(
            w.up.pop(),
            Some(Event::Need { run: Token::new(u64::from(number)), need: Need::Read(Item::AdminToken) })
        );
    }
    w.fire(120_000);
    for run in 1_u64..=10 {
        assert_eq!(w.up.pop(), Some(stop(run, u16::try_from(run).unwrap(), Why::Down)));
    }
    assert!(w.up.is_empty());
    assert_eq!(w.host.next_deadline(), None);
    assert_eq!(w.io.len(), 0);
}

fn check() -> Plan {
    Plan::Check { forge: 1 }
}
fn take_over(token: &[u8]) -> Plan {
    Plan::TakeOver { forge: 1, admin_token: fixtures::secret(token) }
}
fn adopt() -> Plan {
    Plan::Adopt { forge: 1, repository: core::num::NonZeroU64::new(7).unwrap() }
}
fn token_stop(why: Why) -> Event {
    Event::Stop {
        run: Token::new(1),
        failure: Failure { forge: 1, step: Stage::Token, subject: b"api".as_slice().into(), why },
    }
}
fn read(w: &mut World, item: Item) {
    assert_eq!(w.up.pop(), Some(Event::Need { run: Token::new(1), need: Need::Read(item) }));
    assert!(w.up.is_empty());
    assert!(w.io.is_empty());
}
#[test]
fn a_check_run_asks_for_the_api_token_and_a_missing_one_stops_naming_api() {
    let mut w = World::new();
    w.plan(1, check());
    read(&mut w, Item::ApiToken);
    w.answer(1, Answer::Read(None));
    assert_eq!(w.up.pop(), Some(token_stop(Why::Missing)));
    w.plan(2, check());
    assert_eq!(w.up.pop(), Some(Event::Need { run: Token::new(2), need: Need::Read(Item::ApiToken) }));
}
#[test]
fn a_check_run_waiting_on_the_owner_at_its_deadline_stops_down_naming_its_token() {
    let mut w = World::new();
    w.plan(1, check());
    read(&mut w, Item::ApiToken);
    assert_eq!(w.host.next_deadline(), Some(Time::ZERO.saturating_add(Duration::from_secs(120))));
    w.fire(120_000);
    assert_eq!(w.up.pop(), Some(token_stop(Why::Down)));
    assert_eq!(w.host.next_deadline(), None);
}
fn silent_deadline(w: &mut World, expected: Event) {
    let owner = w.connect();
    let socket = Token::new(100);
    w.event(skein_io::Event::Connecting { owner, socket });
    assert!(w.io.is_empty());
    w.fire(120_000);
    assert_eq!(w.up.pop(), Some(expected));
    assert_eq!(w.io.pop(), Some(skein_io::Request::Abort { entity: socket }));
    assert!(w.io.is_empty());
    assert!(w.up.is_empty());
    w.event(skein_io::Event::Closed { owner });
    assert_eq!(w.host.next_deadline(), None);
}
#[test]
fn a_check_whose_forge_never_answers_stops_down_naming_the_token_in_flight() {
    let mut w = World::new();
    w.plan(1, check());
    read(&mut w, Item::ApiToken);
    w.answer(1, Answer::Read(Some(fixtures::secret(b"api-token"))));
    silent_deadline(&mut w, token_stop(Why::Down));
}
#[test]
fn a_take_over_run_sends_the_handed_token_and_asks_the_owner_for_nothing() {
    let mut w = World::new();
    w.plan(1, take_over(&[b't'; 40]));
    assert!(w.up.is_empty());
    w.connect();
}
#[test]
fn a_take_over_token_that_is_empty_or_breaks_its_item_stops_missing_at_once() {
    for token in [b"".as_slice(), b"has a space", &[b't'; 65]] {
        let mut w = World::new();
        w.plan(1, take_over(token));
        assert_eq!(w.up.pop(), Some(stop(1, 1, Why::Missing)));
        assert!(w.io.is_empty());
        assert_eq!(w.host.next_deadline(), None);
        w.fire(1000);
        w.run(2, 1);
        assert_eq!(w.up.pop(), Some(Event::Need { run: Token::new(2), need: Need::Read(Item::AdminToken) }));
        assert_eq!(w.host.next_deadline(), Some(Time::ZERO.saturating_add(Duration::from_secs(121))));
    }
}
#[test]
fn a_take_over_token_of_seven_bytes_connects() {
    let mut w = World::new();
    w.plan(1, take_over(b"1234567"));
    assert!(w.up.is_empty());
    w.connect();
}
#[test]
fn a_take_over_run_whose_forge_never_answers_stops_down_at_its_step() {
    let mut w = World::new();
    w.plan(1, take_over(fixtures::TAKEN));
    silent_deadline(&mut w, stop(1, 1, Why::Down));
}
#[test]
#[expect(clippy::disallowed_macros, reason = "the redaction test inspects request and host Debug rendering")]
fn no_taken_token_is_in_a_requests_or_a_hosts_debug() {
    let mut w = World::new();
    let request = Request::Run { run: Token::new(1), plan: take_over(fixtures::TAKEN) };
    let rendered = format!("{request:?}");
    w.down(request);
    for debug in [rendered, format!("{:?}", w.host)] {
        for fragment in [fixtures::TAKEN, fixtures::TAKEN_SUFFIX] {
            for bytes in debug.as_bytes().windows(fragment.len()) {
                assert_ne!(bytes, fragment);
            }
        }
    }
}
#[test]
fn an_adopt_run_asks_for_the_admin_token_and_a_missing_one_stops_naming_the_admin() {
    let mut w = World::new();
    w.plan(1, adopt());
    read(&mut w, Item::AdminToken);
    w.answer(1, Answer::Read(None));
    assert_eq!(w.up.pop(), Some(stop(1, 1, Why::Missing)));
    assert_eq!(w.host.next_deadline(), None);
}
#[test]
fn an_adopt_run_waiting_on_the_owner_at_its_deadline_stops_down_at_its_step() {
    let mut w = World::new();
    w.plan(1, adopt());
    read(&mut w, Item::AdminToken);
    assert_eq!(w.host.next_deadline(), Some(Time::ZERO.saturating_add(Duration::from_secs(120))));
    w.fire(120_000);
    assert_eq!(w.up.pop(), Some(stop(1, 1, Why::Down)));
}
fn plans() -> [Plan; 5] {
    [
        Plan::Reconcile { forge: 1, adopted: Box::new([]) },
        check(),
        take_over(fixtures::TAKEN),
        adopt(),
        Plan::Create {
            forge: 1,
            org: b"first".as_slice().into(),
            repository: b"starter".as_slice().into(),
            files: Box::new([]),
        },
    ]
}
#[test]
fn a_second_run_of_any_plan_for_a_forge_with_one_live_stops_busy() {
    for first in plans() {
        for second in plans() {
            let mut w = World::new();
            w.plan(1, first.clone());
            let connection = match first {
                Plan::Reconcile { .. } | Plan::Check { .. } | Plan::Adopt { .. } | Plan::Create { .. } => {
                    assert!(w.up.pop().is_some());
                    None
                }
                Plan::TakeOver { .. } => Some(w.connect()),
                Plan::Reachable { .. }
                | Plan::Manifest { .. }
                | Plan::Convert { .. }
                | Plan::Paste { .. }
                | Plan::Install { .. }
                | Plan::Accept { .. } => {
                    unreachable!("Forgejo writing plan table")
                }
            };
            w.plan(2, second);
            assert_eq!(w.up.pop(), Some(stop(2, 1, Why::Busy)));
            assert!(w.up.is_empty());
            if let Some(owner) = connection {
                use skein_lib::stream::{Down, Up};
                let socket = Token::new(100);
                w.event(skein_io::Event::Connecting { owner, socket });
                w.event(skein_io::Event::Connected { owner });
                let Some(skein_io::Request::Stream { down: Down::Demand { .. }, .. }) = w.io.pop() else {
                    panic!("stream demand");
                };
                w.event(skein_io::Event::Stream { owner, up: Up::Room });
                let Some(skein_io::Request::Stream { down: Down::Send(_), .. }) = w.io.pop() else {
                    panic!("request head");
                };
                for line in [b"HTTP/1.1 401 Unauthorized\r\n".as_slice(), b"Content-Length: 0\r\n", b"\r\n"] {
                    let Some(skein_io::Request::Stream { down: Down::Demand { .. }, .. }) = w.io.pop() else {
                        panic!("stream demand");
                    };
                    w.event(skein_io::Event::Stream { owner, up: Up::Bytes(line.into()) });
                }
                assert_eq!(w.up.pop(), Some(stop(1, 1, Why::Refused(401))));
            } else {
                w.answer(1, Answer::Read(None));
                let expected = match first {
                    Plan::Check { .. } => token_stop(Why::Missing),
                    Plan::Reconcile { .. } | Plan::Adopt { .. } | Plan::TakeOver { .. } | Plan::Create { .. } => {
                        stop(1, 1, Why::Missing)
                    }
                    Plan::Reachable { .. }
                    | Plan::Manifest { .. }
                    | Plan::Convert { .. }
                    | Plan::Paste { .. }
                    | Plan::Install { .. }
                    | Plan::Accept { .. } => {
                        unreachable!("Forgejo writing plan table")
                    }
                };
                assert_eq!(w.up.pop(), Some(expected));
            }
        }
    }
}

fn reachable(forge: u16) -> Plan {
    Plan::Reachable { forge, installation: None, page: core::num::NonZeroU32::new(1).unwrap() }
}
fn owner_read(run: u64) -> Event {
    Event::Need { run: Token::new(run), need: Need::Read(Item::AdminToken) }
}
fn reachable_stop(run: u64, why: Why) -> Event {
    Event::Stop {
        run: Token::new(run),
        failure: Failure { forge: 1, step: Stage::Token, subject: b"api".as_slice().into(), why },
    }
}
fn paired() -> World {
    let mut w = World::new();
    w.run(1, 1);
    w.plan(2, reachable(1));
    while w.up.pop().is_some() {}
    w
}
#[test]
fn a_forge_whose_page_could_pass_the_listing_bound_is_unusable() {
    let l = limits();
    let mut s = settings(&[1]);
    forgejo_mut(&mut s).limits.reachable = l.inner + 1;
    assert_eq!(Host::new(&l, s, crypto()).err(), Some(Unusable::Reachable { forge: 1, reachable: 33, inner: 32 }));
}
#[test]
fn a_page_bound_equal_to_the_listing_bound_is_usable() {
    let l = limits();
    let mut s = settings(&[1]);
    forgejo_mut(&mut s).limits.reachable = l.inner;
    Host::new(&l, s, crypto()).expect("a picker page at the listing bound is usable");
}
#[test]
fn a_zero_page_bound_is_refused_by_the_given_first() {
    use crate::given::{Place, Refused};
    let mut s = settings(&[1]);
    forgejo_mut(&mut s).limits.reachable = 0;
    assert_eq!(
        Host::new(&limits(), s, crypto()).err(),
        Some(Unusable::Given { forge: 1, refused: Refused { place: Place::ReachableLimit, other: None } })
    );
}
#[test]
fn a_reachable_run_beside_a_writing_run_of_its_forge_is_live_with_it() {
    let mut w = World::new();
    w.read_admin();
    w.plan(2, reachable(1));
    assert_eq!(w.up.pop(), Some(owner_read(2)));
    w.answer(2, Answer::Read(None));
    assert_eq!(w.up.pop(), Some(Event::Need { run: Token::new(2), need: Need::Read(Item::ApiToken) }));
    w.answer(2, Answer::Read(None));
    assert_eq!(w.up.pop(), Some(reachable_stop(2, Why::Missing)));
    w.answer(1, Answer::Read(None));
    assert_eq!(w.up.pop(), Some(stop(1, 1, Why::Missing)));
    assert!(w.up.is_empty());
    assert_eq!(w.host.next_deadline(), None);
}
#[test]
fn a_second_reachable_run_for_a_forge_stops_busy_naming_the_admin() {
    let mut w = World::new();
    w.plan(2, reachable(1));
    w.up.pop();
    w.plan(3, reachable(1));
    assert_eq!(w.up.pop(), Some(stop(3, 1, Why::Busy)));
    w.answer(2, Answer::Read(None));
    assert_eq!(w.up.pop(), Some(Event::Need { run: Token::new(2), need: Need::Read(Item::ApiToken) }));
    w.answer(2, Answer::Read(None));
    assert_eq!(w.up.pop(), Some(reachable_stop(2, Why::Missing)));
    w.answer(3, Answer::Read(None));
    assert!(w.up.is_empty());
}
#[test]
fn a_second_writing_run_beside_a_reachable_and_a_writing_one_stops_busy() {
    for plan in plans() {
        let mut w = paired();
        w.plan(3, plan);
        assert_eq!(w.up.pop(), Some(stop(3, 1, Why::Busy)));
    }
}
#[test]
fn a_reachable_run_for_another_forge_is_live_beside_a_reachable_run() {
    let mut w = World::with(limits(), settings(&[1, 2]));
    for (run, forge) in [(1, 1), (2, 2)] {
        w.plan(run, reachable(forge));
        assert_eq!(w.up.pop(), Some(owner_read(run)));
    }
    for run in [2, 1] {
        w.answer(run, Answer::Read(Some(fixtures::secret(fixtures::ADMIN))));
        w.connect();
    }
    assert!(w.up.is_empty());
}
#[test]
fn runs_counts_reachable_and_writing_runs_together() {
    let mut l = limits();
    l.forges = 3;
    let mut w = World::with(l, settings(&[1, 2, 3]));
    w.read_admin();
    w.plan(2, reachable(2));
    w.up.pop();
    for plan in [reachable(3), Plan::Reconcile { forge: 3, adopted: Box::new([]) }] {
        w.plan(3, plan);
        assert_eq!(w.up.pop(), Some(stop(3, 3, Why::Busy)));
    }
    for (first, second) in [
        (Plan::Reconcile { forge: 1, adopted: Box::new([]) }, reachable(1)),
        (reachable(1), Plan::Reconcile { forge: 1, adopted: Box::new([]) }),
    ] {
        let mut l = limits();
        l.runs = 1;
        let mut w = World::with(l, settings(&[1]));
        w.plan(1, first);
        w.up.pop();
        w.plan(2, second);
        assert_eq!(w.up.pop(), Some(stop(2, 1, Why::Busy)));
    }
}
#[test]
fn a_reachable_run_asks_the_owner_for_the_admin_token_and_nothing_else() {
    let mut w = World::new();
    w.plan(1, reachable(1));
    read(&mut w, Item::AdminToken);
    w.answer(1, Answer::Read(Some(fixtures::secret(fixtures::ADMIN))));
    w.connect();
    assert!(w.up.is_empty());
}
#[test]
fn a_reachable_run_with_neither_token_stops_naming_the_api_token() {
    let mut w = World::new();
    w.plan(2, reachable(1));
    w.up.pop();
    w.answer(2, Answer::Read(None));
    assert_eq!(w.up.pop(), Some(Event::Need { run: Token::new(2), need: Need::Read(Item::ApiToken) }));
    assert!(w.host.next_deadline().is_some());
    assert!(w.io.is_empty());
    w.answer(2, Answer::Read(None));
    assert_eq!(w.up.pop(), Some(reachable_stop(2, Why::Missing)));
    assert!(w.io.is_empty());
    assert_eq!(w.host.next_deadline(), None);
    w.plan(3, reachable(1));
    assert_eq!(w.up.pop(), Some(owner_read(3)));
}
#[test]
fn a_reachable_plan_for_an_installation_on_forgejo_stops_at_once_and_is_never_live() {
    let mut w = World::new();
    w.read_admin();
    let deadline = w.host.next_deadline();
    w.fire(50_000);
    w.plan(2, Plan::Reachable { forge: 1, installation: Some(7), page: core::num::NonZeroU32::new(1).unwrap() });
    assert_eq!(
        w.up.pop(),
        Some(Event::Stop {
            run: Token::new(2),
            failure: Failure {
                forge: 1,
                step: Stage::Installation,
                subject: b"7".as_slice().into(),
                why: Why::Unexpected
            },
        })
    );
    assert_eq!(w.host.next_deadline(), deadline);
    assert!(w.io.is_empty());
    w.plan(3, reachable(1));
    assert_eq!(w.up.pop(), Some(owner_read(3)));
}
#[test]
fn a_reachable_run_at_its_deadline_stops_down_at_its_step() {
    let mut w = World::new();
    w.plan(1, reachable(1));
    read(&mut w, Item::AdminToken);
    w.fire(120_000);
    assert_eq!(w.up.pop(), Some(stop(1, 1, Why::Down)));
    assert_eq!(w.host.next_deadline(), None);
    let mut w = World::new();
    w.plan(1, reachable(1));
    read(&mut w, Item::AdminToken);
    w.answer(1, Answer::Read(Some(fixtures::secret(fixtures::ADMIN))));
    silent_deadline(&mut w, stop(1, 1, Why::Down));
}
#[test]
fn each_run_keeps_its_own_deadline_when_two_are_live() {
    let mut w = World::new();
    w.read_admin();
    w.fire(50_000);
    w.plan(2, reachable(1));
    assert_eq!(w.up.pop(), Some(owner_read(2)));
    assert_eq!(w.host.next_deadline(), Some(Time::ZERO.saturating_add(Duration::from_secs(120))));
    w.fire(120_000);
    assert_eq!(w.up.pop(), Some(stop(1, 1, Why::Down)));
    assert!(w.up.is_empty());
    assert_eq!(w.host.next_deadline(), Some(Time::ZERO.saturating_add(Duration::from_secs(170))));
    w.fire(170_000);
    assert_eq!(w.up.pop(), Some(stop(2, 1, Why::Down)));
    assert_eq!(w.host.next_deadline(), None);
}
#[test]
fn close_ends_both_runs_unheard_and_says_closed_once_io_has_closed_both_connections() {
    let mut w = paired();
    let mut connections = List::with_capacity(2);
    for run in [1, 2] {
        w.answer(run, Answer::Read(Some(fixtures::secret(fixtures::ADMIN))));
        let owner = w.connect();
        connections.push(owner).unwrap();
    }
    w.down(Request::Close);
    assert!(w.up.is_empty());
    assert!(w.io.is_empty());
    for owner in &connections {
        w.event(skein_io::Event::Connecting { owner: *owner, socket: *owner });
        assert_eq!(w.io.pop(), Some(skein_io::Request::Abort { entity: *owner }));
    }
    w.event(skein_io::Event::Closed { owner: connections.as_slice()[0] });
    assert!(w.up.is_empty());
    w.event(skein_io::Event::Closed { owner: connections.as_slice()[1] });
    assert_eq!(w.up.pop(), Some(Event::Closed));
    w.fire(120_000);
    w.event(skein_io::Event::Closed { owner: connections.as_slice()[0] });
    assert!(w.up.is_empty());
    assert!(w.io.is_empty());
    assert_eq!(w.host.next_deadline(), None);
}
#[test]
fn a_halted_reachable_stops_at_each_phase_with_the_why_given() {
    use crate::host::run::Machine;
    for (machine, stage, subject) in super::reachable::phase_cases().into_iter().skip(1) {
        let mut machine = Machine::Reachable(machine);
        let mut out = Queue::uncapped();
        machine.halt(Why::Down, &mut out);
        assert_eq!(out.pop(), Some(fixtures::named_failure(stage, subject, Why::Down)));
        assert!(out.is_empty());
    }
}
#[test]
fn a_halt_of_an_ended_reachable_pushes_nothing() {
    use crate::{host::run::Machine, reconcile};
    for event in [reconcile::Event::Reply(reconcile::Reply::Rows(Box::new([]))), reconcile::Event::Written] {
        let (machine, _, _) = super::reachable::phase_cases().into_iter().last().unwrap();
        let mut machine = Machine::Reachable(machine);
        let mut out = Queue::uncapped();
        machine.step(event, &mut out);
        out.pop();
        machine.halt(Why::Down, &mut out);
        assert!(out.is_empty());
    }
}

const ADMIN_BODY: &[u8] = br#"{"id":1,"login":"kit-admin","is_admin":true,"full_name":""}"#;
const BOT_BODY: &[u8] = br#"{"id":42,"login":"kit","is_admin":false,"full_name":"kit:bot"}"#;
const SETTINGS_BODY: &[u8] = br#"{"max_response_items":4,"default_paging_num":30}"#;
impl World {
    #[expect(clippy::disallowed_macros, reason = "the HTTP fixture frames its body's length")]
    fn reply(&mut self, body: &[u8]) {
        use skein_lib::{
            Intake,
            stream::{Down, Up},
        };
        let owner = self.connect();
        self.event(skein_io::Event::Connecting { owner, socket: owner });
        self.event(skein_io::Event::Connected { owner });
        let mut intake = Intake::with_capacity(limits().body_bytes_in);
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
        intake.append(head.as_bytes()).unwrap();
        intake.append(body).unwrap();
        for _ in 0_u32..4096 {
            match self.io.pop() {
                Some(skein_io::Request::Stream { down: Down::Demand { read, room }, .. }) => {
                    if room > 0 {
                        self.event(skein_io::Event::Stream { owner, up: Up::Room });
                    } else if let Some(bytes) = intake.meet(read) {
                        self.event(skein_io::Event::Stream { owner, up: Up::Bytes(bytes) });
                    }
                }
                Some(skein_io::Request::Stream { down: Down::Send(_) | Down::Finish, .. }) => (),
                Some(skein_io::Request::Close { entity }) => {
                    assert_eq!(entity, owner);
                    self.event(skein_io::Event::Closed { owner });
                    return;
                }
                Some(other) => panic!("unexpected io: {other:?}"),
                None => self.host.fire(&self.env, &mut self.up, &mut self.io),
            }
        }
        panic!("HTTP reply did not settle");
    }
    fn finish_reachable(&mut self) {
        self.answer(2, Answer::Read(Some(fixtures::secret(fixtures::ADMIN))));
        self.reply(ADMIN_BODY);
        self.reply(SETTINGS_BODY);
        self.reply(br#"{"ok":true,"data":[]}"#);
    }
    fn finish_reconcile(&mut self) {
        self.answer(1, Answer::Read(Some(fixtures::secret(fixtures::ADMIN))));
        self.reply(ADMIN_BODY);
        self.reply(BOT_BODY);
        self.up.pop();
        for _ in [Item::ApiToken, Item::GitToken] {
            self.answer(1, Answer::Read(Some(fixtures::secret(b"bot-token"))));
            self.reply(BOT_BODY);
            self.up.pop();
        }
        self.answer(1, Answer::Read(Some(fixtures::secret(&42_u64.to_le_bytes()))));
    }
}
#[test]
fn a_finished_run_frees_its_own_kind_only() {
    let mut w = paired();
    w.finish_reachable();
    assert_eq!(
        w.up.pop(),
        Some(Event::Done { run: Token::new(2), outcome: Outcome::Reachable { rows: Box::new([]), more: false } })
    );
    w.plan(3, reachable(1));
    assert_eq!(w.up.pop(), Some(owner_read(3)));
    w.run(4, 1);
    assert_eq!(w.up.pop(), Some(stop(4, 1, Why::Busy)));
    let mut w = paired();
    w.finish_reconcile();
    assert_eq!(w.up.pop(), Some(Event::Done { run: Token::new(1), outcome: Outcome::Nothing }));
    w.run(3, 1);
    assert_eq!(w.up.pop(), Some(owner_read(3)));
    w.plan(4, reachable(1));
    assert_eq!(w.up.pop(), Some(stop(4, 1, Why::Busy)));
}

fn github_forge() -> Forge {
    Forge {
        kind: Kind::GitHub(super::github_fixtures::given()),
        address: skein_io::Addr::Ip(([127, 0, 0, 1], 3001).into()),
        transport: Transport::Plaintext { name: None },
    }
}
pub(crate) fn github_settings() -> Settings {
    let mut settings = settings(&[1, 4]);
    *settings.forges.get_mut(1).expect("second forge") = github_forge();
    settings
}
fn github_mut(settings: &mut Settings) -> &mut crate::github::given::Given {
    match &mut settings.forges.get_mut(1).expect("second forge").kind {
        Kind::Forgejo(_) => panic!("GitHub fixture"),
        Kind::GitHub(given) => given,
    }
}
#[test]
fn the_host_admits_a_github_forge_beside_a_forgejo_forge() {
    let settings = github_settings();
    let forgejo = settings.forges.get(0).expect("first forge");
    assert_eq!(forgejo.number(), 1);
    assert!(forgejo.forgejo().is_some());
    assert!(forgejo.github().is_none());
    let github = settings.forges.get(1).expect("second forge");
    assert_eq!(github.number(), 4);
    assert!(github.forgejo().is_none());
    assert_eq!(github.github(), Some(&super::github_fixtures::given()));
    Host::new(&limits(), settings, crypto()).expect("both kinds admitted");
}
#[test]
fn a_github_given_that_is_refused_is_named() {
    use crate::github::given::{Place, Refused};
    let mut settings = github_settings();
    github_mut(&mut settings).limits.reachable = 0;
    assert_eq!(
        Host::new(&limits(), settings, crypto()).err(),
        Some(Unusable::GitHubGiven { forge: 4, refused: Refused { place: Place::Reachable, other: None } })
    );
}
#[test]
fn a_github_forge_off_loopback_is_refused_as_a_forgejo_is() {
    let mut settings = github_settings();
    settings.forges.get_mut(1).expect("second forge").address = skein_io::Addr::Ip(([192, 0, 2, 1], 3001).into());
    assert_eq!(Host::new(&limits(), settings, crypto()).err(), Some(Unusable::Address { forge: 4 }));
}
#[test]
fn a_forge_number_used_by_both_kinds_is_a_duplicate() {
    for github_first in [false, true] {
        let mut settings = github_settings();
        github_mut(&mut settings).forge = 1;
        if github_first {
            let mut forges = List::with_capacity(2);
            let mut github = github_forge();
            github.kind = Kind::GitHub(crate::github::given::Given { forge: 1, ..super::github_fixtures::given() });
            forges.push(github).expect("first forge");
            forges.push(forge(1)).expect("second forge");
            settings.forges = forges;
        }
        assert_eq!(Host::new(&limits(), settings, crypto()).err(), Some(Unusable::Duplicate { forge: 1 }));
    }
}
#[test]
fn forges_counts_both_kinds() {
    let mut limits = limits();
    limits.forges = 1;
    assert_eq!(Host::new(&limits, github_settings(), crypto()).err(), Some(Unusable::Forges { forges: 1 }));
}
fn github_stop(run: u64, name: &[u8], why: Why) -> Event {
    Event::Stop { run: Token::new(run), failure: Failure { forge: 4, step: Stage::App, subject: name.into(), why } }
}
#[test]
#[expect(clippy::disallowed_macros, reason = "the failure's Debug must redact the handed token")]
fn a_forgejo_plan_on_a_github_forge_stops_at_once_unexpected() {
    let token = b"handed-token-that-must-stay-secret";
    for (plan, name) in [
        (Plan::Check { forge: 4 }, b"check".as_slice()),
        (Plan::TakeOver { forge: 4, admin_token: fixtures::secret(token) }, b"take-over"),
        (Plan::Adopt { forge: 4, repository: core::num::NonZeroU64::new(1).expect("repository") }, b"adopt"),
    ] {
        let mut limits = limits();
        limits.runs = 1;
        let mut w = World::with(limits, github_settings());
        // Reusing the owner's run token proves that the refused run is never live.
        for _ in 0..2u8 {
            w.plan(1, plan.clone());
            let event = w.up.pop().expect("immediate Stop");
            assert_eq!(event, github_stop(1, name, Why::Unexpected));
            let rendered = format!("{event:?}");
            for bytes in rendered.as_bytes().windows(token.len()) {
                assert_ne!(bytes, token, "credentials stay out of Debug");
            }
            assert_eq!(w.up.len(), 0);
            assert_eq!(w.io.len(), 0);
            assert_eq!(w.host.next_deadline(), None);
        }
        // A writing plan on the other forge can consume the host's only run slot.
        w.run(2, 1);
        assert_eq!(w.up.pop(), Some(Event::Need { run: Token::new(2), need: Need::Read(Item::AdminToken) }));
    }
}
#[test]
#[expect(clippy::disallowed_macros, reason = "a Busy failure's Debug must redact the handed token")]
fn a_busy_github_forge_names_the_plan_at_the_app() {
    let forgejo = forge(1);
    let github = github_forge();
    for (plan, name) in [
        (Plan::Reconcile { forge: 4, adopted: Box::new([]) }, b"reconcile".as_slice()),
        (Plan::Check { forge: 4 }, b"check"),
        (Plan::TakeOver { forge: 4, admin_token: fixtures::secret(b"handed-token") }, b"take-over"),
        (Plan::Adopt { forge: 4, repository: core::num::NonZeroU64::new(1).expect("repository") }, b"adopt"),
        (
            Plan::Reachable {
                forge: 4,
                installation: Some(1),
                page: core::num::NonZeroU32::new(1).expect("first page"),
            },
            b"reachable",
        ),
    ] {
        let failure = crate::host::about(&github, &plan, Why::Busy);
        assert_eq!(failure, Failure { forge: 4, step: Stage::App, subject: name.into(), why: Why::Busy });
        let rendered = format!("{failure:?}");
        for bytes in rendered.as_bytes().windows(b"handed-token".len()) {
            assert_ne!(bytes, b"handed-token", "credentials stay out of Debug");
        }
        assert_eq!(
            crate::host::about(&forgejo, &plan, Why::Busy),
            Failure { forge: 1, step: Stage::Admin, subject: b"kit-admin".as_slice().into(), why: Why::Busy }
        );
    }
}
fn github_reachable(installation: Option<u64>) -> Plan {
    Plan::Reachable { forge: 4, installation, page: core::num::NonZeroU32::new(1).expect("first page") }
}
#[test]
fn a_github_reachable_with_no_installation_stops_at_once() {
    let mut w = World::with(limits(), github_settings());
    w.plan(1, github_reachable(None));
    assert_eq!(
        w.up.pop(),
        Some(Event::Stop {
            run: Token::new(1),
            failure: Failure {
                forge: 4,
                step: Stage::Installation,
                subject: b"none".as_slice().into(),
                why: Why::Unexpected
            },
        })
    );
    assert_eq!(w.up.len(), 0);
    assert_eq!(w.io.len(), 0);
    assert_eq!(w.host.next_deadline(), None);
}
#[test]
fn a_github_reachable_asks_the_owner_for_the_accepted_set_first() {
    let mut w = World::with(limits(), github_settings());
    w.plan(1, github_reachable(Some(2)));
    assert_eq!(w.up.pop(), Some(Event::Need { run: Token::new(1), need: Need::Read(Item::Accepted) }));
    assert_eq!(w.up.len(), 0);
    assert_eq!(w.io.len(), 0);
}
#[test]
fn a_github_reachable_is_not_a_writing_run() {
    let mut w = World::with(limits(), github_settings());
    w.run(1, 1);
    assert_eq!(w.up.pop(), Some(Event::Need { run: Token::new(1), need: Need::Read(Item::AdminToken) }));
    w.plan(2, github_reachable(Some(2)));
    assert_eq!(w.up.pop(), Some(Event::Need { run: Token::new(2), need: Need::Read(Item::Accepted) }));
    assert_eq!(w.up.len(), 0);
    assert_eq!(w.io.len(), 0);
}
#[test]
fn a_github_forges_bounds_come_from_its_given() {
    use crate::item::{self, Bounds};
    let mut given = super::github_fixtures::given();
    given.limits.name_bytes = 3;
    given.limits.key_bytes = 99;
    given.limits.installations = 2;
    let forge = Forge { kind: Kind::GitHub(given), ..github_forge() };
    let bounds = crate::host::bounds(&limits(), &forge);
    assert_eq!(bounds, Bounds { token_bytes: 64, name_bytes: 3, key_bytes: 99, installations: 2 });
    assert!(item::fits(Item::AppSlug, b"kit", &bounds));
    assert!(!item::fits(Item::AppSlug, b"kits", &bounds));
    assert_eq!(
        crate::host::bounds(&limits(), &self::forge(1)),
        Bounds { token_bytes: 64, name_bytes: 0, key_bytes: 0, installations: 0 }
    );
}
#[test]
fn a_github_picker_page_must_fit_the_hosts_listing_bound() {
    let limits = limits();
    for reachable in [limits.inner, limits.inner + 1] {
        let mut settings = github_settings();
        github_mut(&mut settings).limits.reachable = reachable;
        let expected = if reachable > limits.inner {
            Some(Unusable::Reachable { forge: 4, reachable, inner: limits.inner })
        } else {
            None
        };
        assert_eq!(Host::new(&limits, settings, crypto()).err(), expected);
    }
}
#[test]
fn a_wrong_kind_plan_is_unexpected_even_when_the_host_is_full() {
    let mut limits = limits();
    limits.runs = 1;
    let mut w = World::with(limits, github_settings());
    w.read_admin();
    w.plan(2, Plan::Check { forge: 4 });
    assert_eq!(w.up.pop(), Some(github_stop(2, b"check", Why::Unexpected)));
    assert_eq!(w.up.len(), 0);
    assert_eq!(w.io.len(), 0);
}

#[test]
fn a_host_with_no_github_forge_checks_what_it_did() {
    const HOST_BYTES: u32 = 14; // 127.0.0.1:3000, from forge().
    const AGENT_BYTES: u32 = 9; // kit/0.0.0, from settings().
    const FRAMING: u32 = 192; // Forgejo's request-head overhead in host::limits.
    let mut l = limits();
    l.depth = 9;
    l.tls.send = 1;
    l.io.output = l.target_bytes + HOST_BYTES + l.token_bytes + AGENT_BYTES + FRAMING;
    Host::new(&l, settings(&[1]), crypto()).unwrap();
}
#[test]
fn a_github_forge_with_documents_that_cannot_hold_the_pin_is_unusable() {
    let mut l = limits();
    l.depth = 9;
    assert_eq!(
        Host::new(&l, github_settings(), crypto()).err(),
        Some(Unusable::GitHubDocuments(skein_forge::github::limits::Problem::Depth))
    );
    // Scalar and body strings have independent bounds.
    l.depth = skein_forge::github::limits::DEPTH;
    l.string_bytes = 27;
    Host::new(&l, github_settings(), crypto()).unwrap();
}

fn tls_settings() -> Settings {
    let root = std::fs::read("../../tests/tls/fixtures/root.der").expect("TLS world's root");
    let trust = skein_tls::Config::from_der(&[root.into_boxed_slice()], &[]).expect("valid root").0;
    let mut settings = settings(&[1]);
    settings.forges.get_mut(0).expect("forge").transport =
        Transport::Tls { server_name: skein_tls::Name::new("skein.test").expect("TLS name"), trust };
    settings
}
#[test]
fn a_tls_forge_of_either_kind_off_loopback_is_usable() {
    for github in [false, true] {
        let mut settings = tls_settings();
        let forge = settings.forges.get_mut(0).expect("forge");
        if github {
            forge.kind = Kind::GitHub(super::github_fixtures::given());
        }
        forge.address = skein_io::Addr::Ip(([192, 0, 2, 1], 3000).into());
        Host::new(&limits(), settings, crypto()).expect("TLS at any IP");
    }
}
#[test]
fn tls_stack_demands_past_the_caps_are_refused() {
    let mut l = limits();
    l.tls.read = 1;
    assert_eq!(
        Host::new(&l, tls_settings(), crypto()).err(),
        Some(Unusable::Stack { stacked: Stacked::HttpOverTls, demand: 4096, cap: 1 })
    );
    let mut l = limits();
    l.io.intake = 8192;
    assert_eq!(
        Host::new(&l, tls_settings(), crypto()).err(),
        Some(Unusable::Stack { stacked: Stacked::TlsOverIo, demand: skein_tls::client::LARGEST_READ, cap: 8192 })
    );
    let mut l = limits();
    l.tls.send = l.io.output;
    assert_eq!(
        Host::new(&l, tls_settings(), crypto()).err(),
        Some(Unusable::Stack {
            stacked: Stacked::TlsOverIo,
            demand: skein_tls::client::largest_room(&l.tls),
            cap: l.io.output
        })
    );
}
#[test]
fn in_process_forges_are_refused_on_both_transports() {
    for mut settings in [settings(&[1]), tls_settings()] {
        settings.forges.get_mut(0).expect("forge").address =
            skein_io::Addr::InProcess(skein_io::Name::new(b"setup").expect("in-process name"));
        assert_eq!(Host::new(&limits(), settings, crypto()).err(), Some(Unusable::Address { forge: 1 }));
    }
}

#[test]
fn tls_handshake_failure_answers_down_and_closes_for_both_dialects() {
    use crate::host::connection::Connection;
    use crate::reconcile::Reply;
    use skein_lib::stream::{Fault, Up};
    let settings = tls_settings();
    let forge = settings.forges.get(0).expect("forge");
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: limits() };
    let forgejo = Connection::new(
        Token::new(1),
        forge,
        b"kit",
        &skein_forge::forgejo::request::Request::User.into(),
        &fixtures::secret(fixtures::ADMIN),
        &env.limits,
    )
    .expect("Forgejo call");
    let github = Connection::github(
        Token::new(1),
        forge,
        b"kit",
        &skein_forge::github::request::Request::Conversion { code: b"code".as_slice().into() }.into(),
        &Auth::None,
        env.wall,
        &env.limits,
    )
    .expect("GitHub call");
    for mut connection in [forgejo, github] {
        let call = connection.call.as_ref().expect("held call");
        let mut host = None;
        for header in &call.headers {
            if header.name.as_ref() == b"Host" {
                host = Some(header.value.as_ref());
            }
        }
        assert_eq!(host, Some(b"skein.test:3000".as_slice()));
        connection.socket = Some(Token::new(2));
        connection.connect(&env);
        assert!(connection.call.is_some(), "HTTP waits for Ready");
        let mut io = Queue::with_capacity(16);
        for _ in 0_u32..8 {
            if !connection.pump(&env, &mut io) {
                break;
            }
        }
        connection.stream(&env, Up::Failed(Fault::Reset));
        for _ in 0_u32..16 {
            if !connection.pump(&env, &mut io) {
                break;
            }
        }
        assert_eq!(connection.result.take(), Some(Reply::Down));
        assert!(connection.call.is_none());
        let mut closed = false;
        for _ in 0..io.len() {
            if let Some(skein_io::Request::Close { entity }) = io.pop() {
                assert_eq!(entity, Token::new(2));
                closed = true;
            }
        }
        assert!(closed);
    }
}

#[test]
fn the_host_bound_refuses_invalid_settings_and_overflow() {
    use crate::host::worst_case;
    let l = limits();
    let mut duplicate = settings(&[1, 1]);
    assert_eq!(worst_case(&l, &duplicate), None);
    duplicate.forges.clear();
    duplicate.forges.push(forge(1)).expect("forge");
    duplicate.user_agent = Box::new([]);
    assert_eq!(worst_case(&l, &duplicate), None);
    assert_eq!(Host::new(&l, duplicate, crypto()).err(), Some(Unusable::Request { forge: 1 }));
    let mut invalid = github_settings();
    github_mut(&mut invalid).limits.reachable = 0;
    assert_eq!(worst_case(&l, &invalid), None);
    assert_eq!(
        Host::new(&l, invalid, crypto()).err(),
        Some(Unusable::GitHubGiven {
            forge: 4,
            refused: crate::github::given::Refused { place: crate::github::given::Place::Reachable, other: None },
        })
    );
    let mut huge = l;
    huge.runs = u32::MAX;
    huge.body_bytes_in = u32::MAX - 1;
    assert_eq!(worst_case(&huge, &settings(&[1])), None);
    assert_eq!(Host::new(&huge, settings(&[1]), crypto()).err(), Some(Unusable::Zero));
}
#[test]
fn the_host_bound_prices_only_configured_kinds_and_transports() {
    use crate::host::worst_case;
    let l = limits();
    let forgejo = worst_case(&l, &settings(&[1])).expect("bound");
    assert!(forgejo < worst_case(&l, &github_settings()).expect("mixed bound"));
    assert!(forgejo < worst_case(&l, &tls_settings()).expect("TLS bound"));
}
#[test]
fn an_ended_runs_settling_connection_keeps_its_slot_until_closed() {
    use skein_lib::stream::{Down, Up};
    let mut l = limits();
    l.runs = 1;
    let mut w = World::with(l, settings(&[1, 2]));
    w.send();
    let owner = w.connect();
    w.event(skein_io::Event::Connecting { owner, socket: owner });
    w.event(skein_io::Event::Connected { owner });
    let Some(skein_io::Request::Stream { down: Down::Demand { .. }, .. }) = w.io.pop() else {
        panic!("stream demand");
    };
    w.event(skein_io::Event::Stream { owner, up: Up::Room });
    let Some(skein_io::Request::Stream { down: Down::Send(_), .. }) = w.io.pop() else {
        panic!("request head");
    };
    for line in [b"HTTP/1.1 401 Unauthorized\r\n".as_slice(), b"Content-Length: 0\r\n", b"\r\n"] {
        let Some(skein_io::Request::Stream { down: Down::Demand { .. }, .. }) = w.io.pop() else {
            panic!("stream demand");
        };
        w.event(skein_io::Event::Stream { owner, up: Up::Bytes(line.into()) });
    }
    assert_eq!(w.up.pop(), Some(stop(1, 1, Why::Refused(401))));
    assert_eq!(w.io.pop(), Some(skein_io::Request::Close { entity: owner }));
    w.run(2, 2);
    assert_eq!(w.up.pop(), Some(stop(2, 2, Why::Busy)));
    assert!(w.io.is_empty());
    w.event(skein_io::Event::Closed { owner });
    w.run(2, 2);
    assert_eq!(w.up.pop(), Some(owner_read(2)));
}
