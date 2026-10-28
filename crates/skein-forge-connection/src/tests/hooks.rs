use super::fixtures;
use crate::{
    Component, Event, Limits, Request, Shared, Unusable,
    limits::{self, Stacked},
};
use skein_lib::{Env, Queue, Secret, Time, Token, Wall};

const LISTENER: Token = Token::new(90);
// Repeat the failure to check that it is announced once.
const LISTENER_FAILURES: u32 = 2;
const BOUND: skein_io::Addr =
    skein_io::Addr::Ip(core::net::SocketAddr::V4(core::net::SocketAddrV4::new(core::net::Ipv4Addr::LOCALHOST, 54321)));

struct Listening {
    component: Component,
    env: Env<Limits>,
    up: Queue<Event>,
    io: Queue<skein_io::Request>,
    owner: Token,
}
impl Listening {
    fn new() -> Self {
        let requested = skein_io::Addr::Ip(([127, 0, 0, 1], 0).into());
        let mut settings = fixtures::settings();
        settings.forges.clear();
        settings.webhooks = Some(crate::hooks::Webhooks { listener: requested });
        let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: fixtures::limits() };
        let mut component = Component::new(settings, &env.limits, &fixtures::shared()).expect("listener limits");
        let mut up = Queue::uncapped();
        let mut io = Queue::uncapped();
        component.begin(&env, &mut up, &mut io);
        assert!(up.is_empty());
        let owner = match io.pop().expect("listen request") {
            skein_io::Request::Listen { owner, addr } => {
                assert_eq!(addr, requested);
                owner
            }
            other @ (skein_io::Request::Connect { .. }
            | skein_io::Request::Bind { .. }
            | skein_io::Request::Reject { .. }
            | skein_io::Request::Stream { .. }
            | skein_io::Request::Output { .. }
            | skein_io::Request::Spawn { .. }
            | skein_io::Request::Signal { .. }
            | skein_io::Request::Usage { .. }
            | skein_io::Request::Window { .. }
            | skein_io::Request::Close { .. }
            | skein_io::Request::Abort { .. }) => panic!("listen request: {other:?}"),
        };
        assert!(io.is_empty());
        Self { component, env, up, io, owner }
    }
    fn bound(&mut self) {
        self.component.up(
            &self.env,
            skein_io::Event::Listening { owner: self.owner, listener: LISTENER, addr: BOUND },
            &mut self.up,
            &mut self.io,
        );
    }
}

#[test]
fn the_webhooks_listener_says_its_bound_address_once() {
    let mut h = Listening::new();
    h.bound();
    match h.up.pop() {
        Some(Event::Listening { addr }) => assert_eq!(addr, BOUND),
        other => panic!("bound address: {other:?}"),
    }
    assert!(h.up.is_empty());
    assert!(h.io.is_empty());
    h.component.fire(&h.env, &mut h.up, &mut h.io);
    assert!(h.up.is_empty(), "the bind is announced once");
    assert!(h.io.is_empty());
}

#[test]
fn a_bound_webhooks_listener_still_says_listener_lost_once() {
    let mut h = Listening::new();
    h.bound();
    match h.up.pop() {
        Some(Event::Listening { addr }) => assert_eq!(addr, BOUND),
        other => panic!("bound address: {other:?}"),
    }
    for _ in 0..LISTENER_FAILURES {
        h.component.up(
            &h.env,
            skein_io::Event::Failed { owner: h.owner, error: skein_io::Error::Refused },
            &mut h.up,
            &mut h.io,
        );
    }
    match h.up.pop() {
        Some(Event::ListenerLost) => {}
        other => panic!("listener lost: {other:?}"),
    }
    assert!(h.up.is_empty(), "the failure is announced once");
    match h.io.pop() {
        Some(skein_io::Request::Close { entity }) => assert_eq!(entity, LISTENER),
        other => panic!("close listener: {other:?}"),
    }
    assert!(h.io.is_empty());
}

#[test]
fn a_webhooks_listener_bound_while_closing_is_closed_without_an_announcement() {
    let mut h = Listening::new();
    h.component.down(&h.env, Request::Close, &mut h.up, &mut h.io);
    h.bound();
    assert!(h.up.is_empty());
    match h.io.pop() {
        Some(skein_io::Request::Close { entity }) => assert_eq!(entity, LISTENER),
        other => panic!("close listener: {other:?}"),
    }
    assert!(h.io.is_empty());
}

