//! A finite search belongs to its original call, credential selection and deadline.
use super::fixtures::{self as f, FORGE, Harness, REPO};
use crate::{Request, installations::Tokens};
use alloc::boxed::Box;
use skein_forge::api::{self, Answer, Error, Op, Read};
use skein_lib::{Duration, Secret, Time};

const ONE: &[u8] = b"Bearer artificial-full-one";
const TWO: &[u8] = b"Bearer artificial-full-two";
const THREE: &[u8] = b"Bearer artificial-full-three";
fn branch() -> Op {
    Op::Read(Read::Branch { branch: Box::from(b"main".as_slice()) })
}
fn candidates() -> Harness {
    let mut h = f::github_calls();
    f::github_account(&mut h, 2, 23, b"other", b"artificial-full-two");
    h.component.test_github_mut().recent = Some(2);
    h
}
fn begin(h: &mut Harness, op: Op) {
    h.call(1, REPO, op);
    h.run(Duration::ZERO);
    h.connected(FORGE);
}
fn next(h: &mut Harness) {
    h.run(Duration::ZERO);
    h.connected(FORGE);
}
fn answered(h: &mut Harness, cost: u32, result: Result<Answer, Error>) {
    h.run(Duration::ZERO);
    let answer = h.answered(1).expect("one terminal");
    assert_eq!((answer.cost, &answer.result), (cost, &result));
}
fn credential(h: &Harness, expected: &[u8]) {
    assert!(f::header(&h.request_bytes(FORGE), b"Authorization: ") == expected, "selected credential");
}
fn count(h: &Harness, request: &[u8]) -> u32 {
    skein_lib::bytes::count(&h.all_request_bytes(FORGE), request, u32::MAX)
}
fn settings() -> Answer {
    Answer::Settings(api::Settings {
        default_branch: Box::from(b"main".as_slice()),
        merge: true,
        squash: true,
        rebase: false,
    })
}
fn found(h: &mut Harness, cost: u32) {
    h.answer(FORGE, &f::ok(f::GITHUB_REPOSITORY));
    h.run(Duration::ZERO);
    h.answer(FORGE, &f::ok(f::GITHUB_BRANCH));
    answered(h, cost, Ok(Answer::Commit([0; 32])));
}
#[test]
fn unknown_repository_tries_recent_then_another_installation() {
    let mut h = candidates();
    begin(&mut h, branch());
    credential(&h, TWO);
    h.answer(FORGE, &f::status(404));
    next(&mut h);
    credential(&h, ONE);
    found(&mut h, 3);
    assert_eq!(count(&h, b"GET /repositories/1 "), 2);
    assert_eq!(count(&h, ONE), 2);
    assert_eq!(count(&h, TWO), 1);
}
#[test]
fn all_installation_probes_404_is_missing() {
    let mut h = candidates();
    begin(&mut h, branch());
    h.answer(FORGE, &f::status(404));
    next(&mut h);
    h.answer(FORGE, &f::status(404));
    answered(&mut h, 2, Err(Error::Missing));
    assert_eq!(count(&h, ONE), 1);
    assert_eq!(count(&h, TWO), 1);
    assert_eq!(h.answers.len(), 1);
}
#[test]
fn single_installation_probe_404_is_missing() {
    let mut h = f::github_calls();
    begin(&mut h, branch());
    h.answer(FORGE, &f::status(404));
    answered(&mut h, 1, Err(Error::Missing));
    assert_eq!(count(&h, b"GET /repositories/1 "), 1);
}
#[test]
fn successful_probe_routes_the_call_by_owner_account() {
    let mut h = candidates();
    begin(&mut h, branch());
    credential(&h, TWO);
    found(&mut h, 2);
    assert_eq!(count(&h, b"GET /repositories/1 "), 1);
    assert_eq!(count(&h, ONE), 1);
    assert_eq!(count(&h, TWO), 1);
}
#[test]
fn second_installation_settings_needs_no_extra_read() {
    let mut h = candidates();
    begin(&mut h, Op::Read(Read::Settings));
    h.answer(FORGE, &f::status(404));
    next(&mut h);
    h.answer(FORGE, &f::ok(f::GITHUB_REPOSITORY));
    answered(&mut h, 2, Ok(settings()));
    assert_eq!(count(&h, b"GET /repositories/1 "), 2);
}
#[test]
fn probe_only_advances_after_a_404() {
    let wrong_id = br#"{"id":2,"owner":{"id":22,"login":"acme","type":"Organization"},"name":"widgets","default_branch":"main","html_url":"https://github.com/acme/widgets","allow_merge_commit":true,"allow_squash_merge":true,"allow_rebase_merge":false}"#;
    for (label, response, expected) in [
        ("403", f::response(403, b"{}"), Error::Forbidden),
        ("410", f::status(410), Error::Missing),
        (
            "429",
            f::response_with_fields(429, b"{}", b"Retry-After: 2\r\n"),
            Error::RateLimited { after: Duration::from_secs(2) },
        ),
        ("500", f::status(500), Error::Timeout),
        ("malformed", f::ok(b"{"), Error::InvalidAnswer),
        ("wrong id", f::ok(wrong_id), Error::InvalidAnswer),
    ] {
        let mut h = candidates();
        begin(&mut h, branch());
        h.answer(FORGE, &response);
        h.run(Duration::ZERO);
        let answer = h.answered(1).expect(label);
        assert_eq!((answer.cost, &answer.result), (1, &Err(expected)), "{label}");
        assert_eq!(count(&h, b"GET /repositories/1 "), 1, "{label}");
        assert!(h.component.github_tables().repository(REPO).is_none());
    }
}
#[test]
fn bounded_probe_ends_without_another_candidate() {
    let mut h = candidates();
    h.env.limits.body_bytes_in = 8;
    begin(&mut h, branch());
    h.answer(FORGE, &f::ok(f::GITHUB_REPOSITORY));
    answered(&mut h, 1, Err(Error::InvalidAnswer));
    assert_eq!(count(&h, b"GET /repositories/1 "), 1);
}
#[test]
fn probe_401_renews_once_without_searching_or_replaying() {
    let mut h = candidates();
    begin(&mut h, branch());
    h.answer(FORGE, &f::status(401));
    answered(&mut h, 1, Err(Error::Forbidden));
    assert_eq!(h.component.test_github().rows.get(&2).expect("row").refused_revision, Some(1));
    h.run(Duration::ZERO);
    assert_eq!(h.component.test_github().mint, Some((2, skein_forge::github::request::Scope::Full)));
    assert_eq!(count(&h, b"GET /repositories/1 "), 1);
}
#[test]
fn unseen_probe_failure_costs_zero() {
    let mut h = candidates();
    h.call(1, REPO, branch());
    h.run(Duration::ZERO);
    h.refuse(FORGE);
    answered(&mut h, 0, Err(Error::Unavailable));
    assert_eq!(h.connections_opened(), 1);
}
#[test]
fn seen_probe_failure_costs_one_and_does_not_search() {
    let mut h = candidates();
    begin(&mut h, branch());
    h.reset(FORGE);
    answered(&mut h, 1, Err(Error::Timeout));
    assert_eq!(count(&h, b"GET /repositories/1 "), 1);
}
#[test]
fn known_owner_404_does_not_search_other_installations() {
    let mut h = candidates();
    f::github_repository(&mut h, REPO, 22);
    begin(&mut h, branch());
    h.answer(FORGE, &f::status(404));
    answered(&mut h, 1, Err(Error::Missing));
    assert_eq!(count(&h, b"GET /repositories/1 "), 0);
    assert_eq!(count(&h, TWO), 0);
}
#[test]
fn ordinary_step_404_after_discovery_does_not_search() {
    let mut h = candidates();
    begin(&mut h, branch());
    h.answer(FORGE, &f::ok(f::GITHUB_REPOSITORY));
    h.run(Duration::ZERO);
    h.answer(FORGE, &f::status(404));
    answered(&mut h, 2, Err(Error::Missing));
    assert_eq!(count(&h, b"GET /repositories/1 "), 1);
}
#[test]
fn missing_repository_is_searched_again_on_the_next_call() {
    let mut h = candidates();
    begin(&mut h, branch());
    h.answer(FORGE, &f::status(404));
    next(&mut h);
    h.answer(FORGE, &f::status(404));
    answered(&mut h, 2, Err(Error::Missing));
    h.call(2, REPO, branch());
    next(&mut h);
    credential(&h, ONE);
    h.answer(FORGE, &f::ok(f::GITHUB_REPOSITORY));
    h.run(Duration::ZERO);
    h.answer(FORGE, &f::ok(f::GITHUB_BRANCH));
    h.run(Duration::ZERO);
    let answer = h.answered(2).expect("fresh call");
    assert_eq!((answer.cost, &answer.result), (2, &Ok(Answer::Commit([0; 32]))));
}
fn waiting_on_second() -> Harness {
    let mut h = candidates();
    h.component.test_github_mut().rows.get_mut(&1).expect("row").full.expiry = Time::ZERO;
    begin(&mut h, branch());
    h.answer(FORGE, &f::status(404));
    h.run(Duration::ZERO);
    h
}
#[test]
fn discovery_keeps_its_original_deadline() {
    let mut h = waiting_on_second();
    h.run(Duration::from_secs(9));
    assert!(h.answered(1).is_none());
    assert_eq!(h.connections_for(FORGE), 0);
    h.run(Duration::from_secs(1));
    answered(&mut h, 1, Err(Error::Timeout));
    assert_eq!(count(&h, b"GET /repositories/1 "), 1);
    h.component.test_github_mut().rows.get_mut(&1).expect("row").full.expiry = Time::from_nanos(3_600_000_000_000);
    h.run(Duration::ZERO);
    assert_eq!(count(&h, b"GET /repositories/1 "), 1);
}
#[test]
fn released_full_preserves_the_cost_of_written_probes_only() {
    let mut h = waiting_on_second();
    h.run(Duration::from_secs(9));
    h.component.test_github_mut().rows.get_mut(&1).expect("row").full.expiry = Time::from_nanos(3_600_000_000_000);
    h.component.test_github_mut().rows.get_mut(&1).expect("row").api =
        Some(Secret::new(Box::from(b"renewed-full".as_slice())));
    next(&mut h);
    found(&mut h, 3);
}
#[test]
fn discovery_snapshot_does_not_repeat_after_recency_changes() {
    let mut h = candidates();
    f::github_account(&mut h, 3, 24, b"third", b"artificial-full-three");
    begin(&mut h, branch());
    for expected in [TWO, ONE, THREE] {
        credential(&h, expected);
        h.component.test_github_mut().recent = Some(2);
        h.answer(FORGE, &f::status(404));
        h.run(Duration::ZERO);
        if h.answered(1).is_none() {
            h.connected(FORGE);
        }
    }
    answered(&mut h, 3, Err(Error::Missing));
    assert_eq!(count(&h, ONE), 1);
    assert_eq!(count(&h, TWO), 1);
    assert_eq!(count(&h, THREE), 1);
}
#[test]
fn newly_accepted_candidates_belong_to_the_next_call() {
    let mut h = candidates();
    begin(&mut h, branch());
    f::github_account(&mut h, 3, 24, b"third", b"artificial-full-three");
    h.answer(FORGE, &f::status(404));
    next(&mut h);
    h.answer(FORGE, &f::status(404));
    answered(&mut h, 2, Err(Error::Missing));
    assert_eq!(count(&h, THREE), 0);
}
#[test]
fn gone_candidate_is_skipped_without_cost_or_stale_token() {
    let mut h = candidates();
    f::github_account(&mut h, 3, 24, b"third", b"artificial-full-three");
    begin(&mut h, branch());
    h.component.test_github_mut().rows.get_mut(&1).expect("row").forget();
    h.answer(FORGE, &f::status(404));
    next(&mut h);
    credential(&h, THREE);
    h.answer(FORGE, &f::status(404));
    answered(&mut h, 2, Err(Error::NotInstalled));
    assert_eq!(count(&h, ONE), 0);
}
#[test]
fn all_remaining_candidates_lost_is_not_installed_with_incurred_cost() {
    let mut h = candidates();
    begin(&mut h, branch());
    drop(h.component.test_github_mut().rows.remove(&1));
    h.answer(FORGE, &f::status(404));
    answered(&mut h, 1, Err(Error::NotInstalled));
}
#[test]
fn newly_suspended_candidate_is_forbidden_with_incurred_cost() {
    let mut h = candidates();
    begin(&mut h, branch());
    h.component.test_github_mut().rows.get_mut(&1).expect("row").suspended = true;
    h.answer(FORGE, &f::status(404));
    answered(&mut h, 1, Err(Error::Forbidden));
}
#[test]
fn unsent_waiting_candidate_is_skipped_after_reacceptance() {
    let mut h = waiting_on_second();
    h.component.test_github_mut().rows.get_mut(&1).expect("row").unminted();
    answered(&mut h, 1, Err(Error::NotInstalled));
    assert_eq!(count(&h, ONE), 0);
}
fn stale_success(h: &mut Harness) {
    h.answer(FORGE, &f::ok(f::GITHUB_REPOSITORY));
    answered(h, 1, Err(Error::Forbidden));
    assert!(h.component.github_tables().repository(REPO).is_none());
    assert_eq!(count(h, b"GET /repositories/1 "), 1);
}
#[test]
fn replacement_cannot_publish_a_written_probe_success() {
    let mut h = candidates();
    begin(&mut h, branch());
    h.component.test_github_mut().forget();
    stale_success(&mut h);
}
#[test]
fn loss_reacceptance_cannot_publish_a_written_probe_success() {
    let mut h = candidates();
    begin(&mut h, branch());
    let row = h.component.test_github_mut().rows.get_mut(&2).expect("row");
    row.forget();
    row.unminted();
    stale_success(&mut h);
}
#[test]
fn revoked_404_cannot_start_another_probe() {
    let mut h = candidates();
    begin(&mut h, branch());
    h.component.test_github_mut().rows.get_mut(&2).expect("row").forget();
    h.answer(FORGE, &f::status(404));
    answered(&mut h, 1, Err(Error::Forbidden));
    assert_eq!(count(&h, ONE), 0);
}
fn closed(h: &mut Harness) {
    h.send(Request::Close);
    h.run(Duration::ZERO);
    assert!(h.closed);
    assert!(h.answered(1).is_none());
    h.run(Duration::ZERO);
}
#[test]
fn close_during_discovery_token_wait_has_no_terminal() {
    closed(&mut waiting_on_second());
}
#[test]
fn close_during_a_written_probe_has_no_terminal() {
    let mut h = candidates();
    begin(&mut h, branch());
    closed(&mut h);
}
#[test]
fn close_during_discovery_continuation_has_no_terminal() {
    let mut h = candidates();
    begin(&mut h, branch());
    h.answer(FORGE, &f::ok(f::GITHUB_REPOSITORY));
    h.run(Duration::ZERO);
    closed(&mut h);
}
#[test]
fn discovered_answer_is_offered_once_per_offer() {
    let mut h = candidates();
    begin(&mut h, Op::Read(Read::Settings));
    h.answer(FORGE, &f::ok(f::GITHUB_REPOSITORY));
    answered(&mut h, 1, Ok(settings()));
    let answer = h.answers.remove(&1).expect("answer");
    h.send(Request::Refused(answer));
    h.send(Request::Offer);
    answered(&mut h, 1, Ok(settings()));
    drop(h.answers.remove(&1));
    h.send(Request::Offer);
    h.run(Duration::ZERO);
    assert!(h.answered(1).is_none());
}
#[test]
fn returned_discovered_answer_stays_unoffered() {
    let mut h = candidates();
    begin(&mut h, Op::Read(Read::Settings));
    h.answer(FORGE, &f::ok(f::GITHUB_REPOSITORY));
    answered(&mut h, 1, Ok(settings()));
    let answer = h.answers.remove(&1).expect("answer");
    h.send(Request::Returned(answer));
    h.send(Request::Offer);
    h.run(Duration::ZERO);
    assert!(h.answered(1).is_none());
}
#[test]
fn no_eligible_candidate_preserves_landed_admission_failure() {
    for state in [Tokens::Gone, Tokens::Current] {
        let mut h = f::github_calls();
        let row = h.component.test_github_mut().rows.get_mut(&1).expect("row");
        row.state = state;
        row.suspended = true;
        h.call(1, REPO, branch());
        answered(&mut h, 0, Err(Error::NotInstalled));
        assert_eq!(h.connections_opened(), 0);
    }
}

