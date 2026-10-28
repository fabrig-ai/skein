use super::{fixtures::secret, github_fixtures as fixtures};
use crate::{
    github::{
        given::{Place, Refused},
        install::{self, Install},
    },
    item::Item,
    reconcile::{Event, Failure, Need, Outcome, Reply, Stage, Why},
};
use skein_lib::{Queue, bytes};

pub(super) const STATE: &[u8] = b"0123456789abcdef0123456789abcdef";
const LINK: &[u8] = b"https://github.test/apps/kit-app/installations/new?state=0123456789abcdef0123456789abcdef";
const SECRET: &[u8] = b"install-secret-sentinel";

#[expect(clippy::disallowed_macros, reason = "test checks diagnostics for secret byte spellings")]
fn go(machine: &mut Install, event: Event) -> Need {
    let mut out = Queue::uncapped();
    install::step(machine, event, &mut out);
    assert_eq!(out.len(), 1);
    let need = out.pop().expect("one need");
    let text = format!("{machine:?} {need:?}");
    assert!(bytes::find(text.as_bytes(), SECRET).is_none());
    assert!(!text.contains(&format!("{SECRET:?}")));
    need
}
fn stop(subject: &[u8], why: Why) -> Need {
    Need::Stop(Failure { forge: 4, step: Stage::App, subject: subject.into(), why })
}
fn events() -> [Event; 5] {
    [
        Event::Begin,
        Event::Read(Some(secret(SECRET))),
        Event::Reply(Reply::Down),
        Event::Written,
        Event::Drawn(secret(SECRET)),
    ]
}
fn silent(machine: &mut Install) {
    for event in events() {
        let mut out = Queue::uncapped();
        install::step(machine, event, &mut out);
        assert!(out.is_empty());
    }
}
#[test]
fn an_install_reads_only_the_slug_and_returns_its_state_link() {
    let mut given = fixtures::given();
    given.limits.page_bytes = u32::try_from(LINK.len()).expect("link size");
    let mut machine = Install::new(&given, STATE).expect("valid inputs");
    assert_eq!(go(&mut machine, Event::Begin), Need::Read(Item::AppSlug));
    assert_eq!(
        go(&mut machine, Event::Read(Some(secret(b"kit-app")))),
        Need::Done(Outcome::Install { state: STATE.into(), url: LINK.into() })
    );
    silent(&mut machine);
}
#[test]
fn a_missing_slug_or_oversized_link_stops_without_sending() {
    for (slug, bound, subject, why) in [
        (None, 4096, b"slug".as_slice(), Why::Missing),
        (Some(secret(b"")), 4096, b"slug", Why::Missing),
        (Some(secret(b"bad/slug")), 4096, b"slug", Why::Missing),
        (Some(secret(b"kit-app")), u32::try_from(LINK.len() - 1).expect("link size"), b"install", Why::Unexpected),
    ] {
        let mut given = fixtures::given();
        given.limits.page_bytes = bound;
        let mut machine = Install::new(&given, STATE).expect("valid given");
        assert_eq!(go(&mut machine, Event::Begin), Need::Read(Item::AppSlug));
        assert_eq!(go(&mut machine, Event::Read(slug)), stop(subject, why));
        silent(&mut machine);
    }
    for state in [b"".as_slice(), b"0123456789abcdef0123456789abcdeF", b"0123456789abcdef0123456789abcdef0"] {
        assert_eq!(Install::new(&fixtures::given(), state).err(), Some(Refused { place: Place::State, other: None }));
    }
}
pub(super) fn rejects_unasked_events_and_ignores_ended_events() {
    for phase in 0_u8..2 {
        for event in events() {
            if (phase == 0 && event == Event::Begin)
                || (phase == 1
                    && match event {
                        Event::Read(_) => true,
                        Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => false,
                    })
            {
                continue;
            }
            let mut machine = Install::new(&fixtures::given(), STATE).expect("valid inputs");
            if phase == 1 {
                go(&mut machine, Event::Begin);
            }
            assert_eq!(go(&mut machine, event), stop(b"slug", Why::Unexpected));
            silent(&mut machine);
        }
    }
}
