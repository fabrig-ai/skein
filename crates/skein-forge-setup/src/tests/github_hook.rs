use super::{fixtures::secret, github_fixtures as fixtures};
use crate::{
    call::Auth,
    github::{
        app::Kept,
        checks::{self, Lacking},
        hook::{self, Hook, Progress},
    },
    reconcile::{Event, Failure, Need, Outcome, Reply, Stage, Why},
};
use core::num::NonZeroU64;
use skein_forge::github::{
    document::{Document, Hook as Config},
    request::Request,
};
use skein_lib::{Queue, bytes};

const APP: u64 = 19;
const SLUG: &[u8] = b"kit-app";
const URL: &[u8] = b"https://kit-app.invalid/";
const WRONG: &[u8] = b"https://elsewhere.test/";
const KEY: &[u8] = b"sentinel-hook-app-key";

fn new(slug: &[u8]) -> Hook {
    with_repair(slug, true)
}
fn with_repair(slug: &[u8], repair: bool) -> Hook {
    Hook::new(fixtures::given().forge, Kept::new(NonZeroU64::new(APP).expect("nonzero App"), secret(KEY)), slug, repair)
}
fn config(url: &[u8]) -> Event {
    Event::Reply(Reply::GithubDocument(Document::HookConfig(Config { url: url.into() })))
}
fn go(machine: &mut Hook, event: Event) -> Progress {
    let mut out = Queue::uncapped();
    hook::step(machine, event, &mut out);
    assert_eq!(out.len(), 1, "one pending need or internal completion");
    out.pop().expect("one progress")
}
fn at(phase: usize) -> Hook {
    let mut machine = new(SLUG);
    for event in [Event::Begin, config(WRONG)].into_iter().take(phase) {
        go(&mut machine, event);
    }
    machine
}
fn send(request: Request) -> Progress {
    Progress::Pending(Need::Send { request: request.into(), auth: Auth::App { id: APP, key: secret(KEY) } })
}
fn failure(subject: &[u8], why: Why) -> Progress {
    Progress::Pending(Need::Stop(Failure {
        forge: fixtures::given().forge,
        step: Stage::App,
        subject: subject.into(),
        why,
    }))
}
fn silent(machine: &mut Hook) {
    for event in [
        Event::Begin,
        Event::Read(None),
        Event::Read(Some(secret(KEY))),
        config(URL),
        Event::Reply(Reply::Unsigned),
        Event::Written,
        Event::Drawn(secret(KEY)),
    ] {
        let mut out = Queue::uncapped();
        hook::step(machine, event, &mut out);
        assert!(out.is_empty(), "terminal child is silent");
    }
}

#[test]
fn a_correct_hook_is_read_without_a_patch() {
    let mut machine = at(0);
    assert_eq!(go(&mut machine, Event::Begin), send(Request::HookConfig));
    assert_eq!(go(&mut machine, config(URL)), Progress::Complete);
    silent(&mut machine);
}

#[test]
fn a_read_only_hook_completes_whatever_its_url() {
    for url in [URL, WRONG, b""] {
        let mut machine = with_repair(SLUG, false);
        assert_eq!(go(&mut machine, Event::Begin), send(Request::HookConfig));
        assert_eq!(go(&mut machine, config(url)), Progress::Complete);
        silent(&mut machine);
    }
}

#[test]
fn a_wrong_or_absent_hook_is_patched_to_the_slug_invalid_host() {
    // The admitted Hook document represents an absent URL as empty bytes.
    for url in [WRONG, b"", b"https://kit-app.invalid", b"https://Kit-app.invalid/"] {
        let mut machine = at(1);
        assert_eq!(go(&mut machine, config(url)), send(Request::SetHook { url: URL.into() }));
        assert_eq!(go(&mut machine, config(URL)), Progress::Complete);
        silent(&mut machine);
    }
    // Slugs at the admitted name bound and shorter names use the same URL rule.
    for (slug, expected) in [
        (b"x".as_slice(), b"https://x.invalid/".as_slice()),
        (b"trailing-", b"https://trailing-.invalid/"),
        (b"abcdefghijklmnopqrstuvwxyzabcdefgh", b"https://abcdefghijklmnopqrstuvwxyzabcdefgh.invalid/"),
    ] {
        crate::item::app_slug(slug, fixtures::limits().name_bytes).expect("admitted slug");
        let mut machine = new(slug);
        go(&mut machine, Event::Begin);
        assert_eq!(go(&mut machine, config(WRONG)), send(Request::SetHook { url: expected.into() }));
        assert_eq!(go(&mut machine, config(expected)), Progress::Complete);
        silent(&mut machine);
    }
}

// A concrete parent fixture demonstrates the child's forwarding boundary.
struct Parent {
    hook: Hook,
    lacking: Option<Box<[Lacking]>>,
    continued: bool,
}
fn parent_step(parent: &mut Parent, event: Event, out: &mut Queue<Need>) {
    let mut child = Queue::uncapped();
    hook::step(&mut parent.hook, event, &mut child);
    match child.pop() {
        Some(Progress::Pending(need)) => out.push(need),
        Some(Progress::Complete) => {
            parent.continued = true;
            out.push(Need::Done(Outcome::Lacking { lacking: parent.lacking.take().expect("parent continues once") }));
        }
        None => (),
    }
}

