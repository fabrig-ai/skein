//! Delivery-log transcripts run wholly over the component's scripted byte streams.
use super::{fixtures as f, github_start as start};
use crate::{Change, Event, Fact, Limits, Request, Stopped, puller::State};
use alloc::boxed::Box;
use skein_forge::api::{self, Answer, Op, Read};
use skein_lib::{Decimal, Duration, List, Time, Wall, Writer};

const TAG: &[u8] = b"ETag: \"baseline\"\r\n";
const NEXT: &[u8] =
    b"ETag: \"new\"\r\nLink: <https://api.github.test/app/hook/deliveries?cursor=older%2Fpage>; rel=\"next\"\r\n";
const NEW_TAG: &[u8] = b"ETag: \"new\"\r\n";
const DATE: &[u8] = b"1970-01-01T00:00:00Z";
const LOG_REPO: api::Repository = api::Repository { forge: f::FORGE, repository: 7 };
const ISSUE: &[u8] = br#"{"action":"opened","issue":{"number":3,"body":"hello"},"repository":{"id":7}}"#;
const REPOSITORIES: &[u8] = br#"{"action":"added","installation":{"id":1,"account":{"id":22,"login":"acme"}}}"#;

fn joined(parts: &[&[u8]]) -> Box<[u8]> {
    let mut size = 0_usize;
    for part in parts {
        size = size.checked_add(part.len()).expect("fixture size");
    }
    let mut out = Writer::new(size);
    for part in parts {
        out.put(part).expect("measured fixture");
    }
    out.finish()
}
fn row(id: u8, event: &[u8], redelivery: bool) -> Box<[u8]> {
    let digit = b"0123456789abcdef".get(usize::from(id)).expect("fixture guid digit");
    joined(&[
        b"{\"id\":",
        Decimal::of(u64::from(id)).as_bytes(),
        b",\"guid\":\"00000000-0000-0000-0000-00000000000",
        &[*digit],
        b"\",\"event\":\"",
        event,
        b"\",\"action\":null,\"status_code\":200,\"delivered_at\":\"",
        DATE,
        b"\",\"redelivery\":",
        if redelivery { b"true" } else { b"false" },
        b"}",
    ])
}
fn page(rows: &[Box<[u8]>], fields: &[u8]) -> Box<[u8]> {
    let mut size = 2_usize.checked_add(rows.len().saturating_sub(1)).expect("commas");
    for row in rows {
        size = size.checked_add(row.len()).expect("page bytes");
    }
    let mut out = Writer::new(size);
    out.put(b"[").expect("page start");
    for (at, row) in rows.iter().enumerate() {
        if at != 0 {
            out.put(b",").expect("row comma");
        }
        out.put(row).expect("row");
    }
    out.put(b"]").expect("page end");
    f::response_with_fields(200, &out.finish(), fields)
}
fn delivery(event: &[u8], payload: &[u8]) -> Box<[u8]> {
    // The row's event starts the streaming decoder before the envelope's event fields.
    f::ok(&joined(&[
        b"{\"request\":{\"payload\":",
        payload,
        b",\"headers\":{\"X-GitHub-Event\":\"",
        event,
        b"\",\"X-GitHub-Delivery\":\"00000000-0000-0000-0000-000000000002\"}},\"event\":\"",
        event,
        b"\"}",
    ]))
}
fn ready(limits: Limits) -> f::Harness {
    let limits = Limits { log_poll: Duration::from_secs(60), ..limits };
    let mut settings = f::github_settings();
    settings.forges.get_mut(0).expect("forge").access = None;
    let mut h = f::Harness::with(limits, f::shared(), settings);
    h.component.test_calls_ready();
    h.run(Duration::ZERO); // Arm only after Ready.
    h
}
fn poll(h: &mut f::Harness) {
    h.run(h.env.limits.log_poll);
}
fn baseline(limits: Limits) -> f::Harness {
    let mut h = ready(limits);
    poll(&mut h);
    h.connected(f::FORGE);
    h.answer(f::FORGE, &page(&[row(1, b"issues", false)], TAG));
    h.run(Duration::ZERO);
    h
}
fn kept(h: &f::Harness) -> Option<[u8; 16]> {
    h.component.test_github().puller.newest
}
fn expected(id: u8) -> [u8; 16] {
    let mut guid = [0; 16];
    *guid.last_mut().expect("guid byte") = id;
    guid
}
fn hints(h: &mut f::Harness) {
    h.run(Duration::ZERO);
}
fn facts(h: &f::Harness) -> &List<Event> {
    h.facts.as_ref().expect("GitHub facts")
}
fn requests(h: &f::Harness, needle: &[u8]) -> u32 {
    skein_lib::bytes::count(&h.all_request_bytes(f::FORGE), needle, u32::MAX)
}
fn next_poll(h: &mut f::Harness) {
    // A test may keep its own connection alive by raising idle above log_poll.
    h.run(h.env.limits.log_poll);
    h.connected(f::FORGE);
}

