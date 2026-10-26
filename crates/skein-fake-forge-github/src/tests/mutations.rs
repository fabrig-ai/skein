use super::{fixtures as f, read_support as r};
use crate::service::Response;
use skein_fake_forge_domain::{self as fake, api as a};
use skein_forge::github::{
    document::{Document, Kind},
    request::Method,
};
use skein_lib::{List, Secret};

const CREATE: &[u8] = b"/repos/acme/widgets/rulesets";
const DELETE: &[u8] = b"/repos/acme/widgets/rulesets/12345678";
const RULE: &str = r#"{"name":"probe","target":"branch","enforcement":"active","conditions":{"ref_name":{"include":["refs/heads/trunk"],"exclude":[]}},"bypass_actors":[{"actor_id":7,"actor_type":"Integration","bypass_mode":"always"}],"rules":[{"parameters":{"required_approving_review_count":2,"dismiss_stale_reviews_on_push":false,"require_code_owner_review":false,"require_last_push_approval":false,"required_review_thread_resolution":false},"type":"pull_request"},{"type":"required_status_checks","parameters":{"required_status_checks":[{"context":"ci"}],"strict_required_status_checks_policy":false}},{"type":"merge_queue","parameters":{"check_response_timeout_minutes":60,"grouping_strategy":"ALLGREEN","max_entries_to_build":5,"max_entries_to_merge":5,"merge_method":"MERGE","min_entries_to_merge":1,"min_entries_to_merge_wait_minutes":0}}]}"#;

