//! Ready-time eligibility and private jobs on the reserved connection.
use super::{
    fixtures::{self, FORGE, Harness},
    github_start::{
        CONTENTS, EXPIRES, EXPIRY, FULL, LIVE, NEXT_PAGE, SUSPENDED, VIEWER, accepted, contains, contents, full, mint,
        tokens,
    },
};
use crate::{
    Change, Event, Request, StartRead, Stopped,
    installations::{Read, Tokens},
    start::Start,
};
use alloc::boxed::Box;
use skein_forge::github::{document, request::Scope};
use skein_lib::{Duration, List, Queue, Time, Wall};

const SECOND: Duration = Duration::from_secs(1);
const MISSING: u64 = 9;
const OTHER: &[u8] = br#"[{"id":2,"account":{"id":23,"login":"other","type":"User"},"created_at":"1970-01-01T00:00:01Z","suspended_at":null}]"#;
const RENAMED: &[u8] = br#"[{"id":1,"account":{"id":22,"login":"renamed","type":"Organization"},"created_at":"1970-01-01T00:00:01Z","suspended_at":null}]"#;
const WRONG: &[u8] = br#"{"data":{"viewer":{"databaseId":8,"login":"other"}}}"#;
#[derive(Clone, Copy, Debug)]
enum Stage {
    Reading,
    ReadWaiting,
    Diff,
    Full,
    Contents,
    Viewer,
    Waiting,
    Held,
}
#[derive(Clone, Copy, Debug)]
enum Loss {
    Unaccepted,
    Deleted,
    Suspended,
}
fn empty_start() -> Harness {
    let mut h = Harness::new(fixtures::github_settings());
    h.connected(FORGE);
    h.answer(FORGE, &fixtures::response_with_fields(200, LIVE, NEXT_PAGE));
    h.answer(FORGE, &fixtures::ok(OTHER));
    h.run(Duration::ZERO);
    h
}
fn ready() -> Harness {
    let mut h = empty_start();
    accepted(&mut h, &[1]);
    full(&mut h);
    contents(&mut h);
    h.answer(FORGE, &fixtures::ok(VIEWER));
    h.run(Duration::ZERO);
    h
}
fn at(stage: Stage) -> Harness {
    match stage {
        Stage::Reading => background_read(false),
        Stage::ReadWaiting => background_read(true),
        Stage::Diff => {
            let mut h = ready();
            commit(&mut h, Box::from([row(1, false), row(2, false)]));
            h
        }
        Stage::Full | Stage::Contents | Stage::Viewer | Stage::Waiting | Stage::Held => mint_at(stage),
    }
}
fn background_read(waiting: bool) -> Harness {
    let mut h = ready();
    accepted(&mut h, &[1, MISSING]);
    if waiting {
        h.answer(FORGE, &fixtures::status(503));
    }
    h
}
fn mint_at(stage: Stage) -> Harness {
    let mut h = empty_start();
    accepted(&mut h, &[1]);
    match stage {
        Stage::Reading | Stage::ReadWaiting | Stage::Diff => unreachable!("mint fixture phase"),
        Stage::Full => {}
        Stage::Contents => full(&mut h),
        Stage::Viewer => {
            full(&mut h);
            contents(&mut h);
        }
        Stage::Waiting => {
            h.answer(FORGE, &fixtures::status(503));
        }
        Stage::Held => {
            full(&mut h);
            h.run(Duration::ZERO);
            // Hold the minted value before the local output transition.
            let gh = h.component.test_github_mut();
            let selected = gh.selected.expect("selected Contents");
            gh.held = Some(crate::installations::HeldToken {
                selected,
                token: skein_lib::Secret::new(Box::from(CONTENTS)),
                expires: EXPIRY,
            });
            return h;
        }
    }
    // The next request is written before a test supplies loss or its late reply.
    h.run(Duration::ZERO);
    h
}
fn refresh(h: &mut Harness, body: &[u8]) {
    accepted(h, &[1, MISSING]);
    h.answer(FORGE, &fixtures::ok(body));
    h.run(Duration::ZERO);
}
pub(super) fn row(id: u64, suspended: bool) -> document::Installation {
    document::Installation {
        id,
        account: 22,
        login: Box::from(b"acme".as_slice()),
        kind: document::Account::Organization,
        created: Wall::EPOCH,
        suspended,
    }
}
// Feed the committed reconciliation transition directly to cross a written job
// with loss; socket serialization itself is checked by the wire stories.
pub(super) fn commit(h: &mut Harness, rows: Box<[document::Installation]>) {
    let gh = h.component.test_github_mut();
    let mut reading = crate::installations::Reading::new(gh.rows.capacity(), gh.generation, SECOND);
    reading.page(rows, &gh.rows, &gh.accepted, fixtures::shared().name_bytes).expect("fixture read");
    gh.read = Read::Reading(reading);
    gh.commit();
}
fn lose(h: &mut Harness, loss: Loss) {
    match loss {
        Loss::Unaccepted => accepted(h, &[]),
        Loss::Deleted => commit(h, Box::from([])),
        Loss::Suspended => commit(h, Box::from([row(1, true)])),
    }
}
fn forgotten(h: &Harness, id: u64) {
    if let Some(row) = h.component.test_github().rows.get(&id) {
        assert!(row.api.is_none());
        assert_eq!((row.api_expires, row.git_expires, row.state), (Wall::EPOCH, Wall::EPOCH, Tokens::Gone));
    }
}
pub(super) fn public_ready(h: &Harness) {
    assert_eq!(h.start(FORGE), Start::Ready { cap: 100 });
    assert_eq!(h.connected_count, 1);
}
pub(super) fn requests(h: &Harness) -> u32 {
    let wire = h.all_request_bytes(FORGE);
    let mut count = 0_u32;
    for piece in wire.windows(b" HTTP/1.1\r\n".len()) {
        if piece == b" HTTP/1.1\r\n" {
            count = count.checked_add(1).expect("bounded request history");
        }
    }
    count
}
fn check_sequence(h: &Harness, id: &[u8]) {
    let wire = h.all_request_bytes(FORGE);
    assert!(contains(&wire, id));
    assert!(contains(&wire, b"{\"permissions\":{\"contents\":\"write\"}}"));
}
#[test]
fn newly_accepted_row_mints_while_ready() {
    let mut h = ready();
    accepted(&mut h, &[1, 2]);
    full(&mut h);
    contents(&mut h);
    h.run(Duration::ZERO);
    public_ready(&h);
    assert_eq!(tokens(&h).len(), 2);
    assert_eq!(requests(&h), 7); // two pages, two pairs of mints and one Viewer
    check_sequence(&h, b"/app/installations/2/access_tokens");
    let gh = h.component.test_github();
    assert_eq!(gh.rows.get(&1).expect("other Current row").state, Tokens::Current);
    assert!(gh.identity_checked);
    assert_eq!(h.connections_opened(), 1);
}
#[test]
fn unsuspended_accepted_row_mints_while_ready() {
    let mut h = Harness::new(fixtures::github_settings());
    accepted(&mut h, &[1]);
    h.connected(FORGE);
    h.answer(FORGE, &fixtures::ok(SUSPENDED));
    h.run(Duration::ZERO);
    refresh(&mut h, LIVE);
    full(&mut h);
    contents(&mut h);
    h.answer(FORGE, &fixtures::ok(VIEWER));
    h.run(Duration::ZERO);
    public_ready(&h);
    assert_eq!(tokens(&h).len(), 1);
    assert_eq!(h.component.test_github().rows.get(&1).expect("row").state, Tokens::Current);
}
#[test]
fn equal_accepted_set_does_not_remint() {
    let mut h = ready();
    accepted(&mut h, &[1, 1]);
    h.run(Duration::from_secs(100));
    assert_eq!(requests(&h), 5);
    assert_eq!(tokens(&h).len(), 1);
    public_ready(&h);
}
#[test]
fn rename_preserves_current_tokens_without_reminting() {
    let mut h = ready();
    refresh(&mut h, RENAMED);
    assert_eq!(requests(&h), 6);
    let row = h.component.test_github().rows.get(&1).expect("renamed row");
    assert!(row.api.as_ref().expect("Full").expose() == FULL);
    assert_eq!((row.api_expires, row.git_expires), (EXPIRY, EXPIRY));
    public_ready(&h);
}
#[test]
fn unaccepted_row_loses_tokens_immediately() {
    let mut h = ready();
    accepted(&mut h, &[]);
    forgotten(&h, 1);
    public_ready(&h);
}
#[test]
fn deleted_row_loses_tokens_on_commit() {
    let mut h = ready();
    refresh(&mut h, b"[]");
    forgotten(&h, 1);
    assert!(!h.component.test_github().rows.contains_key(&1));
}
#[test]
fn suspended_row_loses_tokens_on_commit() {
    let mut h = ready();
    refresh(&mut h, SUSPENDED);
    forgotten(&h, 1);
    public_ready(&h);
}
#[test]
fn partial_failed_refresh_keeps_current_tokens() {
    let mut h = ready();
    accepted(&mut h, &[1, MISSING]);
    h.answer(FORGE, &fixtures::response_with_fields(200, SUSPENDED, NEXT_PAGE));
    h.answer(FORGE, &fixtures::status(503));
    let row = h.component.test_github().rows.get(&1).expect("committed row");
    assert_eq!((row.state, row.api_expires, row.git_expires), (Tokens::Current, EXPIRY, EXPIRY));
    assert!(row.api.is_some());
    public_ready(&h);
}
fn stale(stage: Stage, loss: Loss) {
    let mut h = at(stage);
    lose(&mut h, loss);
    h.answer(FORGE, &mint(b"artificial-stale", EXPIRES));
    h.run(Duration::ZERO);
    forgotten(&h, 1);
    assert!(tokens(&h).is_empty());
    assert!(h.component.test_github().held.is_none());
    public_ready(&h);
}
#[test]
fn loss_then_reacceptance_rejects_written_full() {
    let mut h = at(Stage::Full);
    accepted(&mut h, &[]);
    accepted(&mut h, &[1]);
    h.answer(FORGE, &mint(b"artificial-stale", EXPIRES));
    assert!(h.component.test_github().rows.get(&1).expect("row").api.is_none());
    full(&mut h);
    contents(&mut h);
    h.answer(FORGE, &fixtures::ok(VIEWER));
    h.run(Duration::ZERO);
    assert_eq!(tokens(&h).len(), 1);
}
#[test]
fn loss_then_reacceptance_rejects_written_contents() {
    let mut h = at(Stage::Contents);
    accepted(&mut h, &[]);
    accepted(&mut h, &[1]);
    h.answer(FORGE, &mint(b"artificial-stale", EXPIRES));
    h.run(Duration::ZERO);
    assert!(tokens(&h).is_empty());
    assert!(h.component.test_github().rows.get(&1).expect("row").api.is_none());
}
#[test]
fn gone_becoming_eligible_gets_fresh_both_scopes() {
    let mut h = ready();
    accepted(&mut h, &[]);
    accepted(&mut h, &[1]);
    full(&mut h);
    contents(&mut h);
    h.run(Duration::ZERO);
    assert_eq!(tokens(&h).len(), 2);
    assert_eq!(requests(&h), 7);
    public_ready(&h);
}
#[test]
fn first_mint_after_empty_start_checks_bot() {
    let mut h = at(Stage::Viewer);
    h.run(Duration::ZERO);
    public_ready(&h);
    assert!(!h.component.test_github().identity_checked);
    let wire = h.all_request_bytes(FORGE);
    let bearer = skein_forge::github::credential::bearer(&skein_lib::Secret::new(Box::from(FULL))).expect("bearer");
    assert!(contains(&wire, bearer.expose()));
    h.answer(FORGE, &fixtures::ok(VIEWER));
    accepted(&mut h, &[1, 2]);
    full(&mut h);
    contents(&mut h);
    h.run(Duration::ZERO);
    assert!(h.component.test_github().identity_checked);
    assert_eq!(requests(&h), 7);
    public_ready(&h);
}
#[test]
fn late_wrong_user_stops_once_after_connected() {
    let mut h = at(Stage::Viewer);
    h.answer(FORGE, &fixtures::ok(WRONG));
    h.run(Duration::from_secs(100));
    assert_eq!(h.stopped(), Some((FORGE, Stopped::OtherUser { user: 8 })));
    assert_eq!(h.connected_count, 1);
    assert_eq!(requests(&h), 5);
    forgotten(&h, 1);
}
#[test]
fn terminal_deferred_viewer_status_stops_after_connected() {
    terminal_viewer(401);
}
#[test]
fn deferred_viewer_403_is_terminal() {
    terminal_viewer(403);
}
#[test]
fn deferred_viewer_404_is_terminal() {
    terminal_viewer(404);
}
#[test]
fn deferred_viewer_422_is_terminal() {
    terminal_viewer(422);
}
fn terminal_viewer(status: u16) {
    let mut h = at(Stage::Viewer);
    h.answer(FORGE, &fixtures::status(status));
    h.run(Duration::from_secs(100));
    assert_eq!(h.stopped(), Some((FORGE, Stopped::Refused { read: StartRead::Viewer, status })));
    assert_eq!(h.connected_count, 1);
    forgotten(&h, 1);
}

