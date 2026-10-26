use super::{fixtures as f, read_support as r};
use skein_fake_forge_domain::{self as fake, api as a};
use skein_forge::github::{self, plans::tables::Method, request::Request};
use skein_lib::Secret;

fn queue_config() -> crate::service::Config {
    f::config()
}
pub(super) fn harness(queue: bool) -> f::Harness {
    protected(queue, 0, if queue { Box::from([f::boxed(b"required")]) } else { Box::from([]) })
}
fn protected(queue: bool, approvals: u32, contexts: Box<[Box<[u8]>]>) -> f::Harness {
    protected_checks(queue, approvals, contexts, checks(Box::from([]), false))
}
fn checks(contexts: Box<[Box<[u8]>]>, fail: bool) -> a::Checks {
    a::Checks {
        contexts,
        latency_min: skein_lib::Duration::from_secs(45),
        latency_max: skein_lib::Duration::from_secs(45),
        silent: 0,
        passes: 1000,
        reruns: 0,
        cue: Some(a::Cue::Message { fail: f::boxed(if fail { b"feature" } else { b"FAIL" }) }),
    }
}
fn protected_checks(queue: bool, approvals: u32, contexts: Box<[Box<[u8]>]>, checks: a::Checks) -> f::Harness {
    let mut config = r::harness().env.limits;
    config.limits.name_bytes = 128;
    let mut h = f::Harness::with_checks(
        f::limits(),
        queue_config(),
        config,
        Some(a::Protection {
            linear: false,
            code_owners: false,
            bypassing_users: Box::from([]),
            queue,
            branch: f::boxed(b"trunk"),
            contexts,
            approvals,
            dismiss_stale: false,
        }),
        checks,
    );
    fake::grant(&mut h.domain, b"acme/widgets", f::BOT, a::Permission::Write);
    h
}
fn scenario(queue: bool) -> (f::Harness, u64, u64, Secret) {
    let mut h = harness(queue);
    let (number, head) = r::pull(&mut h);
    let token = r::token(&mut h);
    (h, number, head, token)
}
fn ask(h: &mut f::Harness, token: &Secret, request: &Request) -> crate::service::Response {
    let encoded = github::request::encode(request, &h.limits.github).unwrap();
    h.ask(encoded.method, &encoded.target, Some(token.expose()), encoded.body.as_deref().unwrap_or(b""))
}
fn merge(number: u64, head: u64) -> Request {
    Request::Merge {
        owner: f::boxed(b"acme"),
        name: f::boxed(b"widgets"),
        number,
        head: r::object(head),
        method: Method::Rebase,
    }
}
#[test]
fn direct_merge_of_a_queue_branch_lists_the_queue_and_unmet_check_without_merging() {
    let (mut h, number, head, token) = scenario(true);
    let response = ask(&mut h, &token, &merge(number, head));
    let message = b"Repository rule violations found\n\nChanges must be made through the merge queue\n\nRequired status check \"required\" is expected.\n\n";
    f::refusal(&h, &response, 405, message);
    assert!(h.domain.queue_entries(b"acme/widgets").unwrap().is_empty());
    let a::Answer::Pull(pull) = h.domain.inspect(&h.env.limits, b"acme/widgets", &a::Read::Pull { number }).unwrap()
    else {
        panic!("pull")
    };
    assert_eq!(pull.state, a::State::Open);
    assert_eq!(pull.merged, None);
}
#[test]
fn direct_queue_merge_lists_the_missing_approval_then_only_the_queue_after_approval() {
    let mut h = protected(true, 1, Box::from([]));
    let (number, head) = r::pull(&mut h);
    let token = r::token(&mut h);
    let response = ask(&mut h, &token, &merge(number, head));
    f::refusal(
        &h,
        &response,
        405,
        b"Repository rule violations found\n\nChanges must be made through the merge queue\n\nAt least 1 approving review is required by reviewers with write access.\n\n",
    );
    r::call_as(
        &mut h,
        f::BOT,
        a::Op::Write(a::Write::Review { number, verdict: Some(a::Verdict::Approve), body: f::boxed(b"approved") }),
    );
    let response = ask(&mut h, &token, &merge(number, head));
    f::refusal(
        &h,
        &response,
        405,
        b"Repository rule violations found\n\nChanges must be made through the merge queue\n\n",
    );
    assert!(h.domain.queue_entries(b"acme/widgets").unwrap().is_empty());
    let a::Answer::Pull(pull) = h.domain.inspect(&h.env.limits, b"acme/widgets", &a::Read::Pull { number }).unwrap()
    else {
        panic!("pull")
    };
    assert_eq!(pull.state, a::State::Open);
    assert_eq!(pull.merged, None);
}
#[test]
fn ordinary_base_merges_by_rest_beside_a_queue_base() {
    let (mut h, queued, head, token) = scenario(true);
    let first = h.first;
    r::call(&mut h, a::Op::Write(a::Write::CreateBranch { branch: f::boxed(b"direct"), commit: first }));
    let a::Answer::Created(direct) = r::call(
        &mut h,
        a::Op::Write(a::Write::OpenPull {
            title: f::boxed(b"direct merge"),
            body: f::boxed(b"ordinary base"),
            head: f::boxed(b"feature"),
            base: f::boxed(b"direct"),
            opens_empty: false,
        }),
    ) else {
        panic!("ordinary pull")
    };
    let response = ask(&mut h, &token, &merge(queued, head));
    let message = b"Repository rule violations found\n\nChanges must be made through the merge queue\n\nRequired status check \"required\" is expected.\n\n";
    f::refusal(&h, &response, 405, message);
    let skein_forge::api::Answer::Merged(sha) =
        drive_merge(&mut h, &token, direct, head, Admission::Answer, DIRECT).unwrap()
    else {
        panic!("REST merge returns its sha")
    };
    let a::Answer::Commit(landed) =
        h.domain.inspect(&h.env.limits, b"acme/widgets", &a::Read::Branch { branch: f::boxed(b"direct") }).unwrap()
    else {
        panic!("ordinary base moved")
    };
    assert_eq!(sha, r::object(landed));
    assert!(h.domain.queue_entries(b"acme/widgets").unwrap().is_empty());
}