#[test]
fn a_hook_patch_must_answer_before_the_parent_continues() {
    let mut app = fixtures::app();
    app.permissions = Box::new([]);
    let lacking = checks::lacks(&app);
    let expected = lacking.clone();
    let mut parent = Parent { hook: at(0), lacking: Some(lacking), continued: false };
    let mut out = Queue::uncapped();
    for (event, request) in [(Event::Begin, Request::HookConfig), (config(WRONG), Request::SetHook { url: URL.into() })]
    {
        parent_step(&mut parent, event, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(Some(Progress::Pending(out.pop().expect("forwarded need"))), Some(send(request)));
        assert!(!parent.continued);
        assert_eq!(parent.lacking.as_deref(), Some(expected.as_ref()));
        assert!(out.is_empty(), "withholding the patch reply leaves no completion");
    }
    parent_step(&mut parent, config(URL), &mut out);
    assert!(parent.continued);
    assert_eq!(out.len(), 1);
    assert_eq!(out.pop(), Some(Need::Done(Outcome::Lacking { lacking: expected })));
    parent_step(&mut parent, config(URL), &mut out);
    assert!(out.is_empty());
}

#[test]
fn a_hook_names_refused() {
    for phase in [1, 2] {
        for status in [200, 204, 401, 403, 404, 429, 500] {
            let mut machine = at(phase);
            assert_eq!(go(&mut machine, Event::Reply(Reply::Status(status))), failure(SLUG, Why::Refused(status)));
            silent(&mut machine);
        }
    }
}

#[test]
fn a_hook_names_down() {
    for phase in [1, 2] {
        let mut machine = at(phase);
        assert_eq!(go(&mut machine, Event::Reply(Reply::Down)), failure(SLUG, Why::Down));
        silent(&mut machine);
    }
}

#[test]
fn a_hook_names_unsigned() {
    for phase in [1, 2] {
        let mut machine = at(phase);
        assert_eq!(go(&mut machine, Event::Reply(Reply::Unsigned)), failure(b"key", Why::KeyForm));
        silent(&mut machine);
    }
}

#[test]
fn a_hook_names_wrong_document() {
    for phase in [1, 2] {
        for reply in [
            Reply::Empty,
            Reply::GithubDocument(Document::Done),
            Reply::GithubDocument(Document::App(fixtures::app())),
            Reply::GithubRows(Box::new([])),
            Reply::Rows(Box::new([])),
            Reply::Document(skein_forge::forgejo::document::Document::RepoSearch { ok: true }),
        ] {
            let mut machine = at(phase);
            assert_eq!(go(&mut machine, Event::Reply(reply)), failure(SLUG, Why::Unexpected));
            silent(&mut machine);
        }
    }
}

#[test]
fn hook_steps_reject_unasked_events_and_end_once() {
    let bad_begin =
        [Event::Read(None), Event::Read(Some(secret(KEY))), config(URL), Event::Written, Event::Drawn(secret(KEY))];
    let bad_reply =
        [Event::Begin, Event::Read(None), Event::Read(Some(secret(KEY))), Event::Written, Event::Drawn(secret(KEY))];
    for (phase, events) in [(0, bad_begin), (1, bad_reply.clone()), (2, bad_reply)] {
        for event in events {
            let mut machine = at(phase);
            assert_eq!(go(&mut machine, event), failure(SLUG, Why::Unexpected));
            silent(&mut machine);
        }
    }
}

#[test]
#[expect(clippy::disallowed_macros, reason = "check secret bytes in diagnostic text and numeric Debug forms")]
fn a_lacking_outcome_and_hook_failure_contain_no_secret() {
    let app = fixtures::app();
    let outcome = Outcome::Lacking { lacking: checks::lacks(&app) };
    let mut app = app;
    app.permissions = Box::new([]);
    app.events = Box::new([]);
    let lacking = Outcome::Lacking { lacking: checks::lacks(&app) };
    for phase in [0, 1, 2] {
        let mut machine = at(phase);
        let live = format!("{machine:?}");
        assert_eq!(live.contains("Secret"), phase < 2);
        let stopped = go(&mut machine, Event::Read(Some(secret(KEY))));
        let ended = format!("{machine:?} {stopped:?}");
        assert!(!ended.contains("Secret"));
        let mut correct = at(1);
        let complete = go(&mut correct, config(URL));
        let done = format!("{correct:?} {complete:?} {outcome:?} {lacking:?}");
        assert!(!done.contains("Secret"));
        // Inspect an outgoing App auth too: it retains Secret's redaction.
        let need = format!("{:?}", send(Request::SetHook { url: URL.into() }));
        for debug in [live, ended, done, need] {
            for value in [KEY, fixtures::DER, b"sentinel-token", b"sentinel-webhook", b"sentinel-pasted-key"] {
                assert!(bytes::find(debug.as_bytes(), value).is_none());
                assert!(!debug.contains(&format!("{value:?}")));
                assert!(!debug.contains(&format!("{value:x?}")));
                assert!(!debug.contains(&format!("{value:X?}")));
            }
        }
        silent(&mut machine);
    }
}