#[test]
fn transient_deferred_identity_retries_once_due() {
    let mut h = at(Stage::Viewer);
    h.answer(FORGE, &fixtures::status(503));
    h.run(Duration::ZERO);
    let count = tokens(&h).len();
    assert_eq!(h.component.next_deadline(), Some(Time::from_nanos(SECOND.as_nanos())));
    h.run(Duration::from_millis(999));
    assert_eq!(requests(&h), 5);
    h.run(Duration::from_millis(1));
    h.connected(FORGE);
    h.answer(FORGE, &fixtures::ok(VIEWER));
    h.run(Duration::ZERO);
    assert_eq!(requests(&h), 6);
    assert_eq!(tokens(&h).len(), count);
    assert!(h.component.test_github().identity_checked);
    public_ready(&h);
}
#[test]
fn expired_identity_candidate_restarts_mints() {
    let mut h = at(Stage::Viewer);
    h.answer(FORGE, &fixtures::status(503));
    h.env.now = Time::from_nanos(EXPIRY.as_nanos());
    h.run(SECOND);
    h.connected(FORGE);
    assert_eq!(h.component.test_github().mint, Some((1, Scope::Full)));
    assert!(h.component.test_github().rows.get(&1).expect("row").api.is_none());
    public_ready(&h);
}
#[test]
fn row_loss_during_viewer_cannot_validate_identity() {
    let mut h = at(Stage::Viewer);
    accepted(&mut h, &[2]);
    h.answer(FORGE, &fixtures::ok(WRONG));
    assert!(!h.component.test_github().identity_checked);
    full(&mut h);
    contents(&mut h);
    h.answer(FORGE, &fixtures::ok(VIEWER));
    h.run(Duration::ZERO);
    assert!(h.component.test_github().identity_checked);
    public_ready(&h);
}
fn refused_refresh(status: u16, body: &[u8], gone: bool) {
    let mut h = at(Stage::Contents);
    h.answer(FORGE, &fixtures::status(status));
    h.run(Duration::ZERO);
    let row = h.component.test_github().rows.get(&1).expect("row");
    assert!(row.api.is_none());
    assert_eq!((row.api_expires, row.git_expires), (Wall::EPOCH, Wall::EPOCH));
    assert_eq!(requests(&h), 4);
    h.run(SECOND);
    h.connected(FORGE);
    h.answer(FORGE, &fixtures::ok(body));
    h.run(Duration::ZERO);
    assert_eq!(requests(&h), if gone { 5 } else { 6 });
    public_ready(&h);
}
#[test]
fn ready_mint_403_waits_then_refreshes_suspension() {
    refused_refresh(403, SUSPENDED, true);
}
#[test]
fn ready_mint_404_waits_then_refreshes_deletion() {
    refused_refresh(404, b"[]", true);
}
#[test]
fn unexplained_mint_refusal_doubles_after_refresh() {
    let mut h = at(Stage::Full);
    h.answer(FORGE, &fixtures::status(403));
    h.run(SECOND);
    h.connected(FORGE);
    h.answer(FORGE, &fixtures::ok(LIVE));
    h.answer(FORGE, &fixtures::status(403));
    assert_eq!(h.component.test_github().deadline(), Some(Time::from_nanos(3_000_000_000)));
    public_ready(&h);
}
#[test]
fn ready_mint_401_stops_key_refused() {
    let mut h = at(Stage::Full);
    h.answer(FORGE, &fixtures::status(401));
    h.run(Duration::ZERO);
    assert_eq!(h.stopped(), Some((FORGE, Stopped::KeyRefused)));
    forgotten(&h, 1);
}
#[test]
fn ready_mint_terminal_status_stops_token_refused() {
    let mut h = at(Stage::Full);
    h.answer(FORGE, &fixtures::status(422));
    h.run(Duration::ZERO);
    assert_eq!(h.stopped(), Some((FORGE, Stopped::TokenRefused { installation: 1, status: 422 })));
}
#[test]
fn transient_mint_retries_selected_scope_without_listing() {
    transient_mint(429, b"{}");
}
#[test]
fn server_mint_failure_retries_selected_scope() {
    transient_mint(503, b"{}");
}
#[test]
fn malformed_mint_retries_selected_scope() {
    transient_mint(201, b"{");
}
fn transient_mint(status: u16, body: &[u8]) {
    let mut h = at(Stage::Contents);
    h.answer(FORGE, &fixtures::response(status, body));
    h.run(SECOND);
    h.connected(FORGE);
    contents(&mut h);
    h.answer(FORGE, &fixtures::ok(VIEWER));
    h.run(Duration::ZERO);
    assert_eq!(requests(&h), 6);
    assert_eq!(tokens(&h).len(), 1);
    public_ready(&h);
}