fn issue_path(owner: &[u8], name: &[u8]) -> Box<[u8]> {
    joined(&[
        br#"{"action":"opened","issue":{"number":3,"body":"hello"},"repository":{"id":7,"owner":{"login":""#,
        owner,
        br#""},"name":""#,
        name,
        br#""}}"#,
    ])
}
fn logged_poll(h: &mut f::Harness, id: u8, event: &[u8], payload: &[u8]) {
    next_poll(h);
    h.answer(
        f::FORGE,
        &page(&[row(id, event, false), row(id.checked_sub(1).expect("previous guid"), b"ping", false)], NEW_TAG),
    );
    h.answer(f::FORGE, &delivery(event, payload));
    hints(h);
}

#[test]
fn logged_renames_update_kept_rows_and_route_by_the_new_name() {
    let mut h = baseline(f::limits());
    f::github_account(&mut h, 1, 22, b"acme", start::FULL);
    f::github_repository(&mut h, LOG_REPO, 22);
    let other_forge = api::Repository { forge: 2, repository: LOG_REPO.repository };
    f::github_repository(&mut h, other_forge, 23);
    let other = h.component.github_tables().repository(other_forge).expect("other forge").clone();
    let mut expected = h.component.github_tables().repository(LOG_REPO).expect("kept row").clone();
    for (id, name) in [(2, b"gadgets".as_slice()), (3, b"new-name")] {
        logged_poll(&mut h, id, b"issues", &issue_path(b"acme", name));
        expected.name = Box::from(name);
        assert_eq!(h.component.github_tables().repository(LOG_REPO), Some(&expected));
        assert_eq!(h.component.github_tables().repository(other_forge), Some(&other));
    }
    assert_eq!(h.hints.len(), 2);
    assert!(h.answers.is_empty());
    assert!(facts(&h).is_empty());
    assert!(h.installation_events.as_ref().expect("events").is_empty());
    h.call(1, LOG_REPO, Op::Read(Read::Branch { branch: Box::from(b"main".as_slice()) }));
    h.run(Duration::ZERO);
    h.connected(f::FORGE);
    assert_eq!(requests(&h, b"GET /repos/acme/new-name/branches/main "), 1);
    assert_eq!(requests(&h, b"GET /repositories/7 "), 0);
    h.answer(f::FORGE, &f::ok(f::GITHUB_BRANCH));
    h.run(Duration::ZERO);
    let answer = h.answered(1).expect("branch answer");
    assert_eq!((answer.cost, &answer.result), (1, &Ok(Answer::Commit([0; 32]))));
}

#[test]
fn logged_owner_change_drops_the_row_for_the_next_read() {
    let mut h = baseline(f::limits());
    f::github_repository(&mut h, LOG_REPO, 22);
    f::github_repository(&mut h, f::REPO, 22);
    let kept = h.component.github_tables().repository(f::REPO).expect("other repository").clone();
    logged_poll(&mut h, 2, b"issues", &issue_path(b"team", b"widgets"));
    assert!(h.component.github_tables().repository(LOG_REPO).is_none());
    assert_eq!(h.component.github_tables().repository(f::REPO), Some(&kept));
    assert_eq!(h.hints.len(), 1);
}

#[test]
fn logged_path_leaves_a_missing_repository_row_absent() {
    let mut h = baseline(f::limits());
    f::github_repository(&mut h, f::REPO, 22);
    let kept = h.component.github_tables().repository(f::REPO).expect("other repository").clone();
    logged_poll(&mut h, 2, b"issues", &issue_path(b"team", b"gadgets"));
    assert!(h.component.github_tables().repository(LOG_REPO).is_none());
    assert_eq!(h.component.github_tables().repository(f::REPO), Some(&kept));
    assert_eq!(h.hints.len(), 1);
}

