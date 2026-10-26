use super::{fixtures as f, read_support as r};
use skein_fake_forge_domain::api as a;
use skein_forge::{
    api,
    github::{
        self,
        document::{self as d, Document, Kind},
        header::Head,
        plans::{Body, Heard, plan::Step, reads::Plan, tables::Tables},
        request::{Method, Request},
        server,
    },
};
use skein_lib::{List, Secret};

#[test]
fn github_fake_graphql_pull_drives_the_plan_with_empty_ci_fixture() {
    let mut h = r::harness();
    let (number, commit) = r::pull(&mut h);
    let token = r::token(&mut h);
    let mut plan = Plan::new(api::Repository { forge: 1, repository: 2 }, api::Read::Pull { number });
    let mut tables = Tables::new(4, 8);
    let mut step = plan.begin(r::bounds(), b"example-app", &mut tables).unwrap();
    let mut queried = false;
    for _exchange in 0..8_u32 {
        let exchange = match step {
            Step::Done(answer) => {
                let api::Answer::Pull(pull) = answer.unwrap() else { panic!("pull answer") };
                assert!(queried);
                assert_eq!(pull.number, number);
                assert_eq!(pull.commit, r::object(commit));
                assert_eq!(pull.merged, None);
                assert_eq!(pull.queued, None);
                assert_eq!(pull.dequeued, None);
                assert_eq!(h.hooks, 0);
                return;
            }
            Step::Redirect { .. } => panic!("this plan uses only API exchanges"),
            Step::Send(exchange) => exchange,
        };
        // CI routes belong to another task. Supply only their empty plan
        // prerequisites here; every repository/pull/GraphQL exchange uses the fake.
        let events = if github::decode::kind(&exchange.request) == Some(Kind::CombinedStatus) {
            fixture(Document::Combined(d::Combined { check: api::Check::Pending, total: 0 }))
        } else if github::decode::kind(&exchange.request) == Some(Kind::CheckRuns) {
            fixture(Document::Rows)
        } else {
            if let Request::PullGraphql { .. } = &exchange.request {
                queried = true;
            }
            r::exchange_events(&mut h, &exchange, &token)
        };
        let mut next = None;
        for event in events.into_boxed() {
            next = plan.heard(event, r::bounds(), b"example-app", &mut tables).unwrap();
            if next.is_some() {
                break;
            }
        }
        step = next.expect("plan advances");
    }
    panic!("pull plan settles")
}
fn fixture(document: Document) -> List<Heard> {
    let mut events = List::with_capacity(2);
    events.push(Heard::Head { status: 200, head: Head::new() }).unwrap();
    events.push(Heard::Done(Body::Document(document))).unwrap();
    events
}
fn query(h: &mut f::Harness, token: &Secret, name: &[u8], number: u64) -> crate::service::Response {
    let request = Request::PullGraphql { owner: f::boxed(b"acme"), name: f::boxed(name), number };
    let encoded = github::request::encode(&request, &h.limits.github).unwrap();
    h.ask(encoded.method, &encoded.target, Some(token.expose()), encoded.body.as_deref().unwrap())
}
fn pull(h: &f::Harness, response: &crate::service::Response) -> d::PullGraphql {
    assert_eq!(response.status, 200);
    let (_, Document::PullGraphql(pull)) = h.decode(Kind::PullGraphql, response) else { panic!("GraphQL pull") };
    pull
}
#[test]
fn github_fake_graphql_pull_has_stable_identity_and_domain_merge_commit() {
    let mut h = r::harness();
    let (number, head) = r::pull(&mut h);
    let token = r::token(&mut h);
    let response = query(&mut h, &token, b"widgets", number);
    let initial = pull(&h, &response);
    assert!(!initial.id.is_empty());
    assert_eq!(initial.merge_commit, None);
    assert_eq!(initial.queued, None);
    assert_eq!(initial.dequeued, None);
    let a::Answer::Merged(merged) = r::call(&mut h, a::Op::Write(a::Write::Merge { number, head })) else {
        panic!("merged")
    };
    let response = query(&mut h, &token, b"widgets", number);
    let landed = pull(&h, &response);
    assert_eq!(landed.id, initial.id);
    assert_eq!(landed.merge_commit, Some(r::object(merged)));
    assert_eq!(landed.queued, None);
    assert_eq!(landed.dequeued, None);
    let digits = skein_lib::Decimal::of(number);
    let target = f::parts(&[b"/repos/acme/widgets/pulls/", digits.as_bytes()]);
    let response = h.ask(Method::Get, &target, Some(token.expose()), b"");
    assert!(skein_lib::bytes::find(&response.body, b"merge_commit_sha").is_none());
    let api::Answer::Pull(readback) = r::read(&mut h, &token, api::Read::Pull { number }).unwrap() else {
        panic!("merged pull plan")
    };
    assert_eq!(readback.merged, Some(r::object(merged)));
    let target = f::parts(&[&target, b"/merge"]);
    let response = h.ask(
        Method::Put,
        &target,
        Some(token.expose()),
        br#"{"sha":"0000000000000000000000000000000000000000","merge_method":"rebase"}"#,
    );
    assert_eq!(response.status, 200);
    let (_, Document::Merge(repeated)) = h.decode(Kind::Merge, &response) else { panic!("repeat merge") };
    assert_eq!(repeated, r::object(merged));
    let response = query(&mut h, &token, b"widgets", number);
    assert_eq!(pull(&h, &response).merge_commit, Some(repeated));
    let api::Answer::Pull(readback) = r::read(&mut h, &token, api::Read::Pull { number }).unwrap() else {
        panic!("repeat merged pull plan")
    };
    assert_eq!(readback.merged, Some(repeated));
    let response = query(&mut h, &token, b"widgets", 999);
    f::refusal(&h, &response, 404, b"Not Found");
    let calls = h.calls;
    let response = query(&mut h, &token, b"absent", number);
    f::refusal(&h, &response, 404, b"Not Found");
    assert_eq!(h.calls, calls, "scope refusal precedes domain access");
}
#[test]
fn github_fake_graphql_matches_documents_and_bounds_variables() {
    let mut h = r::harness();
    let (number, _) = r::pull(&mut h);
    let token = r::token(&mut h);
    let request = Request::PullGraphql { owner: f::boxed(b"acme"), name: f::boxed(b"widgets"), number };
    let encoded = github::request::encode(&request, &h.limits.github).unwrap();
    let body = encoded.body.unwrap();
    let Some(server::Graphql::Pull { owner, name, number: decoded }) = server::graphql_body(&body, &h.limits.github)
    else {
        panic!("pull query envelope")
    };
    assert_eq!((owner.as_ref(), name.as_ref(), decoded), (b"acme".as_slice(), b"widgets".as_slice(), number));
    let mut limits = h.limits.github;
    limits.string_bytes = 3;
    assert!(server::graphql_body(&body, &limits).is_none());
    limits = h.limits.github;
    limits.body_bytes = u32::try_from(body.len()).unwrap().checked_sub(1).unwrap();
    assert!(server::graphql_body(&body, &limits).is_none());
    let response = h.ask(
        Method::Post,
        b"/graphql",
        Some(token.expose()),
        br#" { "query" : "query { viewer { login databaseId } }" } "#,
    );
    let (_, Document::Viewer(viewer)) = h.decode(Kind::Viewer, &response) else { panic!("viewer byte match") };
    assert_eq!(viewer.id, f::BOT);
    for body in [
        br#"{"query":"query { viewer { databaseId login } }"}"#.as_slice(),
        br#"{"query":"query { viewer { login databaseId } }","query":"other"}"#,
        br#"{"query":"query { viewer { login databaseId } }","variables":null}"#,
        br#"{"query":"query { viewer { login databaseId } }"} []"#,
    ] {
        let response = h.ask(Method::Post, b"/graphql", Some(token.expose()), body);
        assert_eq!(response.status, 200);
        let (_, Document::Graphql(error)) = h.decode(Kind::PullGraphql, &response) else {
            panic!("unsupported document errors")
        };
        assert_eq!(error.kind.as_ref(), b"UNPROCESSABLE");
    }
    // Matching query text still needs all three typed variables.
    for variables in [
        b"{}".as_slice(),
        br#"{"owner":"acme","name":"widgets","number":"1"}"#,
        br#"{"owner":"acme","name":"widgets","number":-1}"#,
    ] {
        let mut measure = skein_json::writer::Encoder::measure(&h.limits.github.writer);
        envelope(&mut measure, variables, &h.limits.github.json);
        let mut json = skein_json::writer::Encoder::write(measure.measured().unwrap(), &h.limits.github.writer);
        envelope(&mut json, variables, &h.limits.github.json);
        let response = h.ask(Method::Post, b"/graphql", Some(token.expose()), &json.finish());
        assert_eq!(response.status, 200);
        let (_, Document::Graphql(_)) = h.decode(Kind::PullGraphql, &response) else {
            panic!("invalid variables errors")
        };
    }
}

