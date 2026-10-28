use super::{
    Answer, Event, Host, Kind, Limits, Plan, Request, Settings, Unusable, bounds, configured,
    run::Run,
    states::{Purpose, States},
};
use crate::{
    github::manifest::Target,
    reconcile::{self, Failure, Outcome, Stage, Why},
    tests::{fixtures::secret, github_fixtures, host as fixtures},
};
use skein_lib::{Duration, Env, Queue, Time, Token, Wall, bytes};

const FORGE: u16 = 4; // github_fixtures::given's forge.
const OTHER: u16 = 5;
const RUN: Token = Token::new(1);
const LIFE: Duration = Duration::from_secs(600); // The fixture's state_life.
const RANDOM: [u8; 16] =
    [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10];
const TEXT: &[u8] = b"0123456789abcdeffedcba9876543210";
const SECRET: &[u8; 16] = b"state-rng-secret";

#[expect(clippy::disallowed_macros, reason = "test checks diagnostics against secret byte spellings")]
fn diagnostic(text: &str) {
    assert!(bytes::find(text.as_bytes(), SECRET).is_none());
    for spelling in [format!("{SECRET:?}"), format!("{SECRET:x?}"), format!("{SECRET:X?}")] {
        assert!(!text.contains(&spelling));
    }
}
fn unknown(forge: u16, purpose: Purpose) -> Failure {
    let subject = match purpose {
        Purpose::Convert => b"convert".as_slice(),
        Purpose::Accept => b"accept".as_slice(),
    };
    Failure { forge, step: Stage::State, subject: subject.into(), why: Why::Unknown }
}
fn issue(states: &mut States, random: &[u8; 16], now: Time) -> Box<[u8]> {
    states.issue(FORGE, Purpose::Convert, random, now, LIFE, b"manifest").expect("unique state and finite expiry")
}
fn manifest(forge: u16) -> Plan {
    Plan::Manifest { forge, target: Target::Personal }
}
struct World {
    host: Host,
    env: Env<Limits>,
    up: Queue<Event>,
    io: Queue<skein_io::Request>,
}
impl World {
    fn new() -> Self {
        Self::with(fixtures::limits(), fixtures::github_settings())
    }
    fn with(limits: Limits, settings: Settings) -> Self {
        Self {
            host: Host::new(&limits, settings, fixtures::crypto()).expect("valid host"),
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            up: Queue::uncapped(),
            io: Queue::uncapped(),
        }
    }
    fn down(&mut self, request: Request) {
        self.host.down(&self.env, request, &mut self.up, &mut self.io);
        assert!(self.up.len() <= 1);
        assert!(self.io.is_empty());
    }
    fn run(&mut self, plan: Plan) {
        self.down(Request::Run { run: RUN, plan });
    }
    #[expect(clippy::disallowed_macros, reason = "test checks terminal event and host Debug for secret bytes")]
    fn one(&mut self) -> Event {
        assert_eq!(self.up.len(), 1);
        let event = self.up.pop().expect("one terminal event");
        diagnostic(&format!("{event:?} {:?}", self.host));
        event
    }
    fn page(&mut self, forge: u16) -> Box<[u8]> {
        self.run(manifest(forge));
        match self.one() {
            Event::Done { run: RUN, outcome: Outcome::Manifest { state, target, manifest } } => {
                assert_eq!(state.len(), 32);
                assert!(bytes::find(&target, &state).is_some());
                for part in [target, manifest] {
                    assert!(
                        u32::try_from(part.len()).expect("bounded form part") <= github_fixtures::limits().page_bytes
                    );
                }
                assert_eq!(self.host.next_deadline(), None);
                state
            }
            Event::Need { .. } | Event::Done { .. } | Event::Stop { .. } | Event::Closed => panic!("manifest Done"),
        }
    }
    fn admit(&mut self) -> Run {
        let forge = configured(&self.host.settings, FORGE);
        Run::new(
            forge,
            manifest(FORGE),
            bounds(&self.env.limits, forge),
            &self.env,
            &mut self.host.crypto,
            &mut self.host.states,
            &mut self.host.plan_out,
        )
        .expect("admitted manifest")
    }
    fn silent(&mut self) {
        self.down(Request::Answer { run: RUN, answer: Answer::Read(Some(secret(SECRET))) });
        assert!(self.up.is_empty());
        self.host.fire(&self.env, &mut self.up, &mut self.io);
        assert!(self.up.is_empty());
        assert!(self.io.is_empty());
        assert_eq!(self.host.next_deadline(), None);
    }
}