#[test]
fn stale_head_is_refused_before_a_queue_requirement() {
    let (mut h, number, head, token) = scenario(true);
    let response = ask(&mut h, &token, &merge(number, head.checked_sub(1).unwrap()));
    f::refusal(&h, &response, 409, crate::texts::MERGE_STALE);
    assert!(h.domain.queue_entries(b"acme/widgets").unwrap().is_empty());
}

#[test]
fn queue_refusal_lists_approval_then_unmet_check_rules() {
    let mut h = protected(true, 1, Box::from([f::boxed(b"required")]));
    let (number, head) = r::pull(&mut h);
    let token = r::token(&mut h);
    let response = ask(&mut h, &token, &merge(number, head));
    let message = b"Repository rule violations found\n\nChanges must be made through the merge queue\n\nAt least 1 approving review is required by reviewers with write access.\n\nRequired status check \"required\" is expected.\n\n";
    f::refusal(&h, &response, 405, message);
    assert_eq!(
        drive_merge(&mut h, &token, number, head, Admission::Answer, QUEUED),
        Err(skein_forge::api::Error::Refused)
    );
    assert!(h.domain.queue_entries(b"acme/widgets").unwrap().is_empty());
}

// REST returns the sha directly; queue admission adds the node query and mutation.
const DIRECT: &[(&[u8], u16)] = &[(b"repository", 200), (b"merge", 200)];
const QUEUED: &[(&[u8], u16)] = &[(b"repository", 200), (b"merge", 405), (b"pull", 200), (b"enqueue", 200)];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Admission {
    Answer,
    MoveHead,
    LoseAnswer,
}

fn drive_merge(
    h: &mut f::Harness,
    token: &Secret,
    number: u64,
    head: u64,
    admission: Admission,
    expected: &[(&[u8], u16)],
) -> Result<skein_forge::api::Answer, skein_forge::api::Error> {
    use skein_forge::{
        api,
        github::plans::{
            Failure, Heard,
            plan::{Plan, Step},
            tables::Tables,
        },
    };
    // Repository resolution and the three merge exchanges are the longest route.
    const EXCHANGES: u32 = 4;
    let mut plan = Plan::new(
        api::Repository { forge: 1, repository: 2 },
        api::Op::Write(api::Write::Merge { number, head: r::object(head) }),
    );
    let mut tables = Tables::new(4, 8);
    let mut step = plan.begin(r::bounds(), b"example-app", &mut tables).unwrap();
    let mut requests = skein_lib::List::with_capacity(EXCHANGES);
    for _exchange in 0..=EXCHANGES {
        let exchange = match step {
            Step::Done(result) => {
                assert_eq!(requests.as_slice(), expected);
                return result;
            }
            Step::Redirect { .. } => panic!("this plan uses only API exchanges"),
            Step::Send(exchange) => exchange,
        };
        let label: &[u8] = if let Request::Repository { .. } = &exchange.request {
            b"repository"
        } else if let Request::Merge { .. } = &exchange.request {
            b"merge"
        } else if let Request::PullGraphql { .. } = &exchange.request {
            b"pull"
        } else if let Request::Enqueue { head: expected, .. } = &exchange.request {
            assert_eq!(*expected, r::object(head));
            if admission == Admission::MoveHead {
                fake::advance(&mut h.domain, &h.env, b"acme/widgets", b"feature", b"one", b"moved", 1).unwrap();
            }
            b"enqueue"
        } else {
            panic!("unexpected merge request")
        };
        let response = ask(h, token, &exchange.request);
        requests.push((label, response.status)).unwrap();
        let mut events = r::response_events(h, &exchange.request, response);
        if label == b"enqueue" && admission == Admission::LoseAnswer {
            // The domain admitted it; none of its HTTP answer reaches the plan.
            events.clear();
            events.push(Heard::Failed(Failure::Seen)).unwrap();
        }
        let mut next = None;
        for event in events.into_boxed() {
            next = plan.heard(event, r::bounds(), b"example-app", &mut tables).unwrap();
            if next.is_some() {
                break;
            }
        }
        step = next.expect("exchange advances plan");
    }
    panic!("merge plan settles within its four exchanges")
}