#[test]
fn a_hook_secret_past_its_bound_is_unusable_naming_the_forge() {
    let mut settings = fixtures::hook_settings();
    settings.forges.get_mut(0).expect("forge").access.as_mut().expect("fixture access").webhook_secret =
        Some(Secret::new(Box::new([7; 129])));
    assert_eq!(limits::check(&fixtures::limits(), &fixtures::shared(), &settings), Err(Unusable::Secret { forge: 1 }));
}
#[test]
fn a_hook_secret_bound_below_32_is_unusable() {
    let limits = Limits { hook_secret_bytes: 31, ..fixtures::limits() };
    assert_eq!(
        limits::check(&limits, &fixtures::shared(), &fixtures::settings()),
        Err(Unusable::SecretBytes { bytes: 31 })
    );
}
#[test]
fn a_hook_limit_of_zero_is_unusable() {
    let limits = Limits { hooks: 0, ..fixtures::limits() };
    assert_eq!(limits::check(&limits, &fixtures::shared(), &fixtures::settings()), Err(Unusable::Zero));
}
#[test]
fn a_hook_head_limit_of_zero_is_unusable() {
    let limits = Limits { hook_head_bytes: 0, ..fixtures::limits() };
    assert_eq!(limits::check(&limits, &fixtures::shared(), &fixtures::settings()), Err(Unusable::Zero));
}
#[test]
fn a_hook_body_limit_of_zero_is_unusable() {
    let limits = Limits { hook_body_bytes: 0, ..fixtures::limits() };
    assert_eq!(limits::check(&limits, &fixtures::shared(), &fixtures::settings()), Err(Unusable::Zero));
}
#[test]
fn a_hook_secret_limit_of_zero_is_unusable() {
    let limits = Limits { hook_secret_bytes: 0, ..fixtures::limits() };
    assert_eq!(limits::check(&limits, &fixtures::shared(), &fixtures::settings()), Err(Unusable::Zero));
}
#[test]
fn a_hook_deadline_of_zero_is_unusable() {
    let limits = Limits { hook_deadline: skein_lib::Duration::ZERO, ..fixtures::limits() };
    assert_eq!(limits::check(&limits, &fixtures::shared(), &fixtures::settings()), Err(Unusable::Zero));
}
#[test]
fn a_hook_head_past_what_io_holds_is_unusable_as_a_stacked_server() {
    // Only intake differs from the shipped bound: the webhook head cannot fit it.
    let shared = Shared { io: skein_io::Limits { intake: 8191, ..fixtures::shared().io }, ..fixtures::shared() };
    assert_eq!(
        limits::check(&fixtures::limits(), &shared, &fixtures::settings()),
        Err(Unusable::Stack { stacked: Stacked::ServerOverIo, demand: 8192, cap: 8191 })
    );
}
#[test]
fn the_worst_case_grows_with_the_webhook_connections() {
    let limits = fixtures::limits();
    let one = limits::worst_case(
        &Limits { hooks: 1, ..limits },
        &fixtures::shared(),
        limits::Forges::of(&fixtures::hook_settings()).expect("forge counts"),
        crate::settings::owned_bytes(&fixtures::hook_settings()).expect("owner bytes"),
    )
    .expect("one");
    let two = limits::worst_case(
        &Limits { hooks: 2, ..limits },
        &fixtures::shared(),
        limits::Forges::of(&fixtures::hook_settings()).expect("forge counts"),
        crate::settings::owned_bytes(&fixtures::hook_settings()).expect("owner bytes"),
    )
    .expect("two");
    assert!(two > one);
}
#[test]
fn each_delivery_becomes_the_hint_forge_md_5_names() {
    for (delivery, change, key) in hint_rows() {
        assert_eq!(
            skein_forge::forgejo::plans::hint::hint(1, fixtures::WORD, &delivery),
            Some(skein_forge::api::Hint {
                repository: skein_forge::api::Repository { forge: 1, repository: 2 },
                change,
                key,
            }),
            "{delivery:?}"
        );
    }
}
type HintRow = (skein_forge::forgejo::delivery::Delivery, skein_forge::api::Change, Option<Box<[u8]>>);
fn hint_rows() -> [HintRow; 7] {
    use fixtures::{HOOK_HEAD as HEAD, HOOK_NUMBER as NUMBER, HOOK_REPOSITORY as REPOSITORY};
    use skein_forge::api::Change;
    use skein_forge::forgejo::{
        delivery::{Delivery, JobEnd},
        request::ReviewEvent,
    };
    [
        (
            Delivery::Issue { repository: REPOSITORY, number: NUMBER, body: fixtures::hook_body() },
            Change::Item(NUMBER),
            Some(Box::from([1, 2])),
        ),
        (
            Delivery::Comment { repository: REPOSITORY, number: NUMBER, body: fixtures::hook_body() },
            Change::Item(NUMBER),
            Some(Box::from([1, 2])),
        ),
        (
            Delivery::Review {
                repository: REPOSITORY,
                number: NUMBER,
                body: fixtures::hook_body(),
                event: ReviewEvent::Approved,
            },
            Change::Item(NUMBER),
            Some(Box::from([1, 2])),
        ),
        (Delivery::Pull { repository: REPOSITORY, number: NUMBER }, Change::Item(NUMBER), None),
        (
            Delivery::Push {
                repository: REPOSITORY,
                reference: Box::from(b"refs/heads/main".as_slice()),
                after: fixtures::hook_id(),
                pusher: 2,
            },
            Change::BranchMoved { branch: Box::from(b"main".as_slice()), head: HEAD, actor: 2 },
            None,
        ),
        (fixtures::hook_ref(b"refs/heads/main"), Change::Branch(Box::from(b"main".as_slice())), None),
        (
            Delivery::Job { repository: REPOSITORY, commit: fixtures::hook_id(), end: JobEnd::Success },
            Change::Commit(HEAD),
            None,
        ),
    ]
}
#[test]
fn a_tag_delivery_hints_nothing() {
    let delivery = fixtures::hook_ref(b"refs/tags/version");
    assert_eq!(skein_forge::forgejo::plans::hint::hint(1, fixtures::WORD, &delivery), None);
}
#[test]
fn a_repository_id_past_u32_hints_nothing() {
    let delivery =
        skein_forge::forgejo::delivery::Delivery::Pull { repository: u64::MAX, number: fixtures::HOOK_NUMBER };
    assert_eq!(skein_forge::forgejo::plans::hint::hint(1, fixtures::WORD, &delivery), None);
}