#[test]
fn absent_paths_oversized_and_unhinted_deliveries_leave_rows_alone() {
    let oversized = joined(&[
        br#"{"repository":{"id":7,"owner":{"login":"team"},"name":"gadgets"},"issue":{"number":3,"body":""#,
        &f::filled(600, b'b'),
        br#""}}"#,
    ]);
    let unhinted = br#"{"action":"unknown","pull_request":{"number":3},"repository":{"id":7,"owner":{"login":"team"},"name":"gadgets"}}"#;
    for (label, event, payload, count) in [
        ("id only", b"issues".as_slice(), ISSUE, 1),
        ("past delivery_bytes", b"issues", oversized.as_ref(), 0),
        ("unhinted action", b"pull_request", unhinted.as_slice(), 0),
    ] {
        let mut h = baseline(Limits { delivery_bytes: 512, ..f::limits() });
        f::github_repository(&mut h, LOG_REPO, 22);
        let kept = h.component.github_tables().repository(LOG_REPO).expect("kept row").clone();
        logged_poll(&mut h, 2, event, payload);
        assert_eq!(h.component.github_tables().repository(LOG_REPO), Some(&kept), "{label}");
        assert_eq!(h.hints.len(), count, "{label}");
        assert!(h.answers.is_empty(), "{label}");
    }
}

#[test]
fn logged_fill_keeps_its_row_recent_for_table_eviction() {
    let mut h = baseline(f::limits());
    f::github_repository(&mut h, LOG_REPO, 22);
    f::github_repository(&mut h, f::REPO, 22);
    logged_poll(&mut h, 2, b"issues", &issue_path(b"acme", b"gadgets"));
    let third = api::Repository { forge: f::FORGE, repository: 8 };
    f::github_repository(&mut h, third, 22);
    assert!(h.component.github_tables().repository(LOG_REPO).is_some());
    assert!(h.component.github_tables().repository(f::REPO).is_none());
    assert!(h.component.github_tables().repository(third).is_some());
}

#[test]
fn first_poll_keeps_non_redelivery_guid_and_validator_without_hints() {
    let mut h = ready(f::limits());
    poll(&mut h);
    h.connected(f::FORGE);
    h.answer(f::FORGE, &page(&[row(3, b"issues", true), row(2, b"issues", false), row(1, b"issues", false)], TAG));
    hints(&mut h);
    assert_eq!(kept(&h), Some(expected(2)));
    assert_eq!(h.component.test_github().puller.validator.as_ref().expect("tag").as_bytes(), b"\"baseline\"");
    assert!(h.hints.is_empty());
    assert_eq!(requests(&h, b"GET /app/hook/deliveries/"), 0);
}

#[test]
fn cursor_poll_hints_each_row_once_and_keeps_first_pages_validator() {
    let mut h = baseline(f::limits());
    next_poll(&mut h);
    h.answer(f::FORGE, &page(&[row(3, b"issues", false)], NEXT));
    h.answer(f::FORGE, &delivery(b"issues", ISSUE));
    hints(&mut h);
    h.answer(f::FORGE, &page(&[row(2, b"issues", false), row(1, b"issues", false)], b"ETag: \"ignored\"\r\n"));
    h.answer(f::FORGE, &delivery(b"issues", ISSUE));
    hints(&mut h);
    assert_eq!(h.hints.len(), 2);
    assert_eq!(kept(&h), Some(expected(3)));
    assert_eq!(h.component.test_github().puller.validator.as_ref().expect("tag").as_bytes(), b"\"new\"");
    assert_eq!(requests(&h, b"cursor=older%2Fpage"), 1);
    assert_eq!(requests(&h, b"GET /app/hook/deliveries/3 "), 1);
    assert_eq!(requests(&h, b"GET /app/hook/deliveries/2 "), 1);
    next_poll(&mut h);
    h.answer(f::FORGE, &page(&[row(3, b"issues", false)], NEW_TAG));
    hints(&mut h);
    assert_eq!(h.hints.len(), 2);
}

#[test]
fn unchanged_poll_keeps_guid_and_validator_and_reads_no_delivery() {
    let mut h = baseline(f::limits());
    next_poll(&mut h);
    h.answer(f::FORGE, &f::response(304, b""));
    hints(&mut h);
    assert_eq!(kept(&h), Some(expected(1)));
    assert_eq!(h.component.test_github().puller.validator.as_ref().expect("tag").as_bytes(), b"\"baseline\"");
    assert_eq!(requests(&h, b"If-None-Match: \"baseline\""), 1);
    assert!(h.hints.is_empty());
}