#[test]
fn merge_plan_enters_the_queue_after_rest_refusal_and_duplicate_calls_keep_one_entry() {
    let mut h = protected_checks(
        true,
        0,
        Box::from([f::boxed(b"required")]),
        checks(Box::from([f::boxed(b"required")]), false),
    );
    let (number, head) = r::pull(&mut h);
    let token = r::token(&mut h);
    h.drain_timers = false;
    for _attempt in 0..2_u32 {
        assert_eq!(
            drive_merge(&mut h, &token, number, head, Admission::Answer, QUEUED),
            Ok(skein_forge::api::Answer::Queued)
        );
    }
    assert_eq!(h.domain.queue_entries(b"acme/widgets").unwrap(), [a::QueueEntry { number, head, position: 1 }]);
}

#[test]
fn merge_plan_observes_a_head_moved_between_the_pull_query_and_enqueue_as_stale() {
    let (mut h, number, head, token) = scenario(true);
    assert_eq!(
        drive_merge(&mut h, &token, number, head, Admission::MoveHead, QUEUED),
        Err(skein_forge::api::Error::Stale)
    );
    assert!(h.domain.queue_entries(b"acme/widgets").unwrap().is_empty());
}

#[test]
fn lost_enqueue_answer_recovery_and_a_repeated_merge_keep_one_admission() {
    let mut h = protected_checks(
        true,
        0,
        Box::from([f::boxed(b"required")]),
        checks(Box::from([f::boxed(b"required")]), false),
    );
    let (number, head) = r::pull(&mut h);
    let token = r::token(&mut h);
    h.drain_timers = false;
    assert_eq!(
        drive_merge(&mut h, &token, number, head, Admission::LoseAnswer, QUEUED),
        Err(skein_forge::api::Error::Timeout)
    );
    let skein_forge::api::Answer::Pull(pull) =
        r::read(&mut h, &token, skein_forge::api::Read::Pull { number }).unwrap()
    else {
        panic!("recovery pull")
    };
    assert_eq!(pull.queued, Some(1));
    assert_eq!(pull.commit, r::object(head));
    assert_eq!(pull.merged, None);
    // Recovery reads adopt the queued entry. A fresh Merge call is idempotent too.
    assert_eq!(
        drive_merge(&mut h, &token, number, head, Admission::Answer, QUEUED),
        Ok(skein_forge::api::Answer::Queued)
    );
    assert_eq!(h.domain.queue_entries(b"acme/widgets").unwrap(), [a::QueueEntry { number, head, position: 1 }]);
}