#[test]
fn ready_mint_backoff_doubles_to_cap() {
    let mut h = at(Stage::Full);
    for (wait, until) in [(1, 1), (2, 3), (4, 7), (8, 15), (8, 23)] {
        h.answer(FORGE, &fixtures::status(503));
        assert_eq!(h.component.test_github().deadline(), Some(Time::from_nanos(Duration::from_secs(until).as_nanos())));
        h.run(Duration::from_secs(wait));
        h.connected(FORGE);
        public_ready(&h);
    }
}
#[test]
fn ready_mint_long_retry_after_wins() {
    let mut h = at(Stage::Full);
    h.answer(FORGE, &fixtures::response_with_fields(429, b"{}", b"Retry-After: 20\r\n"));
    assert_eq!(h.component.test_github().deadline(), Some(Time::from_nanos(20_000_000_000)));
    h.run(Duration::from_secs(19));
    assert_eq!(h.component.test_github().deadline(), Some(Time::from_nanos(20_000_000_000)));
    h.run(SECOND);
    h.connected(FORGE);
    full(&mut h);
    contents(&mut h);
    h.answer(FORGE, &fixtures::ok(VIEWER));
    h.run(Duration::ZERO);
    public_ready(&h);
}
#[test]
fn loss_suppresses_held_git_token() {
    let mut h = at(Stage::Held);
    accepted(&mut h, &[]);
    h.run(Duration::ZERO);
    assert!(tokens(&h).is_empty());
    assert!(h.component.test_github().held.is_none());
}
#[test]
fn ready_changes_precede_the_held_token() {
    let mut h = at(Stage::Held);
    commit(&mut h, Box::from([row(1, false), row(2, false)]));
    // A lifecycle diff precedes the held token.
    h.component.test_github_mut().read = Read::Changes(crate::installations::Diff {
        old: skein_lib::Map::with_capacity(3),
        accepted: skein_lib::Set::with_capacity(3),
        missing: skein_lib::Map::with_capacity(3),
        baseline: true,
        after: None,
        renamed: None,
    });
    let mut up = Queue::uncapped();
    let mut io = Queue::uncapped();
    h.component.test_local(&h.env, &mut up);
    assert_eq!(up.len(), 1);
    assert!(h.component.test_github().held.is_some());
    assert!(io.is_empty());
    let mut delivered = List::with_capacity(3);
    for _ in 0_u32..3 {
        h.component.fire(&h.env, &mut up, &mut io);
        for _ in 0..up.len() {
            delivered.push(up.pop().expect("event")).expect("bounded output");
        }
    }
    let mut git = 0_u32;
    let mut changes = 0_u32;
    for event in delivered.as_slice() {
        match event {
            Event::GitToken { .. } => git = git.checked_add(1).expect("one held token"),
            Event::Installation { change, .. } => {
                assert_eq!(*change, Change::Created);
                changes = changes.checked_add(1).expect("bounded changes");
            }
            Event::Fact { .. }
            | Event::Answered(_)
            | Event::Hint(_)
            | Event::Listening { .. }
            | Event::ListenerLost
            | Event::Connected { .. }
            | Event::Stopped { .. }
            | Event::Closed => unreachable!("held outputs"),
        }
    }
    assert_eq!((git, changes), (1, 2));
}
fn close_at(stage: Stage) {
    let mut h = at(stage);
    h.send(Request::Close);
    h.run(Duration::ZERO);
    assert!(h.closed);
    assert!(h.component.test_github().held.is_none());
    forgotten(&h, 1);
    let count = tokens(&h).len();
    h.run(Duration::from_secs(100));
    assert_eq!(tokens(&h).len(), count);
}
fn replace_at(stage: Stage) {
    let mut h = at(stage);
    let tokens_before = tokens(&h).len();
    h.send(Request::Connect {
        forge: FORGE,
        access: fixtures::app_access(skein_fake_forge_github::keys::KEYS.first().expect("public test key"), 7, 9),
    });
    h.run(Duration::ZERO);
    h.connected(FORGE);
    h.answer(FORGE, &fixtures::ok(SUSPENDED));
    h.run(Duration::ZERO);
    assert_eq!(h.connected_count, 2);
    assert!(!h.component.test_github().identity_checked);
    assert_eq!(tokens(&h).len(), tokens_before);
    forgotten(&h, 1);
    assert!(h.component.test_github().held.is_none());
}
#[test]
fn retry_debug_redacts_tokens() {
    redacted(Stage::Waiting);
}
#[test]
fn held_debug_redacts_tokens() {
    redacted(Stage::Held);
}
#[test]
fn candidate_debug_redacts_tokens() {
    redacted(Stage::Viewer);
}
#[expect(clippy::disallowed_macros, reason = "the test inspects Secret redaction on private retry and held paths")]
fn redacted(stage: Stage) {
    let h = at(stage);
    let debug = std::format!("{:?} {:?} {:?}", h.component, h.up, h.io);
    assert!(!contains(debug.as_bytes(), FULL));
    assert!(!contains(debug.as_bytes(), CONTENTS));
}