#[test]
fn page_gap_emits_once_and_rereads_installations() {
    let mut h = baseline(Limits { log_pages: 1, ..f::limits() });
    next_poll(&mut h);
    h.answer(f::FORGE, &page(&[row(2, b"ping", false)], NEXT));
    hints(&mut h);
    assert_eq!(facts(&h).len(), 1);
    match facts(&h).get(0).expect("gap") {
        Event::Fact { fact, .. } => assert_eq!(*fact, Fact::Gap),
        Event::Answered(_)
        | Event::Installation { .. }
        | Event::GitToken { .. }
        | Event::Hint(_)
        | Event::Listening { .. }
        | Event::ListenerLost
        | Event::Connected { .. }
        | Event::Stopped { .. }
        | Event::Closed => unreachable!("fact"),
    }
    assert_eq!(requests(&h, b"GET /app/installations?"), 1);
    assert_eq!(requests(&h, b"cursor="), 0);
    h.answer(f::FORGE, &f::ok(b"[]"));
    hints(&mut h);
    assert_eq!(facts(&h).len(), 1);
    assert_eq!(kept(&h), Some(expected(2)));
}

#[test]
fn catch_up_boundary_holds_then_one_nanosecond_past_gives_gap() {
    let mut h = baseline(f::limits());
    h.env.wall = Wall::from_nanos(h.env.limits.log_catch_up.as_nanos());
    next_poll(&mut h);
    h.answer(f::FORGE, &page(&[row(2, b"ping", false), row(1, b"ping", false)], b""));
    hints(&mut h);
    assert!(facts(&h).is_empty());
    h.env.wall = Wall::from_nanos(h.env.limits.log_catch_up.as_nanos().saturating_add(1));
    next_poll(&mut h);
    h.answer(f::FORGE, &page(&[row(3, b"ping", false), row(2, b"ping", false)], b""));
    hints(&mut h);
    assert_eq!(facts(&h).len(), 1);
    assert_eq!(requests(&h, b"GET /app/installations?"), 1);
}

#[test]
fn redelivery_is_skipped_and_never_becomes_newest() {
    let mut h = baseline(f::limits());
    next_poll(&mut h);
    h.answer(f::FORGE, &page(&[row(3, b"issues", true), row(2, b"issues", false), row(1, b"issues", false)], b""));
    h.answer(f::FORGE, &delivery(b"issues", ISSUE));
    hints(&mut h);
    assert_eq!(kept(&h), Some(expected(2)));
    assert_eq!(h.hints.len(), 1);
    assert_eq!(requests(&h, b"GET /app/hook/deliveries/3 "), 0);
}

#[test]
fn two_installation_rows_trigger_one_whole_reread() {
    let mut h = baseline(f::limits());
    next_poll(&mut h);
    h.answer(
        f::FORGE,
        &page(&[row(3, b"installation", false), row(2, b"installation", false), row(1, b"ping", false)], b""),
    );
    h.answer(f::FORGE, &f::ok(b"[]"));
    hints(&mut h);
    assert_eq!(requests(&h, b"GET /app/installations?"), 1);
    assert_eq!(requests(&h, b"GET /app/hook/deliveries/"), 0);
    assert_eq!(kept(&h), Some(expected(3)));
}

#[test]
fn repository_row_reads_whole_and_sends_its_installation_change() {
    let mut h = baseline(f::limits());
    next_poll(&mut h);
    h.answer(f::FORGE, &page(&[row(2, b"installation_repositories", false), row(1, b"ping", false)], b""));
    h.answer(f::FORGE, &delivery(b"installation_repositories", REPOSITORIES));
    hints(&mut h);
    match h.installation_events.as_ref().expect("events").get(0).expect("repositories") {
        Event::Installation { installation, account, change, .. } => {
            assert_eq!((*installation, *change), (1, Change::Repositories));
            assert_eq!(account.as_ref().expect("account").id, 22);
        }
        Event::Answered(_)
        | Event::Fact { .. }
        | Event::GitToken { .. }
        | Event::Hint(_)
        | Event::Listening { .. }
        | Event::ListenerLost
        | Event::Connected { .. }
        | Event::Stopped { .. }
        | Event::Closed => unreachable!("installation"),
    }
    assert_eq!(requests(&h, b"GET /app/hook/deliveries/2 "), 1);
}