fn harness() -> f::Harness {
    let mut config = f::config();
    config.ruleset_id = 12_345_678;
    config.tokens = List::with_capacity(1);
    config.tokens.push((Secret::new(f::boxed(b"invented-person")), 31, None)).unwrap();
    let mut h = f::Harness::with_domain(f::limits(), config, r::domain_config());
    fake::grant(&mut h.domain, b"acme/widgets", 31, a::Permission::Admin);
    h
}
fn ask(h: &mut f::Harness, method: Method, target: &[u8], body: &[u8]) -> Response {
    h.ask(method, target, Some(f::bearer(b"invented-person").expose()), body)
}
#[test]
fn github_fake_person_rulesets_write_protection_and_read_the_server_shape() {
    let mut h = harness();
    let response = ask(&mut h, Method::Post, CREATE, RULE.as_bytes());
    assert_eq!(response.status, 201);
    assert!(response.body.starts_with(br#"{"id":12345678,"#));
    let (_, Document::Ruleset(rule)) = h.decode(Kind::Ruleset, &response) else { panic!("ruleset") };
    assert_eq!(rule, skein_forge::api::Bypass::Always);
    let rule = h.domain.inspect_repository_protection(b"acme/widgets").unwrap();
    assert_eq!(rule.branch.as_ref(), b"trunk");
    assert_eq!(rule.approvals, 2);
    assert_eq!(rule.contexts.as_ref(), &[f::boxed(b"ci")]);
    assert_eq!(rule.bypassing_users.as_ref(), &[f::BOT]);
    assert!(rule.queue);
    assert_eq!(ask(&mut h, Method::Get, DELETE, b"").body.as_ref(), br#"{"current_user_can_bypass":"always"}"#);
    let response = ask(&mut h, Method::Delete, DELETE, b"");
    assert_eq!(response.status, 204);
    assert!(response.body.is_empty());
    assert!(h.domain.inspect_repository_protection(b"acme/widgets").is_none());
    assert_eq!(ask(&mut h, Method::Get, DELETE, b"").status, 404);
}
#[test]
fn github_fake_rulesets_refuse_unknown_duplicate_and_out_of_subset_fields_without_writing() {
    let mut h = harness();
    for (from, to) in [
        ("\"name\":\"probe\"", "\"extra\":true,\"name\":\"probe\""),
        ("\"name\":\"probe\"", "\"name\":\"probe\",\"name\":\"duplicate\""),
        ("\"active\"", "\"disabled\""),
        ("\"branch\"", "\"tag\""),
        ("\"required_approving_review_count\":2", "\"required_approving_review_count\":2,\"unknown\":true"),
        ("\"context\":\"ci\"", "\"context\":\"ci\",\"unknown\":true"),
        ("\"merge_queue\"", "\"unknown\""),
        ("\"bypass_mode\":\"always\"", "\"bypass_mode\":\"pull_request\""),
        ("\"include\":[\"refs/heads/trunk\"]", "\"include\":[\"refs/heads/trunk\"],\"exclude\":[]"),
        ("\"required_approving_review_count\":2", "\"required_approving_review_count\":-1"),
    ] {
        let body = RULE.replace(from, to);
        let response = ask(&mut h, Method::Post, CREATE, body.as_bytes());
        f::refusal(&h, &response, 422, b"Validation Failed");
        assert!(h.domain.inspect_repository_protection(b"acme/widgets").is_none());
    }
    assert_eq!(ask(&mut h, Method::Delete, DELETE, b"{}").status, 422);
}
#[test]
fn github_fake_ruleset_mutations_require_a_person_with_domain_admin_permission() {
    let mut h = harness();
    let installed = f::bearer(h.mint(1, false).token.expose());
    for (method, target, body) in [(Method::Post, CREATE, RULE.as_bytes()), (Method::Delete, DELETE, b"".as_slice())] {
        let calls = h.calls;
        let response = h.ask(method, target, Some(installed.expose()), body);
        f::refusal(&h, &response, 403, b"Resource not accessible by integration");
        assert_eq!(h.calls, calls);
        assert_eq!(h.ask(method, target, None, body).status, 401);
        fake::grant(&mut h.domain, b"acme/widgets", 31, a::Permission::Write);
        assert_eq!(ask(&mut h, method, target, body).status, 404);
        assert!(h.domain.inspect_repository_protection(b"acme/widgets").is_none());
    }
}

#[test]
fn github_fake_pull_request_rules_require_each_of_the_five_parameters() {
    for field in [
        "required_approving_review_count",
        "dismiss_stale_reviews_on_push",
        "require_code_owner_review",
        "require_last_push_approval",
        "required_review_thread_resolution",
    ] {
        let mut h = harness();
        let value = if field == "required_approving_review_count" { "2" } else { "false" };
        let member = alloc::fmt::format(format_args!("\"{field}\":{value}"));
        let first = alloc::fmt::format(format_args!("{member},"));
        let last = alloc::fmt::format(format_args!(",{member}"));
        let body = RULE.replace(&first, "").replace(&last, "");
        assert_ne!(body, RULE);
        let response = ask(&mut h, Method::Post, CREATE, body.as_bytes());
        f::refusal(
            &h,
            &response,
            422,
            b"Invalid request.\n\nInvalid property /rules/0: data matches no possible input. See `documentation_url`.",
        );
        assert!(h.domain.inspect_repository_protection(b"acme/widgets").is_none());
    }
}

#[test]
fn github_fake_status_and_queue_rules_require_each_parameter_at_its_rule_index() {
    for (index, member) in [
        (1_u32, "\"strict_required_status_checks_policy\":false"),
        (2, "\"check_response_timeout_minutes\":60"),
        (2, "\"grouping_strategy\":\"ALLGREEN\""),
        (2, "\"max_entries_to_build\":5"),
        (2, "\"max_entries_to_merge\":5"),
        (2, "\"merge_method\":\"MERGE\""),
        (2, "\"min_entries_to_merge\":1"),
        (2, "\"min_entries_to_merge_wait_minutes\":0"),
    ] {
        let mut h = harness();
        let first = alloc::fmt::format(format_args!("{member},"));
        let last = alloc::fmt::format(format_args!(",{member}"));
        let body = RULE.replace(&first, "").replace(&last, "");
        assert_ne!(body, RULE);
        let response = ask(&mut h, Method::Post, CREATE, body.as_bytes());
        let message = alloc::fmt::format(format_args!(
            "Invalid request.\n\nInvalid property /rules/{index}: data matches no possible input. See `documentation_url`."
        ));
        f::refusal(&h, &response, 422, message.as_bytes());
        assert!(h.domain.inspect_repository_protection(b"acme/widgets").is_none());
    }
}
#[test]
fn github_fake_merge_queue_rules_require_an_organization() {
    let mut config = f::config();
    for index in 0..config.repositories.len() {
        let row = config.repositories.get_mut(index).unwrap();
        if row.id == 2 {
            row.organization = false;
        }
    }
    config.tokens = List::with_capacity(1);
    config.tokens.push((Secret::new(f::boxed(b"invented-person")), 31, None)).unwrap();
    let mut h = f::Harness::with_domain(f::limits(), config, r::domain_config());
    fake::grant(&mut h.domain, b"acme/widgets", 31, a::Permission::Admin);
    let response = ask(&mut h, Method::Post, CREATE, RULE.as_bytes());
    f::refusal(&h, &response, 422, b"Validation Failed");
    assert!(h.domain.inspect_repository_protection(b"acme/widgets").is_none());
}