#[test]
fn released_renewal_requests_add_no_call_cost() {
    let mut h = super::github_renewal::renewing();
    f::github_account(&mut h, 2, 23, b"other", b"artificial-full-two");
    h.component.test_github_mut().recent = Some(2);
    h.component.test_github_mut().rows.get_mut(&1).expect("row").full.expiry = Time::ZERO;
    begin(&mut h, branch());
    h.answer(FORGE, &f::status(404));
    h.run(Duration::ZERO);
    assert!(h.answered(1).is_none());
    super::github_renewal::renewed_full(&mut h);
    h.connected(FORGE);
    found(&mut h, 3);
    assert_eq!(count(&h, b"POST /app/installations/1/access_tokens "), 3);
}
#[test]
fn a_404_head_discards_its_unread_exchange_before_the_next_probe() {
    let mut h = candidates();
    begin(&mut h, branch());
    h.answer(FORGE, b"HTTP/1.1 404 Not Found\r\nContent-Length: 1000\r\n\r\n");
    next(&mut h);
    credential(&h, ONE);
    found(&mut h, 3);
}
#[test]
fn eligibility_is_rechecked_after_connect_before_the_first_byte() {
    let mut h = candidates();
    h.call(1, REPO, branch());
    h.run(Duration::ZERO);
    h.component.test_github_mut().rows.get_mut(&2).expect("row").forget();
    h.connected(FORGE);
    credential(&h, ONE);
    found(&mut h, 2);
    assert_eq!(count(&h, TWO), 0);
}
#[test]
fn full_expiring_during_connect_waits_without_writing() {
    let mut h = candidates();
    h.call(1, REPO, branch());
    h.run(Duration::ZERO);
    h.component.test_github_mut().rows.get_mut(&2).expect("row").full.expiry = Time::ZERO;
    h.connected(FORGE);
    h.run(Duration::ZERO);
    assert_eq!(count(&h, b"GET /repositories/1 "), 0);
    assert_eq!(h.connections_for(FORGE), 1); // Released idle connection, with no holder.
    h.component.test_github_mut().rows.get_mut(&2).expect("row").full.expiry = Time::from_nanos(3_600_000_000_000);
    h.component.test_github_mut().rows.get_mut(&2).expect("row").api =
        Some(Secret::new(Box::from(b"renewed-full".as_slice())));
    h.run(Duration::ZERO);
    found(&mut h, 2);
}