#[test]
fn stale_full_after_unaccepted_is_discarded() {
    stale(Stage::Full, Loss::Unaccepted);
}

#[test]
fn stale_full_after_deleted_is_discarded() {
    stale(Stage::Full, Loss::Deleted);
}

#[test]
fn stale_full_after_suspended_is_discarded() {
    stale(Stage::Full, Loss::Suspended);
}

#[test]
fn stale_contents_after_unaccepted_is_discarded() {
    stale(Stage::Contents, Loss::Unaccepted);
}

#[test]
fn stale_contents_after_deleted_is_discarded() {
    stale(Stage::Contents, Loss::Deleted);
}

#[test]
fn stale_contents_after_suspended_is_discarded() {
    stale(Stage::Contents, Loss::Suspended);
}

#[test]
fn close_during_ready_full_discards_private_work() {
    close_at(Stage::Full);
}

#[test]
fn replace_during_ready_full_discards_private_work() {
    replace_at(Stage::Full);
}

#[test]
fn close_during_ready_contents_discards_private_work() {
    close_at(Stage::Contents);
}

#[test]
fn replace_during_ready_contents_discards_private_work() {
    replace_at(Stage::Contents);
}

#[test]
fn close_during_ready_viewer_discards_private_work() {
    close_at(Stage::Viewer);
}

