use skein_forge::api::{self, Answer as Value, Error, Op, Read};
use skein_forge_connection::{Access, Request, settings::Kind};
use skein_forge_machine_world::{
    fixtures::{self, FORGE},
    github,
    machine::{Answer, Machine, Pause, Response},
};
use skein_lib::{Duration, Secret, Time};

const SECOND: Duration = Duration::from_secs(1);
const REPO: api::Repository = api::Repository { forge: FORGE, repository: 2 };
const PAGE_ONE: &str = "GET /app/installations?per_page=100&page=1";
const PAGE_TWO: &str = "GET /app/installations?per_page=100&page=2";
const MINT: &str = "POST /app/installations/1/access_tokens";
const VIEWER: &str = "POST /graphql";
const LOCATE: &str = "GET /repos/acme/widgets";
const RESOLVE: &str = "GET /repositories/2";
const BRANCH: &str = "GET /repos/acme/widgets/branches/main";
const START: usize = 5;
fn world(accepted: &[u64], mut calls: Vec<Answer>) -> Machine {
    let mut script: Vec<_> = [PAGE_ONE, PAGE_TWO].into_iter().map(Answer::served).collect();
    if accepted.contains(&1) {
        script.extend([MINT, MINT, VIEWER].into_iter().map(Answer::served));
    }
    script.append(&mut calls);
    world_script(accepted, script)
}
fn world_script(accepted: &[u64], script: Vec<Answer>) -> Machine {
    let mut settings = fixtures::settings();
    let forge = settings.forges.get_mut(0).expect("forge");
    forge.kind = Kind::GitHub;
    forge.access = Some(Access {
        credential: skein_forge::credential::Credential::App { id: 7, key: Secret::new(Box::from(github::app_key())) },
        user: 9,
        webhook_secret: None,
    });
    let mut world = Machine::github(7, fixtures::limits(), settings, script);
    world.action(Request::Installations { forge: FORGE, accepted: Box::from(accepted) });
    world.run_for(SECOND);
    world
}
fn locate() -> Op {
    Op::Read(Read::Locate { owner: Box::from(b"acme".as_slice()), name: Box::from(b"widgets".as_slice()) })
}
fn branch() -> Op {
    Op::Read(Read::Branch { branch: Box::from(b"main".as_slice()) })
}
fn value(world: &Machine, call: u64, cost: u32, result: &Result<Value, Error>) {
    let answer = world.answered(call).expect("one terminal");
    assert_eq!((answer.cost, &answer.result), (cost, result));
}
fn located(world: &mut Machine) {
    world.call(1, api::Repository { repository: 0, ..REPO }, locate());
    world.run_for(SECOND);
    value(world, 1, 1, &Ok(Value::Located { repository: 2 }));
}
fn bytes(request: &'static str, body: &[u8]) -> Answer {
    Answer { request, response: Response::Bytes(fixtures::response(200, body)) }
}
#[test]
fn github_locate_fills_the_repository_owner_row() {
    let mut w = world(&[1], vec![Answer::served(LOCATE), Answer::served(BRANCH)]);
    located(&mut w);
    w.call(2, REPO, branch());
    w.run_for(SECOND);
    let commit = skein_fake_forge_protocol::routes::object(1).bytes;
    value(&w, 2, 1, &Ok(Value::Commit(commit)));
    assert_eq!(w.requests().len(), START + 2);
    assert!(w.script_done());
    assert!(w.close());
}
#[test]
fn github_branches_follow_all_linked_pages() {
    let mut w = world(
        &[1],
        vec![
            Answer::served(RESOLVE),
            Answer::served("GET /repos/acme/widgets/branches?per_page=8&page=1"),
            Answer::served("GET /repos/acme/widgets/branches?per_page=8&page=2"),
        ],
    );
    w.call(1, REPO, Op::Read(Read::Branches));
    w.run_for(SECOND);
    value(
        &w,
        1,
        3,
        &Ok(Value::Branches([b"alpha".as_slice(), b"feature/one", b"main"].into_iter().map(Box::from).collect())),
    );
    assert!(w.script_done());
    assert!(w.close());
}
const ISSUE: &[u8] = br#"{"number":7,"state":"open","user":{"id":9},"updated_at":"2026-10-20T08:00:00Z","title":"widgets","labels":[],"body":"description"}"#;
const COMMENTS: &[u8] = br#"[{"id":8,"user":{"id":9},"created_at":"2026-10-20T08:00:00Z","updated_at":"2026-10-20T08:00:00Z","body":"hello"}]"#;
const PULL: &[u8] = br#"{"number":7,"state":"open","merged":false,"merge_commit_sha":null,"mergeable":true,"head":{"ref":"feature","sha":"0000000000000000000000000000000000000000"},"base":{"ref":"main","sha":"1111111111111111111111111111111111111111"},"requested_reviewers":[]}"#;
const COMBINED: &[u8] = br#"{"state":"pending","total_count":0,"statuses":[]}"#;
const RUNS: &[u8] = br#"{"check_runs":[]}"#;
const GRAPHQL: &[u8] = br#"{"data":{"repository":{"pullRequest":{"id":"opaque-node","mergeCommit":null,"mergeQueueEntry":null,"timelineItems":{"nodes":[]}}}}}"#;
#[test]
fn github_item_reads_its_comments() {
    let mut w = world(
        &[1],
        vec![
            Answer::served(LOCATE),
            bytes("GET /repos/acme/widgets/issues/7", ISSUE),
            bytes("GET /repos/acme/widgets/issues/7/comments?per_page=1&page=1", COMMENTS),
        ],
    );
    located(&mut w);
    w.call(2, REPO, Op::Read(Read::Item { number: 7, after: 0 }));
    w.run_for(SECOND);
    let time =
        Time::from_nanos(skein_forge::forgejo::time::read(b"2026-10-20T08:00:00Z").expect("fixture time").as_nanos());
    value(
        &w,
        2,
        2,
        &Ok(Value::Item {
            item: api::Summary {
                labels: Box::from([]),
                number: 7,
                kind: api::Kind::Issue,
                state: api::State::Open,
                author: 9,
                key: None,
                title: Box::from(b"widgets".as_slice()),
                body: Box::from(b"description".as_slice()),
                updated: time,
            },
            comments: Box::from([api::Comment {
                id: 8,
                provenance: api::Provenance::Original,
                author: 9,
                created: time,
                revision: u64::from_be_bytes(
                    skein_lib::sha256::digest(b"hello")[..8].try_into().expect("digest prefix"),
                ),
                key: None,
                body: Box::from(b"hello".as_slice()),
            }]),
            more: false,
        }),
    );
    assert!(w.script_done());
    assert!(w.close());
}
#[test]
fn github_pull_reads_both_ci_sources() {
    let mut w = world(
        &[1],
        vec![
            Answer::served(LOCATE),
            bytes("GET /repos/acme/widgets/pulls/7", PULL),
            bytes(
                "GET /repos/acme/widgets/commits/0000000000000000000000000000000000000000/status?per_page=1&page=1",
                COMBINED,
            ),
            bytes(
                "GET /repos/acme/widgets/commits/0000000000000000000000000000000000000000/check-runs?filter=latest&per_page=1&page=1",
                RUNS,
            ),
            bytes(VIEWER, GRAPHQL),
        ],
    );
    located(&mut w);
    w.call(2, REPO, Op::Read(Read::Pull { number: 7 }));
    w.run_for(SECOND);
    let mut base = [0; 32];
    base[..20].fill(0x11);
    value(
        &w,
        2,
        4,
        &Ok(Value::Pull(api::Pull {
            reviewers: Box::from([]),
            number: 7,
            state: api::State::Open,
            head: Box::from(b"feature".as_slice()),
            base: Box::from(b"main".as_slice()),
            commit: [0; 32],
            base_commit: Some(base),
            merged: None,
            mergeable: true,
            ci: api::Ci::None,
            queued: None,
            dequeued: None,
        })),
    );
    assert!(w.script_done());
    assert!(w.close());
}
#[test]
fn github_unaccepted_account_is_not_installed_without_an_ordinary_request() {
    let mut w = world(&[], vec![]);
    w.call(1, api::Repository { repository: 0, ..REPO }, locate());
    w.run_for(SECOND);
    value(&w, 1, 0, &Err(Error::NotInstalled));
    assert_eq!(w.requests().len(), 2);
    assert!(w.close());
}
#[test]
fn github_accepted_suspended_account_is_forbidden_without_an_ordinary_request() {
    let mut w = world(&[3], vec![]);
    w.call(
        1,
        api::Repository { repository: 0, ..REPO },
        Op::Read(Read::Locate { owner: Box::from(b"other".as_slice()), name: Box::from(b"widgets".as_slice()) }),
    );
    w.run_for(SECOND);
    value(&w, 1, 0, &Err(Error::Forbidden));
    assert_eq!(w.requests().len(), 2);
    assert!(w.close());
}
#[test]
fn call_token_401_renews_without_replaying_the_call() {
    let mut w = world(
        &[1],
        vec![
            Answer::served(LOCATE),
            Answer { request: BRANCH, response: Response::Bytes(fixtures::response(401, b"{}")) },
            Answer::served(MINT),
            Answer::served(MINT),
            Answer::served(BRANCH),
        ],
    );
    located(&mut w);
    w.call(2, REPO, branch());
    w.run_for(SECOND);
    value(&w, 2, 1, &Err(Error::Forbidden));
    assert_eq!(w.git_tokens().len(), 2);
    w.call(3, REPO, branch());
    w.run_for(SECOND);
    let commit = skein_fake_forge_protocol::routes::object(1).bytes;
    value(&w, 3, 1, &Ok(Value::Commit(commit)));
    assert_eq!(w.requests().len(), START + 5);
    assert!(w.script_done());
    assert!(w.close());
}
#[test]
fn renewal_preserves_calls_until_old_full_expires() {
    let mut w = world(
        &[1],
        vec![
            Answer::served(LOCATE),
            Answer {
                request: MINT,
                response: Response::Bytes(
                    b"HTTP/1.1 429 Rate\r\nRetry-After: 55\r\nContent-Length: 2\r\n\r\n{}".to_vec(),
                ),
            },
            Answer::served(BRANCH),
            Answer::served(MINT),
            Answer::served(BRANCH),
            Answer::served(MINT),
        ],
    );
    located(&mut w);
    let due = Time::from_nanos(Duration::from_secs(3540).as_nanos());
    w.advance_to(due);
    assert_eq!(w.requests().len(), START + 2, "renewal has answered with a wait");
    w.call(2, REPO, branch());
    w.advance_to(due);
    let commit = skein_fake_forge_protocol::routes::object(1).bytes;
    value(&w, 2, 1, &Ok(Value::Commit(commit)));
    // A later retry starts just before expiry; hold its response within its deadline.
    w.pause(Pause::Reading(0));
    w.advance_to(Time::from_nanos(Duration::from_secs(3595).as_nanos()));
    assert_eq!(w.requests().len(), START + 4, "retry mint is held");
    w.advance_to(Time::from_nanos(Duration::from_secs(3600).as_nanos()));
    w.call(3, REPO, branch());
    w.advance_to(Time::from_nanos(Duration::from_secs(3600).as_nanos()));
    assert!(w.answered(3).is_none());
    w.release_response();
    w.run_for(SECOND);
    value(&w, 3, 1, &Ok(Value::Commit(commit)));
    assert!(w.script_done());
    assert!(w.close());
}