#[test]
fn repository_id_above_the_vocabulary_bound_is_too_large() {
    const TOO_LARGE: &[u8] = br#"{"id":4294967296,"owner":{"id":22,"login":"acme","type":"Organization"},"name":"widgets","default_branch":"main","html_url":"https://github.com/acme/widgets","allow_merge_commit":true,"allow_squash_merge":true,"allow_rebase_merge":false}"#;
    let mut h = candidates();
    begin(&mut h, branch());
    h.answer(FORGE, &f::ok(TOO_LARGE));
    answered(&mut h, 1, Err(Error::TooLarge));
    assert_eq!(count(&h, b"GET /repositories/1 "), 1);
}

#[test]
fn discovery_waits_for_installation_changes_and_git_tokens() {
    use super::github_installation_loss::{commit, row};
    use crate::{
        Event,
        installations::{HeldToken, Selected},
    };
    use skein_lib::Queue;
    let mut h = waiting_on_second();
    let mut renamed = row(1, false);
    renamed.login = Box::from(b"renamed".as_slice());
    let mut other = row(2, false);
    other.account = 23;
    other.login = Box::from(b"other".as_slice());
    commit(&mut h, Box::from([renamed, other]));
    let gh = h.component.test_github_mut();
    gh.held = Some(HeldToken {
        selected: Selected { id: 1, connect: gh.connect, revision: gh.rows.get(&1).expect("row").revision },
        token: Secret::new(Box::from(b"artificial-contents".as_slice())),
        expires: skein_lib::Wall::from_nanos(3_600_000_000_000),
    });
    let mut up = Queue::uncapped();
    h.component.test_local(&h.env, &mut up);
    assert_eq!(up.len(), 1);
    assert!(h.component.test_github().held.is_some());
    match up.pop().expect("first lifecycle change") {
        Event::Installation { change, .. } => assert_eq!(change, crate::Change::Renamed),
        Event::GitToken { .. }
        | Event::Fact { .. }
        | Event::Answered(_)
        | Event::Hint(_)
        | Event::Listening { .. }
        | Event::ListenerLost
        | Event::Connected { .. }
        | Event::Stopped { .. }
        | Event::Closed => unreachable!("installation change"),
    }
    h.component.test_local(&h.env, &mut up);
    assert_eq!(up.len(), 1);
    assert!(h.component.test_github().held.is_none());
    match up.pop().expect("token follows changes") {
        Event::GitToken { .. } => {}
        Event::Installation { .. }
        | Event::Fact { .. }
        | Event::Answered(_)
        | Event::Hint(_)
        | Event::Listening { .. }
        | Event::ListenerLost
        | Event::Connected { .. }
        | Event::Stopped { .. }
        | Event::Closed => unreachable!("git token"),
    }
    h.component.test_local(&h.env, &mut up);
    assert!(up.is_empty());
    let row = h.component.test_github_mut().rows.get_mut(&1).expect("row");
    row.full.expiry = Time::from_nanos(3_600_000_000_000);
    row.api = Some(Secret::new(Box::from(b"renewed-full".as_slice())));
    next(&mut h);
    found(&mut h, 3);
}
#[test]
#[expect(clippy::disallowed_macros, reason = "inspect discovery snapshot Secret redaction")]
fn discovery_snapshots_keep_credentials_out_of_debug_and_request_lines() {
    let mut h = candidates();
    begin(&mut h, branch());
    let debug = std::format!("{:?} {:?}", h.component, h.up);
    for secret in [b"artificial-full-one".as_slice(), b"artificial-full-two"] {
        assert!(!super::github_start::contains(debug.as_bytes(), secret));
        for line in h.request_lines(FORGE).into_boxed() {
            assert!(!super::github_start::contains(&line, secret));
        }
    }
}