#[test]
fn replace_during_ready_viewer_discards_private_work() {
    replace_at(Stage::Viewer);
}

#[test]
fn close_during_ready_waiting_discards_private_work() {
    close_at(Stage::Waiting);
}

#[test]
fn replace_during_ready_waiting_discards_private_work() {
    replace_at(Stage::Waiting);
}

#[test]
fn close_during_ready_held_discards_private_work() {
    close_at(Stage::Held);
}

#[test]
fn replace_during_ready_held_discards_private_work() {
    replace_at(Stage::Held);
}

#[test]
fn unrelated_acceptance_keeps_the_written_full_eligible() {
    let mut h = at(Stage::Full);
    accepted(&mut h, &[1, 2]);
    full(&mut h);
    assert!(h.component.test_github().rows.get(&1).expect("row").api.as_ref().expect("Full").expose() == FULL);
    contents(&mut h);
    full(&mut h);
    contents(&mut h);
    h.answer(FORGE, &fixtures::ok(VIEWER));
    h.run(Duration::ZERO);
    assert_eq!(tokens(&h).len(), 2);
    public_ready(&h);
}
#[test]
fn suspension_then_unsuspension_rejects_the_old_full() {
    let mut h = at(Stage::Full);
    commit(&mut h, Box::from([row(1, true)]));
    commit(&mut h, Box::from([row(1, false)]));
    h.answer(FORGE, &mint(b"artificial-stale", EXPIRES));
    assert!(h.component.test_github().rows.get(&1).expect("row").api.is_none());
    full(&mut h);
    contents(&mut h);
    h.answer(FORGE, &fixtures::ok(VIEWER));
    h.run(Duration::ZERO);
    assert_eq!(tokens(&h).len(), 1);
}
#[test]
fn suspension_then_unsuspension_rejects_the_old_contents() {
    let mut h = at(Stage::Contents);
    commit(&mut h, Box::from([row(1, true)]));
    commit(&mut h, Box::from([row(1, false)]));
    h.answer(FORGE, &mint(b"artificial-stale", EXPIRES));
    h.run(Duration::ZERO);
    assert!(tokens(&h).is_empty());
    assert!(h.component.test_github().rows.get(&1).expect("row").api.is_none());
}
#[test]
fn deferred_identity_candidate_loss_during_wait_uses_another_row() {
    let mut h = at(Stage::Viewer);
    h.answer(FORGE, &fixtures::status(503));
    accepted(&mut h, &[2]);
    h.run(Duration::ZERO);
    h.connected(FORGE);
    full(&mut h);
    contents(&mut h);
    h.answer(FORGE, &fixtures::ok(VIEWER));
    h.run(Duration::ZERO);
    assert!(h.component.test_github().identity_checked);
    public_ready(&h);
}
#[test]
fn read_refusal_while_ready_stops_once_after_connected() {
    let mut h = empty_start();
    accepted(&mut h, &[1, MISSING]);
    h.answer(FORGE, &fixtures::status(401));
    h.run(Duration::from_secs(100));
    assert_eq!(h.stopped(), Some((FORGE, Stopped::KeyRefused)));
    assert_eq!(h.connected_count, 1);
    forgotten(&h, 1);
}
#[test]
fn each_ready_send_has_a_fresh_deadline() {
    let mut h = at(Stage::Full);
    for (response, deadline) in [(mint(FULL, EXPIRES), 19), (mint(CONTENTS, EXPIRES), 28)] {
        h.run(Duration::from_secs(9));
        h.answer(FORGE, &response);
        assert_eq!(h.component.next_deadline(), Some(Time::from_nanos(Duration::from_secs(deadline).as_nanos())));
        public_ready(&h);
    }
}
#[test]
fn ready_transport_failure_retries_without_a_listing() {
    let mut h = at(Stage::Full);
    h.reset(FORGE);
    h.run(SECOND);
    h.connected(FORGE);
    full(&mut h);
    contents(&mut h);
    h.answer(FORGE, &fixtures::ok(VIEWER));
    h.run(Duration::ZERO);
    assert_eq!(tokens(&h).len(), 1);
    public_ready(&h);
}