#[test]
fn another_installations_call_proceeds_after_one_full_expires() {
    const MINT_TWO: &str = "POST /app/installations/2/access_tokens";
    const OTHER_LOCATE: &str = "GET /repos/example/other";
    const OTHER_BRANCH: &str = "GET /repos/example/other/branches/main";
    let mint = br#"{"token":"artificial-two","expires_at":"1970-01-12T18:06:40Z"}"#;
    // WALL_BASE is 1970-01-12T13:46:40Z; this second row lasts beyond the first hour.
    let other = br#"{"id":9,"owner":{"id":23,"login":"example","type":"User"},"name":"other","default_branch":"main","html_url":"https://github.com/example/other","allow_merge_commit":true,"allow_squash_merge":true,"allow_rebase_merge":false}"#;
    let branch = br#"{"name":"main","commit":{"sha":"0000000000000000000000000000000000000000"}}"#;
    let mut w = world_script(
        &[1, 2],
        vec![
            Answer::served(PAGE_ONE),
            Answer::served(PAGE_TWO),
            Answer::served(MINT),
            Answer::served(MINT),
            Answer { request: MINT_TWO, response: Response::Bytes(fixtures::response(201, mint)) },
            Answer { request: MINT_TWO, response: Response::Bytes(fixtures::response(201, mint)) },
            bytes(VIEWER, br#"{"data":{"viewer":{"databaseId":9,"login":"bot"}}}"#),
            Answer::served(LOCATE),
            bytes(OTHER_LOCATE, other),
            Answer {
                request: MINT,
                response: Response::Bytes(
                    b"HTTP/1.1 429 Rate\r\nRetry-After: 120\r\nContent-Length: 2\r\n\r\n{}".to_vec(),
                ),
            },
            bytes(OTHER_BRANCH, branch),
        ],
    );
    located(&mut w);
    w.call(
        2,
        api::Repository { repository: 0, ..REPO },
        Op::Read(Read::Locate { owner: Box::from(b"example".as_slice()), name: Box::from(b"other".as_slice()) }),
    );
    w.run_for(SECOND);
    w.advance_to(Time::from_nanos(Duration::from_secs(3540).as_nanos()));
    w.advance_to(Time::from_nanos(Duration::from_secs(3600).as_nanos()));
    w.call(3, REPO, self::branch());
    w.call(4, api::Repository { repository: 9, ..REPO }, self::branch());
    w.run_for(SECOND);
    assert!(w.answered(3).is_none());
    value(&w, 4, 1, &Ok(Value::Commit([0; 32])));
    assert!(w.script_done());
    assert!(w.close());
}

const TOOLS: api::Repository = api::Repository { forge: FORGE, repository: 4 };
const TOOLS_RESOLVE: &str = "GET /repositories/4";
const FEATURE: &str = "GET /repos/acme/tools/branches/feature";
const DIRECT_MERGE: &str = "PUT /repos/acme/tools/pulls/1/merge";
const QUEUE_MERGE: &str = "PUT /repos/acme/tools/pulls/2/merge";
// Startup's five requests, repository discovery and the feature-head read.
const MERGE_READY: usize = START + 2;
// REST merge, GQ pull and GQ enqueue are one call's three exchanges.
const MERGE_EXCHANGES: u32 = 3;
fn tools(calls: Vec<Answer>) -> (Machine, api::Commit) {
    let mut script = vec![Answer::served(TOOLS_RESOLVE), Answer::served(FEATURE)];
    script.extend(calls);
    let mut w = world(&[1], script);
    w.call(1, TOOLS, Op::Read(Read::Branch { branch: Box::from(b"feature".as_slice()) }));
    w.run_for(SECOND);
    let Ok(Value::Commit(head)) = w.answered(1).expect("feature head").result else { panic!("feature commit") };
    (w, head)
}
fn merge_call(w: &mut Machine, call: u64, number: u64, head: api::Commit) {
    w.call(call, TOOLS, Op::Write(api::Write::Merge { number, head }));
    w.run_for(SECOND);
}
fn queue_script() -> Vec<Answer> {
    [QUEUE_MERGE, VIEWER, VIEWER].into_iter().map(Answer::served).collect()
}
fn enqueue_count(w: &Machine) -> usize {
    w.requests()
        .iter()
        .filter(|request| request.windows(b"enqueuePullRequest".len()).any(|b| b == b"enqueuePullRequest"))
        .count()
}
#[test]
fn github_reviewers_add_and_remove_through_the_fake() {
    const PULL: &str = "GET /repos/acme/tools/pulls/1";
    const ADD: &str = "POST /repos/acme/tools/pulls/1/requested_reviewers";
    const REMOVE: &str = "DELETE /repos/acme/tools/pulls/1/requested_reviewers";
    let (mut w, _) = tools([PULL, "GET /user/31", ADD, PULL, REMOVE].into_iter().map(Answer::served).collect());
    for (call, reviewers, cost) in [(2, Box::from([31_u64].as_slice()), 3), (3, Box::from([]), 2)] {
        w.call(call, TOOLS, Op::Write(api::Write::SetReviewers { number: 1, reviewers }));
        w.run_for(SECOND);
        value(&w, call, cost, &Ok(Value::Done));
    }
    assert!(w.script_done());
    assert!(w.close());
}
#[test]
fn github_rest_merge_returns_the_fakes_landed_commit() {
    const BASE: &str = "GET /repos/acme/tools/branches/main";
    let (mut w, head) = tools(vec![Answer::served(DIRECT_MERGE), Answer::served(BASE)]);
    merge_call(&mut w, 2, 1, head);
    let Ok(Value::Merged(commit)) = w.answered(2).expect("merged").result else { panic!("merged commit") };
    assert_eq!(w.answered(2).expect("merged").cost, 1);
    w.call(3, TOOLS, Op::Read(Read::Branch { branch: Box::from(b"main".as_slice()) }));
    w.run_for(SECOND);
    value(&w, 3, 1, &Ok(Value::Commit(commit)));
    assert!(w.script_done());
    assert!(w.close());
}
#[test]
fn github_queue_merge_uses_three_requests_and_one_installation() {
    let (mut w, head) = tools(queue_script());
    merge_call(&mut w, 2, 2, head);
    value(&w, 2, MERGE_EXCHANGES, &Ok(Value::Queued));
    let requests = &w.requests()[MERGE_READY..];
    let authorizations: Vec<_> = requests
        .iter()
        .map(|request| {
            request.split(|b| *b == b'\n').find(|line| line.starts_with(b"Authorization: ")).expect("Full header")
        })
        .collect();
    assert_eq!(authorizations.len(), usize::try_from(MERGE_EXCHANGES).unwrap());
    assert!(authorizations.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(enqueue_count(&w), 1);
    assert!(w.script_done());
    assert!(w.close());
}
#[test]
fn github_lost_enqueue_is_recorded_once_across_reconnect() {
    for reset in [true, false] {
        let mut script = queue_script();
        script.push(Answer::served(FEATURE));
        let (mut w, head) = tools(script);
        // Drop the served enqueue's entire response, after the fake admits it.
        w.cut(skein_forge_machine_world::machine::Cut { request: MERGE_READY + 2, response: true, at: 0, reset });
        merge_call(&mut w, 2, 2, head);
        value(&w, 2, MERGE_EXCHANGES, &Err(Error::Timeout));
        w.call(3, TOOLS, Op::Read(Read::Branch { branch: Box::from(b"feature".as_slice()) }));
        w.run_for(SECOND);
        value(&w, 3, 1, &Ok(Value::Commit(head)));
        assert_eq!(enqueue_count(&w), 1);
        assert!(w.script_done());
        assert!(w.close());
    }
}
#[test]
fn github_merge_401_renews_full_once_for_a_later_call() {
    let (mut w, head) = tools(vec![
        Answer { request: DIRECT_MERGE, response: Response::Bytes(fixtures::response(401, b"")) },
        Answer::served(MINT),
        Answer::served(MINT),
        Answer::served(DIRECT_MERGE),
    ]);
    merge_call(&mut w, 2, 1, head);
    value(&w, 2, 1, &Err(Error::Forbidden));
    assert_eq!(w.requests().len(), MERGE_READY + 3);
    merge_call(&mut w, 3, 1, head);
    assert!(matches!(w.answered(3).expect("fresh merge").result, Ok(Value::Merged(_))));
    assert_eq!(w.requests().iter().filter(|r| r.starts_with(MINT.as_bytes())).count(), 4);
    assert!(w.script_done());
    assert!(w.close());
}

#[test]
fn github_compare_answers_two_commits_in_full() {
    compare_commits(2);
}

#[test]
fn github_compare_answers_several_commits_in_full() {
    compare_commits(8);
}

fn compare_commits(count: u64) {
    const COMPARE: &str = "GET /repos/acme/widgets/compare/0000000000000000000000000000000000000000...1111111111111111111111111111111111111111?per_page=8";
    const BEFORE: &str = "GET /repos/acme/widgets/contents/file?ref=0000000000000000000000000000000000000000";
    const AFTER: &str = "GET /repos/acme/widgets/contents/file?ref=1111111111111111111111111111111111111111";
    let mut after = [0; 32];
    after[..20].fill(0x11);
    let mut commits: Vec<_> = (1..count)
        .map(|id| skein_forge::github::id::commit(format!("{id:040x}").as_bytes()).expect("commit"))
        .collect();
    commits.push(after);
    let rows: Vec<_> = (1..count)
        .map(|id| format!(r#"{{"sha":"{id:040x}"}}"#))
        .chain(std::iter::once(format!(r#"{{"sha":"{}"}}"#, "1".repeat(40))))
        .collect();
    let body = format!(
        r#"{{"total_commits":{count},"behind_by":0,"commits":[{}],"files":[{{"filename":"file"}}]}}"#,
        rows.join(",")
    );
    let mut w = world(
        &[1],
        vec![
            Answer::served(LOCATE),
            bytes(COMPARE, body.as_bytes()),
            Answer {
                request: BEFORE,
                response: Response::Bytes(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nold".to_vec()),
            },
            Answer {
                request: AFTER,
                response: Response::Bytes(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nnew".to_vec()),
            },
        ],
    );
    located(&mut w);
    w.call(2, REPO, Op::Read(Read::Compare { before: [0; 32], after }));
    w.run_for(SECOND);
    value(
        &w,
        2,
        3,
        &Ok(Value::Compare {
            before: [0; 32],
            after,
            contains_before: true,
            commits: commits.into_boxed_slice(),
            files: Box::from([api::File {
                path: Box::from(b"file".as_slice()),
                before: Some(Box::from(b"old".as_slice())),
                after: Some(Box::from(b"new".as_slice())),
            }]),
        }),
    );
    assert!(w.script_done());
    assert_eq!(w.requests().len(), START + 4);
    assert!(w.close());
}