#[test]
fn refused_hint_blocks_remaining_deliveries_until_offer_and_admission() {
    let mut h = baseline(f::limits());
    next_poll(&mut h);
    h.answer(f::FORGE, &page(&[row(3, b"issues", false), row(2, b"issues", false), row(1, b"ping", false)], b""));
    h.answer(f::FORGE, &delivery(b"issues", ISSUE));
    let hinted = match h.up.pop().expect("hint offer") {
        Event::Hint(hint) => hint,
        Event::Answered(_)
        | Event::Installation { .. }
        | Event::Fact { .. }
        | Event::GitToken { .. }
        | Event::Listening { .. }
        | Event::ListenerLost
        | Event::Connected { .. }
        | Event::Stopped { .. }
        | Event::Closed => unreachable!("hint"),
    };
    h.send(Request::RefusedHint(hinted.clone()));
    h.run(Duration::ZERO);
    assert_eq!(requests(&h, b"GET /app/hook/deliveries/2 "), 0);
    assert_eq!(kept(&h), Some(expected(1)));
    h.send(Request::Offer);
    let again = match h.up.pop().expect("offered again") {
        Event::Hint(hint) => hint,
        Event::Answered(_)
        | Event::Installation { .. }
        | Event::Fact { .. }
        | Event::GitToken { .. }
        | Event::Listening { .. }
        | Event::ListenerLost
        | Event::Connected { .. }
        | Event::Stopped { .. }
        | Event::Closed => unreachable!("hint"),
    };
    assert_eq!(again, hinted);
    h.run(Duration::ZERO);
    assert_eq!(requests(&h, b"GET /app/hook/deliveries/2 "), 1);
    h.answer(f::FORGE, &delivery(b"issues", ISSUE));
    hints(&mut h);
    assert_eq!(kept(&h), Some(expected(3)));
}

#[test]
fn delivery_bound_holds_at_limit_and_skips_one_byte_past() {
    let response = delivery(b"issues", ISSUE);
    let body =
        response.get(skein_lib::bytes::find(&response, b"\r\n\r\n").expect("head").saturating_add(4)..).expect("body");
    let size = u32::try_from(body.len()).expect("fixture size");
    for (bound, count) in [(size, 2), (size.saturating_sub(1), 0)] {
        let mut h = baseline(Limits { delivery_bytes: bound, ..f::limits() });
        next_poll(&mut h);
        h.answer(f::FORGE, &page(&[row(3, b"issues", false), row(2, b"issues", false), row(1, b"ping", false)], b""));
        h.answer(f::FORGE, &response);
        hints(&mut h);
        h.connected(f::FORGE);
        h.answer(f::FORGE, &response);
        hints(&mut h);
        assert_eq!(h.hints.len(), count);
        assert_eq!(kept(&h), Some(expected(3)));
        assert_eq!(requests(&h, b"GET /app/hook/deliveries/2 "), 1);
    }
}

#[test]
fn failed_page_preserves_guid_validator_and_retries_at_next_poll() {
    let mut h = baseline(f::limits());
    next_poll(&mut h);
    h.answer(f::FORGE, &page(&[row(3, b"ping", false)], NEXT));
    h.answer(f::FORGE, &f::response(500, b"{}"));
    hints(&mut h);
    assert_eq!(kept(&h), Some(expected(1)));
    assert_eq!(h.component.test_github().puller.validator.as_ref().expect("tag").as_bytes(), b"\"baseline\"");
    next_poll(&mut h);
    assert_eq!(requests(&h, b"If-None-Match: \"baseline\""), 2);
}

#[test]
fn unauthorized_page_or_delivery_stops_the_forge() {
    for whole in [false, true] {
        let mut h = baseline(f::limits());
        next_poll(&mut h);
        if whole {
            h.answer(f::FORGE, &page(&[row(2, b"issues", false)], b""));
        }
        h.answer(f::FORGE, &f::response(401, b"{}"));
        hints(&mut h);
        assert_eq!(h.stopped, Some((f::FORGE, Stopped::KeyRefused)));
        assert_eq!(kept(&h), Some(expected(1)));
        assert_eq!(h.component.test_github().puller.due, None);
    }
}