fn running_queue(green: bool) -> (f::Harness, u64, u64, Secret) {
    let mut config = r::domain_config();
    config.limits.name_bytes = 128;
    // Base, pull head and group each retain their check run.
    config.limits.statuses = 3;
    let mut h = f::Harness::with_checks(
        f::limits(),
        queue_config(),
        config,
        Some(a::Protection {
            linear: false,
            code_owners: false,
            bypassing_users: Box::from([]),
            queue: true,
            branch: f::boxed(b"trunk"),
            contexts: Box::from([f::boxed(b"ci")]),
            approvals: 0,
            dismiss_stale: false,
        }),
        checks(Box::from([f::boxed(b"ci")]), false),
    );
    fake::grant(&mut h.domain, b"acme/widgets", f::BOT, a::Permission::Write);
    let parent = fake::commit(
        &mut h.domain,
        &config,
        h.first,
        Box::from([a::File { path: f::boxed(b"ci"), content: f::boxed(b"first") }]),
        b"first",
    )
    .unwrap()
    .unwrap();
    let head = fake::commit(
        &mut h.domain,
        &config,
        parent,
        Box::from([a::File { path: f::boxed(b"ci"), content: f::boxed(if green { b"green" } else { b"red" }) }]),
        b"feature",
    )
    .unwrap()
    .unwrap();
    r::call(&mut h, a::Op::Git(a::Git::Push { branch: f::boxed(b"feature"), commit: head, expected: None }));
    let a::Answer::Created(number) = r::call(
        &mut h,
        a::Op::Write(a::Write::OpenPull {
            title: f::boxed(if green { b"queue" } else { b"FAIL queue" }),
            body: f::boxed(b"group checks"),
            head: f::boxed(b"feature"),
            base: f::boxed(b"trunk"),
            opens_empty: false,
        }),
    ) else {
        panic!("pull")
    };
    let token = r::token(&mut h);
    h.drain_timers = false;
    (h, number, head, token)
}

fn advance_queue(h: &mut f::Harness) {
    let mut out = skein_lib::Queue::with_capacity(fake::MAX_OUT);
    for _ in 0..128_u32 {
        let Some(at) = h.domain.next_deadline() else {
            break;
        };
        h.env.now = at;
        fake::fire(&mut h.domain, &h.env, &mut out);
        out.pop();
        h.domain.reclaim();
    }
}

#[test]
fn merge_plan_queues_then_pull_plan_reads_the_landed_graphql_commit() {
    let (mut h, number, head, token) = running_queue(true);
    assert_eq!(
        drive_merge(&mut h, &token, number, head, Admission::Answer, QUEUED),
        Ok(skein_forge::api::Answer::Queued)
    );
    let a::Answer::Pull(admitted) =
        h.domain.inspect(&h.env.limits, b"acme/widgets", &a::Read::Pull { number }).unwrap()
    else {
        panic!("pull")
    };
    let group = admitted.group.unwrap();
    advance_queue(&mut h);
    let rest = ask(&mut h, &token, &Request::Pull { owner: f::boxed(b"acme"), name: f::boxed(b"widgets"), number });
    assert!(skein_lib::bytes::find(&rest.body, b"merge_commit_sha").is_none());
    let (_, github::document::Document::Pull(rest)) = h.decode(github::document::Kind::Pull, &rest) else {
        panic!("REST pull")
    };
    assert!(rest.merged);
    assert_eq!(rest.merge_commit, None);
    let skein_forge::api::Answer::Pull(pull) =
        r::read(&mut h, &token, skein_forge::api::Read::Pull { number }).unwrap()
    else {
        panic!("plan pull")
    };
    assert_eq!(pull.state, skein_forge::api::State::Closed);
    assert_eq!(pull.merged, Some(r::object(group)));
    assert_eq!(pull.queued, None);
    assert_eq!(pull.dequeued, None);
    let skein_forge::api::Answer::Checks(checks) =
        r::read(&mut h, &token, skein_forge::api::Read::Checks { commit: r::object(group) }).unwrap()
    else {
        panic!("group checks")
    };
    assert_eq!(checks.len(), 1);
    assert_eq!(checks.first().unwrap().check, skein_forge::api::Check::Passed);
}

#[test]
fn failed_group_leaves_pull_plan_open_with_ci_failure() {
    let (mut h, number, head, token) = running_queue(false);
    assert_eq!(
        drive_merge(&mut h, &token, number, head, Admission::Answer, QUEUED),
        Ok(skein_forge::api::Answer::Queued)
    );
    advance_queue(&mut h);
    let skein_forge::api::Answer::Pull(pull) =
        r::read(&mut h, &token, skein_forge::api::Read::Pull { number }).unwrap()
    else {
        panic!("plan pull")
    };
    assert_eq!(pull.state, skein_forge::api::State::Open);
    assert_eq!(pull.queued, None);
    assert_eq!(pull.merged, None);
    assert_eq!(pull.dequeued.as_deref(), Some(b"CI_FAILURE".as_slice()));
}