#[test]
#[expect(clippy::disallowed_macros, reason = "test independently encodes the injected host draw")]
fn a_state_is_32_lowercase_hex_bytes_from_the_host_rng() {
    let mut states = States::new(1);
    assert_eq!(issue(&mut states, &RANDOM, Time::ZERO).as_ref(), TEXT);
    let mut rng = fixtures::crypto();
    let mut random = [0_u8; 16];
    rng.fill(&mut random);
    let mut expected = [0_u8; 32];
    for (pair, byte) in expected.chunks_exact_mut(2).zip(random) {
        for (slot, digit) in pair.iter_mut().zip(format!("{byte:02x}").bytes()) {
            *slot = digit;
        }
    }
    let mut w = World::new();
    let state = w.page(FORGE);
    assert_eq!(state.as_ref(), expected);
    for byte in state.as_ref() {
        assert!(byte.is_ascii_digit() || (b'a'..=b'f').contains(byte));
    }
    let mut name = [0_u8; 3];
    rng.fill(&mut name);
    let mut next_state = [0_u8; 16];
    rng.fill(&mut next_state);
    let mut table = States::new(1);
    let expected_next = issue(&mut table, &next_state, Time::ZERO);
    assert_eq!(w.page(FORGE), expected_next); // Exactly 16 state bytes and three name bytes per run.
    w.silent();
}
#[test]
fn a_taken_state_is_unknown_the_second_time() {
    let mut states = States::new(1);
    let state = issue(&mut states, &RANDOM, Time::ZERO);
    assert_eq!(states.take(FORGE, Purpose::Convert, &state, Time::ZERO), Ok(()));
    assert_eq!(states.take(FORGE, Purpose::Convert, &state, Time::ZERO), Err(unknown(FORGE, Purpose::Convert)));
}
#[test]
#[expect(clippy::disallowed_macros, reason = "test checks state failure Debug for secret bytes")]
fn an_unknown_expired_or_wrong_binding_state_is_unknown() {
    for (forge, purpose, state, now) in [
        (FORGE, Purpose::Convert, SECRET.as_slice(), Time::ZERO),
        (FORGE, Purpose::Convert, TEXT, Time::ZERO.checked_add(LIFE).expect("finite expiry")),
        (OTHER, Purpose::Convert, TEXT, Time::ZERO),
        (FORGE, Purpose::Accept, TEXT, Time::ZERO),
    ] {
        let mut states = States::new(1);
        issue(&mut states, &RANDOM, Time::ZERO);
        let failure = states.take(forge, purpose, state, now).expect_err("unknown binding");
        assert_eq!(failure, unknown(forge, purpose));
        diagnostic(&format!("{failure:?}"));
    }
    for (forge, purpose) in [(OTHER, Purpose::Convert), (FORGE, Purpose::Accept)] {
        let mut states = States::new(1);
        let state = issue(&mut states, &RANDOM, Time::ZERO);
        assert_eq!(states.take(forge, purpose, &state, Time::ZERO), Err(unknown(forge, purpose)));
        assert_eq!(states.take(FORGE, Purpose::Convert, &state, Time::ZERO), Ok(()));
    }
    let mut states = States::new(1);
    let state = states.issue(FORGE, Purpose::Accept, &RANDOM, Time::ZERO, LIFE, b"install").expect("accept state");
    assert_eq!(states.take(FORGE, Purpose::Convert, &state, Time::ZERO), Err(unknown(FORGE, Purpose::Convert)));
    assert_eq!(states.take(FORGE, Purpose::Accept, &state, Time::ZERO), Ok(()));
}
#[test]
fn a_state_lives_until_one_nanosecond_before_expiry() {
    let mut states = States::new(1);
    let birth = Time::from_nanos(1);
    let state = issue(&mut states, &RANDOM, birth);
    assert_eq!(states.take(FORGE, Purpose::Convert, &state, Time::from_nanos(LIFE.as_nanos())), Ok(()));
}
#[test]
fn a_full_table_evicts_the_oldest_issued_state() {
    for second_birth in [Time::ZERO, Time::from_nanos(1)] {
        let mut states = States::new(2);
        let first = issue(&mut states, &[1; 16], Time::ZERO);
        let second = issue(&mut states, &[2; 16], second_birth);
        let third =
            states.issue(OTHER, Purpose::Accept, &[3; 16], second_birth, LIFE, b"install").expect("third issuance");
        assert_eq!(states.take(FORGE, Purpose::Convert, &first, second_birth), Err(unknown(FORGE, Purpose::Convert)));
        assert_eq!(states.take(FORGE, Purpose::Convert, &second, second_birth), Ok(()));
        assert_eq!(states.take(OTHER, Purpose::Accept, &third, second_birth), Ok(()));
    }
}
#[test]
fn a_state_collision_preserves_the_first_binding() {
    let mut states = States::new(1);
    let first = issue(&mut states, &RANDOM, Time::ZERO);
    assert_eq!(
        states.issue(OTHER, Purpose::Accept, &RANDOM, Time::ZERO, LIFE, b"install"),
        Err(Failure { forge: OTHER, step: Stage::State, subject: b"install".as_slice().into(), why: Why::Unexpected })
    );
    assert_eq!(states.take(OTHER, Purpose::Accept, &first, Time::ZERO), Err(unknown(OTHER, Purpose::Accept)));
    assert_eq!(states.take(FORGE, Purpose::Convert, &first, Time::ZERO), Ok(()));
    let mut w = World::new();
    let mut rng = fixtures::crypto();
    let mut random = [0_u8; 16];
    rng.fill(&mut random);
    let first = states.issue(OTHER, Purpose::Accept, &random, Time::ZERO, LIFE, b"install").expect("fresh binding");
    w.host.states = states;
    w.run(manifest(FORGE));
    assert_eq!(
        w.one(),
        Event::Stop {
            run: RUN,
            failure: Failure {
                forge: FORGE,
                step: Stage::State,
                subject: b"manifest".as_slice().into(),
                why: Why::Unexpected,
            }
        }
    );
    assert_eq!(w.host.states.take(OTHER, Purpose::Accept, &first, Time::ZERO), Ok(()));
    w.silent();
}
#[test]
fn expiry_overflow_leaves_existing_states_usable() {
    let mut states = States::new(1);
    let now = Time::from_nanos(u64::MAX.checked_sub(1).expect("one nanosecond before the end"));
    let first =
        states.issue(FORGE, Purpose::Convert, &RANDOM, now, Duration::from_nanos(1), b"manifest").expect("expiry fits");
    assert_eq!(
        states.issue(OTHER, Purpose::Accept, &[2; 16], now, Duration::from_nanos(2), b"install"),
        Err(Failure { forge: OTHER, step: Stage::State, subject: b"install".as_slice().into(), why: Why::Unexpected })
    );
    assert_eq!(states.take(FORGE, Purpose::Convert, &first, now), Ok(()));
}
#[test]
fn expired_entries_are_pruned_before_eviction_or_collision_checks() {
    let mut states = States::new(2);
    let first = issue(&mut states, &[1; 16], Time::ZERO);
    states
        .issue(FORGE, Purpose::Convert, &RANDOM, Time::ZERO, Duration::from_nanos(1), b"manifest")
        .expect("short state");
    let now = Time::from_nanos(1);
    let reused = issue(&mut states, &RANDOM, now);
    assert_eq!(states.take(FORGE, Purpose::Convert, &first, now), Ok(()));
    assert_eq!(states.take(FORGE, Purpose::Convert, &reused, now), Ok(()));
}
#[test]
fn a_restart_forgets_every_state() {
    let mut w = World::new();
    let state = w.page(FORGE);
    let mut restarted = World::new();
    assert_eq!(
        restarted.host.states.take(FORGE, Purpose::Convert, &state, Time::ZERO),
        Err(unknown(FORGE, Purpose::Convert))
    );
    w.down(Request::Close);
    assert_eq!(w.one(), Event::Closed);
    assert_eq!(w.host.states.take(FORGE, Purpose::Convert, &state, Time::ZERO), Err(unknown(FORGE, Purpose::Convert)));
    w.silent();
    w.down(Request::Close);
    assert!(w.up.is_empty());
}
#[test]
fn the_host_rejects_zero_states() {
    let mut limits = fixtures::limits();
    limits.states = 0;
    assert_eq!(Host::new(&limits, fixtures::github_settings(), fixtures::crypto()).err(), Some(Unusable::Zero));
}
#[test]
fn the_host_runs_a_manifest_without_io_or_items() {
    let mut w = World::new();
    let state = w.page(FORGE);
    assert_eq!(w.host.states.take(FORGE, Purpose::Convert, &state, Time::ZERO), Ok(()));
    w.silent();
    w.run(manifest(1)); // The Forgejo fixture refuses the GitHub-only plan.
    assert_eq!(
        w.one(),
        Event::Stop {
            run: RUN,
            failure: Failure {
                forge: 1,
                step: Stage::Admin,
                subject: b"kit-admin".as_slice().into(),
                why: Why::Unexpected,
            }
        }
    );
    w.silent();
}
#[test]
fn a_manifest_uses_the_normal_write_slot_and_admission_precedes_issuance() {
    let mut w = World::new();
    let mut run = w.admit();
    assert!(run.writes);
    assert_eq!(run.pending, Some(reconcile::Need::Draw(3)));
    let first = run.issued_state.clone().expect("issued state").1;
    let mut expected_rng = w.host.crypto.clone();
    run.pending = None; // Pause the first run between admission and its name draw.
    let previous = w.host.runs.insert(Token::new(2), run).expect("one live write");
    assert!(previous.is_none());
    w.run(manifest(FORGE));
    assert_eq!(
        w.one(),
        Event::Stop {
            run: RUN,
            failure: Failure { forge: FORGE, step: Stage::App, subject: b"manifest".as_slice().into(), why: Why::Busy }
        }
    );
    w.host.runs.get_mut(&Token::new(2)).expect("live manifest").pending = Some(reconcile::Need::Draw(3));
    w.host.fire(&w.env, &mut w.up, &mut w.io);
    match w.one() {
        Event::Done { run, outcome: Outcome::Manifest { state, .. } } => {
            assert_eq!(run, Token::new(2));
            assert_eq!(state, first);
        }
        Event::Need { .. } | Event::Done { .. } | Event::Stop { .. } | Event::Closed => panic!("earlier manifest Done"),
    }
    assert!(w.io.is_empty());
    let mut expected = [0_u8; 3];
    expected_rng.fill(&mut expected);
    let mut following = [0_u8; 16];
    expected_rng.fill(&mut following);
    let mut table = States::new(1);
    let next = issue(&mut table, &following, Time::ZERO);
    assert_eq!(w.page(FORGE), next);
    assert_eq!(w.host.states.take(FORGE, Purpose::Convert, &first, Time::ZERO), Ok(()));
    w.silent();
}
#[test]
fn a_failed_manifest_drops_its_state() {
    for (target, page_bytes) in [(Target::Org(b"bad/login".as_slice().into()), 4096), (Target::Personal, 1)] {
        let mut settings = fixtures::github_settings();
        match &mut settings.forges.get_mut(1).expect("GitHub fixture").kind {
            Kind::GitHub(given) => given.limits.page_bytes = page_bytes,
            Kind::Forgejo(_) => panic!("GitHub fixture"),
        }
        let mut w = World::with(fixtures::limits(), settings);
        let mut rng = fixtures::crypto();
        let mut random = [0_u8; 16];
        rng.fill(&mut random);
        let mut table = States::new(1);
        let state = issue(&mut table, &random, Time::ZERO);
        w.run(Plan::Manifest { forge: FORGE, target });
        assert_eq!(
            w.one(),
            Event::Stop {
                run: RUN,
                failure: Failure {
                    forge: FORGE,
                    step: Stage::App,
                    subject: b"manifest".as_slice().into(),
                    why: Why::Unexpected,
                }
            }
        );
        assert_eq!(
            w.host.states.take(FORGE, Purpose::Convert, &state, Time::ZERO),
            Err(unknown(FORGE, Purpose::Convert))
        );
        w.silent();
    }
}
#[test]
fn the_host_caps_states_across_forges_and_uses_each_state_life() {
    let mut limits = fixtures::limits();
    limits.states = 2;
    let mut settings = fixtures::github_settings();
    let mut other = github_fixtures::given();
    other.forge = OTHER;
    other.limits.state_life = Duration::from_nanos(2);
    settings.forges.get_mut(0).expect("second GitHub forge").kind = Kind::GitHub(other);
    let mut w = World::with(limits, settings);
    let first = w.page(FORGE);
    let second = w.page(OTHER);
    let third = w.page(FORGE);
    assert_eq!(w.host.states.take(FORGE, Purpose::Convert, &first, Time::ZERO), Err(unknown(FORGE, Purpose::Convert)));
    w.env.now = Time::from_nanos(2);
    assert_eq!(w.host.states.take(OTHER, Purpose::Convert, &second, w.env.now), Err(unknown(OTHER, Purpose::Convert)));
    assert_eq!(w.host.states.take(FORGE, Purpose::Convert, &third, w.env.now), Ok(()));
    w.silent();
}
#[test]
fn a_halted_manifest_drops_its_state_and_its_machine_stays_silent() {
    let mut w = World::new();
    let mut run = w.admit();
    let state = run.issued_state.clone().expect("issued state").1;
    run.halt(Why::Down, &mut w.host.plan_out);
    for event in [
        reconcile::Event::Begin,
        reconcile::Event::Written,
        reconcile::Event::Read(Some(secret(SECRET))),
        reconcile::Event::Reply(reconcile::Reply::Down),
        reconcile::Event::Drawn(secret(SECRET)),
    ] {
        let mut out = Queue::uncapped();
        run.machine.step(event, &mut out);
        assert!(out.is_empty());
    }
    let previous = w.host.runs.insert(RUN, run).expect("one live run");
    assert!(previous.is_none());
    w.host.fire(&w.env, &mut w.up, &mut w.io);
    assert_eq!(
        w.one(),
        Event::Stop {
            run: RUN,
            failure: Failure { forge: FORGE, step: Stage::App, subject: b"manifest".as_slice().into(), why: Why::Down }
        }
    );
    assert_eq!(w.host.states.take(FORGE, Purpose::Convert, &state, Time::ZERO), Err(unknown(FORGE, Purpose::Convert)));
    w.silent();
}
#[test]
#[expect(clippy::disallowed_macros, reason = "test checks state table Debug against raw draw bytes")]
fn the_state_table_debug_keeps_no_raw_random_bytes() {
    let mut states = States::new(1);
    issue(&mut states, SECRET, Time::ZERO);
    diagnostic(&format!("{states:?}"));
}