#[test]
fn reacceptance_cannot_bypass_a_refused_mints_wait_and_reread() {
    let mut h = at(Stage::Full);
    h.answer(FORGE, &fixtures::status(403));
    accepted(&mut h, &[]);
    accepted(&mut h, &[1]);
    h.run(Duration::ZERO);
    assert_eq!(h.component.test_github().deadline(), Some(Time::from_nanos(SECOND.as_nanos())));
    assert!(h.component.test_github().mint.is_none());
    h.run(SECOND);
    h.connected(FORGE);
    h.answer(FORGE, &fixtures::ok(SUSPENDED));
    h.run(Duration::ZERO);
    forgotten(&h, 1);
    assert_eq!(requests(&h), 4);
}

#[test]
fn acceptance_without_access_waits_for_connect() {
    let mut settings = fixtures::github_settings();
    settings.forges.get_mut(0).expect("fixture forge").access = None;
    let mut h = Harness::new(settings);
    accepted(&mut h, &[1]);
    h.run(Duration::from_secs(100));
    assert_eq!(h.start(FORGE), Start::Unconnected);
    assert_eq!(h.connections_opened(), 0);
}
#[test]
fn acceptance_after_late_stop_cannot_restart_work() {
    let mut h = at(Stage::Full);
    h.answer(FORGE, &fixtures::status(401));
    accepted(&mut h, &[1, MISSING]);
    h.run(Duration::from_secs(100));
    assert_eq!(h.start(FORGE), Start::Stopped(Stopped::KeyRefused));
    assert_eq!(h.connections_opened(), 1);
}

#[test]
fn close_during_ready_reading_discards_private_work() {
    close_at(Stage::Reading);
}

#[test]
fn replace_during_ready_reading_discards_private_work() {
    replace_at(Stage::Reading);
}

#[test]
fn close_during_ready_readwaiting_discards_private_work() {
    close_at(Stage::ReadWaiting);
}

#[test]
fn replace_during_ready_readwaiting_discards_private_work() {
    replace_at(Stage::ReadWaiting);
}

#[test]
fn close_during_ready_diff_discards_private_work() {
    close_at(Stage::Diff);
}

#[test]
fn replace_during_ready_diff_discards_private_work() {
    replace_at(Stage::Diff);
}

#[test]
fn lost_unsent_mint_is_retired_without_sending() {
    let mut h = empty_start();
    h.component.test_github_mut().accept(&[1]);
    h.component.test_local(&h.env, &mut h.up);
    accepted(&mut h, &[]);
    h.run(Duration::ZERO);
    assert_eq!(requests(&h), 2);
    assert!(tokens(&h).is_empty());
    forgotten(&h, 1);
}