#[test]
fn poll_timer_runs_only_ready_and_forgejo_never_polls() {
    let mut h = f::Harness::new(f::github_settings());
    assert_eq!(h.component.test_github().puller.due, None);
    h.connected(f::FORGE);
    h.answer(f::FORGE, &f::ok(b"[]"));
    h.run(Duration::ZERO);
    assert_eq!(h.component.test_github().puller.due, Some(Time::ZERO.saturating_add(h.env.limits.log_poll)));
    h.send(Request::Close);
    assert_eq!(h.component.test_github().puller.due, None);
    let mut forgejo = f::Harness::ready();
    forgejo.run(f::limits().log_poll);
    assert_eq!(requests(&forgejo, b"/app/hook/deliveries"), 0);
}

#[test]
fn connect_and_stop_preserve_the_guid_for_catch_up() {
    let mut h = baseline(f::limits());
    next_poll(&mut h);
    h.answer(f::FORGE, &f::response(401, b"{}"));
    hints(&mut h);
    h.send(Request::Connect {
        forge: f::FORGE,
        access: f::app_access(skein_fake_forge_github::keys::KEYS.first().expect("public test key"), 7, 9),
    });
    h.run(Duration::ZERO);
    h.connected(f::FORGE);
    h.answer(f::FORGE, &f::ok(b"[]"));
    hints(&mut h);
    next_poll(&mut h);
    h.answer(f::FORGE, &page(&[row(2, b"issues", false), row(1, b"ping", false)], b""));
    h.answer(f::FORGE, &delivery(b"issues", ISSUE));
    hints(&mut h);
    assert_eq!(h.hints.len(), 1);
    assert_eq!(kept(&h), Some(expected(2)));
}

#[test]
fn failed_whole_delivery_hints_nothing_and_continues_poll() {
    let mut h = baseline(f::limits());
    next_poll(&mut h);
    h.answer(f::FORGE, &page(&[row(3, b"issues", false), row(2, b"issues", false), row(1, b"ping", false)], b""));
    h.answer(f::FORGE, &f::response(500, b"{}"));
    h.connected(f::FORGE);
    h.answer(f::FORGE, &delivery(b"issues", ISSUE));
    hints(&mut h);
    assert_eq!(h.hints.len(), 1);
    assert_eq!(kept(&h), Some(expected(3)));
}

#[test]
fn mint_interleaves_between_log_deliveries_while_call_pays_only_its_read() {
    use skein_forge::api::{Answer, Op, Read};
    let mut h = baseline(f::limits());
    f::github_account(&mut h, 1, 22, b"acme", start::FULL);
    f::github_repository(&mut h, f::REPO, 22);
    next_poll(&mut h);
    h.answer(f::FORGE, &page(&[row(3, b"issues", false), row(2, b"issues", false), row(1, b"ping", false)], b""));
    let before = h.all_request_bytes(f::FORGE);
    h.component.test_github_mut().rows.get_mut(&1).expect("installation").full.pending = true;
    h.call(1, f::REPO, Op::Read(Read::Branch { branch: Box::from(b"main".as_slice()) }));
    h.run(Duration::ZERO);
    // The new call opens a pooled connection; the log owns the reserved one.
    assert_eq!(h.connections_opened(), 2);
    assert_eq!(requests(&h, b"POST /app/installations/1/access_tokens"), 0);
    h.connected(f::FORGE);
    h.answer(f::FORGE, &f::ok(f::GITHUB_BRANCH));
    hints(&mut h);
    let answer = h.answered(1).expect("call answer");
    assert_eq!((answer.cost, &answer.result), (1, &Ok(Answer::Commit([0; 32]))));
    // The harness's ordinary answer helper chooses the newest connection. Close
    // that idle call connection so the scripted log answers reach the own one.
    h.run(h.env.limits.idle);
    h.answer(f::FORGE, &delivery(b"issues", ISSUE));
    hints(&mut h);
    let wire = h.all_request_bytes(f::FORGE);
    assert!(wire.len() > before.len());
    assert_eq!(requests(&h, b"POST /app/installations/1/access_tokens"), 1);
    start::full(&mut h);
    hints(&mut h);
    assert_eq!(requests(&h, b"GET /app/hook/deliveries/2 "), 1);
    h.answer(f::FORGE, &delivery(b"issues", ISSUE));
    hints(&mut h);
    assert_eq!(kept(&h), Some(expected(3)));
    assert_eq!(h.hints.len(), 2);
}