fn envelope(json: &mut skein_json::writer::Encoder, variables: &[u8], limits: &skein_json::tokenizer::Limits) {
    json.object_start();
    json.key(b"variables");
    json.json_text(variables, limits).unwrap();
    json.key(b"query");
    json.string(d::PULL);
    json.object_end();
}

#[test]
fn github_fake_graphql_pull_reads_the_domains_retained_admission() {
    let mut h = super::queue::harness(true);
    let (number, head) = r::pull(&mut h);
    let token = r::token(&mut h);
    let response = query(&mut h, &token, b"widgets", number);
    assert_eq!(pull(&h, &response).queued, None);
    r::call(&mut h, a::Op::Write(a::Write::Enqueue { number, head }));
    let response = query(&mut h, &token, b"widgets", number);
    let entry = h.domain.queue_entries(b"acme/widgets").unwrap().first().unwrap();
    assert_eq!(pull(&h, &response).queued, Some(entry.position));
}

fn enqueue(h: &mut f::Harness, token: &Secret, id: &[u8], head: u64) -> crate::service::Response {
    let encoded =
        github::request::encode(&Request::Enqueue { id: f::boxed(id), head: r::object(head) }, &h.limits.github)
            .unwrap();
    h.ask(encoded.method, &encoded.target, Some(token.expose()), encoded.body.as_deref().unwrap())
}
fn enqueued(h: &f::Harness, response: &crate::service::Response) -> u32 {
    assert_eq!(response.status, 200);
    let (_, Document::Enqueued(position)) = h.decode(Kind::Enqueue, response) else { panic!("admission entry") };
    position
}
#[test]
fn github_fake_enqueue_is_idempotent_and_pull_reads_its_position() {
    let mut h = super::queue::harness(true);
    skein_fake_forge_domain::grant(&mut h.domain, b"acme/widgets", f::BOT, a::Permission::Write);
    let (number, head) = r::pull(&mut h);
    let token = r::token(&mut h);
    let response = query(&mut h, &token, b"widgets", number);
    let id = pull(&h, &response).id;
    r::call_as(
        &mut h,
        f::BOT,
        a::Op::Write(a::Write::Status { commit: head, context: f::boxed(b"required"), state: a::Check::Passed }),
    );
    for _delivery in 0..2_u32 {
        let response = enqueue(&mut h, &token, &id, head);
        assert_eq!(enqueued(&h, &response), 1);
    }
    let entries = h.domain.queue_entries(b"acme/widgets").unwrap();
    assert_eq!(entries, [a::QueueEntry { number, head, position: 1 }]);
    let response = query(&mut h, &token, b"widgets", number);
    assert_eq!(pull(&h, &response).queued, Some(1));
}
#[test]
fn github_fake_enqueue_distinguishes_stale_heads_and_unknown_nodes() {
    let mut h = r::harness();
    skein_fake_forge_domain::grant(&mut h.domain, b"acme/widgets", f::BOT, a::Permission::Write);
    let (number, head) = r::pull(&mut h);
    let token = r::token(&mut h);
    let response = query(&mut h, &token, b"widgets", number);
    let id = pull(&h, &response).id;
    let stale = h.first;
    let response = enqueue(&mut h, &token, &id, stale);
    assert_eq!(response.status, 200);
    let (_, Document::Graphql(error)) = h.decode(Kind::Enqueue, &response) else { panic!("stale error") };
    assert_eq!(error.kind.as_ref(), b"UNPROCESSABLE");
    assert_eq!(error.message.as_ref(), crate::texts::ENQUEUE_STALE);
    for id in [b"unknown".as_slice(), b"PR_2_999", b"PR_999_1", b"PR_02_1", b"PR_6_1"] {
        let response = enqueue(&mut h, &token, id, head);
        assert_eq!(response.status, 200);
        let (_, Document::Graphql(error)) = h.decode(Kind::Enqueue, &response) else { panic!("missing node") };
        assert_eq!(error.kind.as_ref(), b"NOT_FOUND");
    }
    assert!(h.domain.queue_entries(b"acme/widgets").unwrap().is_empty());
}