#[test]
fn unapproved_enqueue_answers_the_recorded_graphql_text_without_an_entry() {
    for (count, message) in [
        (1, b"Pull request At least 1 approving review is required by reviewers with write access.".as_slice()),
        (2, b"Pull request At least 2 approving reviews are required by reviewers with write access."),
    ] {
        let mut h = protected(true, count, Box::from([]));
        let (number, head) = r::pull(&mut h);
        let token = r::token(&mut h);
        let request = Request::Enqueue {
            id: f::parts(&[b"PR_2_", skein_lib::Decimal::of(number).as_bytes()]),
            head: r::object(head),
        };
        let response = ask(&mut h, &token, &request);
        assert_eq!(response.status, 200);
        assert_eq!(
            response.body.as_ref(),
            f::parts(&[
                br#"{"data":{"enqueuePullRequest":null},"errors":[{"type":"UNPROCESSABLE","path":["enqueuePullRequest"],"message":""#,
                message,
                br#""}]}"#,
            ])
            .as_ref(),
        );
        let (rows, document) = h.decode(github::document::Kind::Enqueue, &response);
        assert_eq!(rows.len(), 0);
        assert_eq!(
            document,
            github::document::Document::Graphql(github::document::GraphqlRefusal {
                kind: f::boxed(b"UNPROCESSABLE"),
                message: f::boxed(message),
            })
        );
        assert!(h.domain.queue_entries(b"acme/widgets").unwrap().is_empty());
    }
}

#[test]
fn enqueue_requires_each_named_head_check_to_pass() {
    for context in [b"ci".as_slice(), b"build"] {
        for state in [None, Some(a::Check::Pending), Some(a::Check::Failed), Some(a::Check::Passed)] {
            let required = Box::from([f::boxed(context)]);
            let mut h = if state.is_none() {
                protected(true, 0, required)
            } else {
                protected_checks(
                    true,
                    0,
                    required,
                    checks(Box::from([f::boxed(context)]), state == Some(a::Check::Failed)),
                )
            };
            if state == Some(a::Check::Pending) {
                h.drain_timers = false;
            }
            let (number, head) = r::pull(&mut h);
            let token = r::token(&mut h);
            h.drain_timers = false;
            let request = Request::Enqueue {
                id: f::parts(&[b"PR_2_", skein_lib::Decimal::of(number).as_bytes()]),
                head: r::object(head),
            };
            let response = ask(&mut h, &token, &request);
            assert_eq!(response.status, 200);
            let (_, document) = h.decode(github::document::Kind::Enqueue, &response);
            if state == Some(a::Check::Passed) {
                assert_eq!(document, github::document::Document::Enqueued(1));
                assert_eq!(h.domain.queue_entries(b"acme/widgets").unwrap().len(), 1);
            } else {
                let github::document::Document::Graphql(error) = document else { panic!("check refusal") };
                assert_eq!(error.kind.as_ref(), b"UNPROCESSABLE");
                assert_eq!(
                    error.message,
                    f::parts(&[b"Pull request Required status check \"", context, b"\" is in progress."])
                );
                assert_eq!(
                    drive_merge(&mut h, &token, number, head, Admission::Answer, QUEUED),
                    Err(skein_forge::api::Error::Protected)
                );
                assert!(h.domain.queue_entries(b"acme/widgets").unwrap().is_empty());
            }
        }
    }
}
#[test]
fn group_commit_merges_after_workflow_ci_success() {
    let (mut h, number, head, token) = running_queue(true);
    assert_eq!(
        drive_merge(&mut h, &token, number, head, Admission::Answer, QUEUED),
        Ok(skein_forge::api::Answer::Queued)
    );
    let branches = ask(
        &mut h,
        &token,
        &Request::Branches { owner: f::boxed(b"acme"), name: f::boxed(b"widgets"), page: 1, per_page: 100 },
    );
    let (rows, _) = h.decode(github::document::Kind::BranchRows, &branches);
    let mut group = None;
    for row in &rows {
        if let github::document::Row::Branch(branch) = row
            && branch.starts_with(b"gh-readonly-queue/")
        {
            group = Some(branch.clone());
            break;
        }
    }
    let branch = group.expect("group branch");
    let response =
        ask(&mut h, &token, &Request::Branch { owner: f::boxed(b"acme"), name: f::boxed(b"widgets"), branch });
    let (_, github::document::Document::Branch(branch)) = h.decode(github::document::Kind::Branch, &response) else {
        panic!("group commit")
    };
    let commit = super::super::reads::count(&branch.commit).unwrap();
    assert_ne!(commit, head);
    advance_queue(&mut h);
    let skein_forge::api::Answer::Pull(pull) =
        r::read(&mut h, &token, skein_forge::api::Read::Pull { number }).unwrap()
    else {
        panic!("merged pull")
    };
    assert_eq!(pull.merged, Some(r::object(commit)));
}