#[test]
#[expect(
    clippy::print_stdout,
    clippy::disallowed_macros,
    reason = "report the puller's measured peak beside its checked ledger"
)]
fn puller_holding_a_hint_fits_its_counting_allocator_ledger() {
    use super::github_files::CallHeap;
    let mut h = baseline(f::limits());
    let action = f::filled(usize::try_from(h.env.limits.string_bytes).expect("string bytes"), b'a');
    let count = h.env.limits.inner.min(skein_forge::github::pin::PAGE_CAP);
    let mut rows = List::with_capacity(count);
    for _ in 0..count {
        rows.push(joined(&[
            b"{\"id\":2,\"guid\":\"00000000-0000-0000-0000-000000000002\",\"event\":\"issues\",\"action\":\"",
            &action,
            b"\",\"status_code\":200,\"delivered_at\":\"",
            DATE,
            b"\",\"redelivery\":false}",
        ]))
        .expect("full page");
    }
    let page = page(rows.as_slice(), NEXT);
    let body = f::filled(usize::try_from(h.env.limits.body_bytes).expect("body bytes"), b'b');
    let response = delivery(
        b"issues",
        &joined(&[
            br#"{"action":"opened","issue":{"number":3,"body":""#,
            &body,
            br#""},"repository":{"id":7,"owner":{"login":""#,
            &[b'a'; 39],
            br#""},"name":""#,
            &[b'w'; 100],
            br#""}}"#,
        ]),
    );
    next_poll(&mut h);
    h.answer(f::FORGE, &page);
    // Price the existing page, decoder and request before measuring only
    // component calls. The wire's fragment trace belongs to the harness.
    let held = u64::from(crate::limits::log_page_bytes(&h.env.limits).expect("page ledger"))
        .checked_add(crate::limits::decoder_heap(&h.env.limits, super::GITHUB).expect("decoder ledger"))
        .unwrap()
        .checked_add(u64::try_from(size_of::<skein_forge::github::plans::start::LogDelivery>()).unwrap())
        .unwrap()
        .checked_add(u64::try_from(size_of::<skein_forge::github::plans::plan::Exchange>()).unwrap())
        .unwrap()
        .checked_add(u64::from(h.env.limits.string_bytes))
        .unwrap();
    h.component_heap = Some(CallHeap { peak: 0, held: i64::try_from(held).unwrap() });
    h.answer(f::FORGE, &response);
    let hinted = match h.up.pop().expect("hint") {
        Event::Hint(hint) => hint,
        Event::Answered(_)
        | Event::Installation { .. }
        | Event::Fact { .. }
        | Event::GitToken { .. }
        | Event::Listening { .. }
        | Event::ListenerLost
        | Event::Connected { .. }
        | Event::Stopped { .. }
        | Event::Closed => unreachable!("hint"),
    };
    h.send(Request::RefusedHint(hinted));
    h.run(Duration::ZERO);
    let heap = h.component_heap.take().expect("component meter");
    match h.component.test_github().puller.state {
        State::Holding { hint: Some(_), offer: false } => {}
        State::Resting | State::Reading | State::Delivery { .. } | State::Holding { .. } => {
            unreachable!("held hint at peak")
        }
    }
    let bound = crate::limits::puller_heap(&h.env.limits).expect("puller ledger");

    assert!(u64::try_from(heap.held).expect("positive held") <= bound, "{} > {bound}", heap.held);
    // A growing parser table holds its old and new buffers at the realloc peak.
    assert!(u64::try_from(heap.peak).expect("positive peak") <= 2 * bound, "{} > 2 * {bound}", heap.peak);
    assert!(
        u64::try_from(heap.peak).expect("positive peak")
            < List::<skein_json::tokenizer::Event>::worst_case(h.env.limits.delivery_bytes).expect("event buffer size")
    );
    println!("puller holding hint: peak {} <= twice puller heap {bound}", heap.peak);
}

#[test]
fn due_poll_waits_for_mint_without_leaving_a_past_alarm() {
    let mut h = baseline(f::limits());
    f::github_account(&mut h, 1, 22, b"acme", start::FULL);
    h.component.test_github_mut().rows.get_mut(&1).expect("installation").full.pending = true;
    h.run(Duration::from_nanos(h.env.limits.log_poll.as_nanos().saturating_sub(Duration::from_secs(1).as_nanos())));
    h.run(Duration::from_secs(1));
    assert_eq!(h.component.test_github().puller.due, None);
    assert!(h.component.next_deadline().expect("mint deadline") > h.env.now);
    assert_eq!(requests(&h, b"GET /app/hook/deliveries?"), 1);
    start::full(&mut h);
    hints(&mut h);
    assert_eq!(requests(&h, b"GET /app/hook/deliveries?"), 2);
}