#[test]
fn signature_shape_is_refused_at_the_head() {
    // SHA-256 signatures have 64 lowercase hex digits (forge-connection.md 6.2).
    const DIGITS: usize = 64;
    let correct = fixtures::hook_signature(fixtures::PULL_PAYLOAD);
    let mut upper = correct;
    for byte in &mut upper {
        *byte = byte.to_ascii_uppercase();
    }
    let mut nonhex = correct;
    nonhex[0] = b'g';
    let mut wrong = correct;
    wrong[0] = if wrong[0] == b'0' { b'1' } else { b'0' };
    for (name, signature, head_status, demands, final_status, hints, closed) in [
        ("short", correct.get(..DIGITS - 1).expect("short signature"), Some(403), 0, Some(403), 0, true),
        ("long", [b'a'; DIGITS + 1].as_slice(), Some(403), 0, Some(403), 0, true),
        ("uppercase", upper.as_slice(), Some(403), 0, Some(403), 0, true),
        ("nonhex", nonhex.as_slice(), Some(403), 0, Some(403), 0, true),
        ("correct", correct.as_slice(), None, 1, Some(204), 1, false),
        ("wrong digest", wrong.as_slice(), None, 1, Some(403), 0, true),
    ] {
        let mut h = fixtures::hook();
        h.write(&fixtures::hook_head(fixtures::PULL_PAYLOAD, Some(b"pull_request"), signature));
        assert_eq!((h.status(), h.body_demands(), h.hints()), (head_status, demands, 0), "{name}: head only");
        h.write(fixtures::PULL_PAYLOAD);
        assert_eq!((h.status(), h.hints(), h.closed()), (final_status, hints, closed), "{name}: terminal");
    }
}
#[test]
fn a_verified_delivery_without_an_event_is_answered_204_without_a_hint() {
    let mut h = fixtures::hook();
    h.write(&fixtures::signed_head(b"{}", None));
    assert_eq!(h.status(), None);
    h.write(b"{}");
    assert_eq!((h.status(), h.hints(), h.closed()), (Some(204), 0, false));
}