#[test]
fn log_limits_are_positive_and_smallest_values_are_admitted() {
    for limits in [
        Limits { log_poll: Duration::ZERO, ..f::limits() },
        Limits { log_catch_up: Duration::ZERO, ..f::limits() },
        Limits { log_pages: 0, ..f::limits() },
        Limits { delivery_bytes: 0, ..f::limits() },
    ] {
        f::admission(f::github_settings(), &limits, Err(crate::Unusable::Zero), b"zero log limit");
    }
    let limits = Limits {
        log_poll: Duration::from_nanos(1),
        log_catch_up: Duration::from_nanos(1),
        log_pages: 1,
        delivery_bytes: 1,
        ..f::limits()
    };
    f::admission(f::github_settings(), &limits, Ok(()), b"positive log limits");
}

#[test]
fn unseen_whole_delivery_failure_skips_it_and_reads_the_next_row() {
    let mut h = baseline(f::limits());
    next_poll(&mut h);
    h.answer(
        f::FORGE,
        &page(&[row(3, b"issues", false), row(2, b"issues", false), row(1, b"ping", false)], b"Connection: close\r\n"),
    );
    h.refuse(f::FORGE);
    h.connected(f::FORGE);
    h.answer(f::FORGE, &delivery(b"issues", ISSUE));
    hints(&mut h);
    assert_eq!(h.hints.len(), 1);
    assert_eq!(kept(&h), Some(expected(3)));
    assert_eq!(requests(&h, b"GET /app/hook/deliveries/3 "), 0);
    assert_eq!(requests(&h, b"GET /app/hook/deliveries/2 "), 1);
}

#[test]
fn a_delivery_disagreeing_with_its_row_hints_nothing_and_the_next_row_hints() {
    let mut h = baseline(f::limits());
    next_poll(&mut h);
    h.answer(f::FORGE, &page(&[row(3, b"issues", false), row(2, b"issues", false), row(1, b"issues", false)], NEW_TAG));
    h.answer(f::FORGE, &delivery(b"issue_comment", ISSUE));
    hints(&mut h);
    assert!(h.hints.is_empty());
    h.connected(f::FORGE);
    assert_eq!(requests(&h, b"GET /app/hook/deliveries/2 "), 1);
    h.answer(f::FORGE, &delivery(b"issues", ISSUE));
    hints(&mut h);
    assert_eq!(h.hints.len(), 1);
    assert_eq!(kept(&h), Some(expected(3)));
}

#[test]
fn puller_heap_does_not_grow_with_the_delivery_byte_bound() {
    let small = Limits { delivery_bytes: 64 * 1024, ..f::limits() };
    let large = Limits { delivery_bytes: 1024 * 1024, ..small };
    assert_eq!(crate::limits::puller_heap(&small), crate::limits::puller_heap(&large));
}

#[test]
fn an_overlong_repository_path_hints_nothing_and_the_next_row_hints() {
    for (owner_len, name_len) in [(40, 7), (4, 101)] {
        let mut h = baseline(f::limits());
        next_poll(&mut h);
        h.answer(
            f::FORGE,
            &page(&[row(3, b"issues", false), row(2, b"issues", false), row(1, b"ping", false)], NEW_TAG),
        );
        let owner = f::filled(owner_len, b'a');
        let name = f::filled(name_len, b'w');
        let bad = joined(&[
            br#"{"action":"opened","issue":{"number":3,"body":"hello"},"repository":{"id":7,"owner":{"login":""#,
            &owner,
            br#""},"name":""#,
            &name,
            br#""}}"#,
        ]);
        h.answer(f::FORGE, &delivery(b"issues", &bad));
        hints(&mut h);
        assert!(h.hints.is_empty());
        h.connected(f::FORGE);
        assert_eq!(requests(&h, b"GET /app/hook/deliveries/2 "), 1);
        h.answer(f::FORGE, &delivery(b"issues", ISSUE));
        hints(&mut h);
        assert_eq!(h.hints.len(), 1);
        assert_eq!(kept(&h), Some(expected(3)));
    }
}
