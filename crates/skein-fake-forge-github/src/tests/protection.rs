use super::{fixtures as f, read_support as r};
use crate::service::{Config, Shown};
use skein_fake_forge_domain::api as a;
use skein_forge::{
    api,
    github::{self, request::Request},
};
#[expect(
    clippy::fn_params_excessive_bools,
    reason = "the test matrix varies independent protection and scenario facts"
)]
fn harness(shown: Shown, queue: bool, bypass: bool, pulls_only: bool, signatures: bool) -> f::Harness {
    let config = Config {
        shown,
        pull_requests_only: pulls_only,
        required_signatures: signatures,
        ruleset_id: 23,
        ..f::config()
    };
    let mut domain = r::domain_config();
    domain.limits.contexts = 2;
    f::Harness::with_protection(
        f::limits(),
        config,
        domain,
        Some(a::Protection {
            branch: f::boxed(b"trunk"),
            contexts: Box::from([f::boxed(b"build"), f::boxed(b"lint")]),
            approvals: 2,
            dismiss_stale: true,
            code_owners: true,
            linear: true,
            queue,
            bypassing_users: if bypass { Box::from([f::BOT]) } else { Box::from([]) },
        }),
    )
}
fn read(h: &mut f::Harness, branch: &[u8]) -> Option<api::Protection> {
    let token = r::token(h);
    let api::Answer::Protection(value) =
        r::read(h, &token, api::Read::Protection { branch: f::boxed(branch) }).unwrap()
    else {
        panic!("protection")
    };
    value
}
fn expected(queue: bool, bypass: api::Bypass, signatures: bool) -> api::Protection {
    api::Protection {
        branch: f::boxed(b"trunk"),
        contexts: Box::from([f::boxed(b"build"), f::boxed(b"lint")]),
        approvals: 2,
        dismiss_stale: true,
        code_owners: true,
        linear: true,
        queue,
        bypass,
        signatures,
    }
}
#[test]
fn ruleset_classic_and_both_decode_to_the_same_effective_protection() {
    for shown in [Shown::Ruleset, Shown::Classic, Shown::Both] {
        let mut h = harness(shown, false, false, false, true);
        assert_eq!(read(&mut h, b"trunk"), Some(expected(false, api::Bypass::Never, true)));
        assert_eq!(read(&mut h, b"release"), None);
    }
}
#[test]
fn ruleset_exposes_queue_and_each_app_bypass_and_both_tightens_bypass() {
    for (bypassing, pulls_only, bypass) in [
        (false, false, api::Bypass::Never),
        (false, true, api::Bypass::Never),
        (true, false, api::Bypass::Always),
        (true, true, api::Bypass::PullsOnly),
    ] {
        let mut h = harness(Shown::Ruleset, true, bypassing, pulls_only, true);
        assert_eq!(read(&mut h, b"trunk"), Some(expected(true, bypass, true)));
        let mut h = harness(Shown::Both, true, bypassing, pulls_only, true);
        assert_eq!(read(&mut h, b"trunk"), Some(expected(true, api::Bypass::Never, true)));
    }
}
#[test]
fn signatures_come_only_from_scenario_configuration() {
    for shown in [Shown::Ruleset, Shown::Classic, Shown::Both] {
        let mut h = harness(shown, false, false, false, false);
        assert_eq!(read(&mut h, b"trunk"), Some(expected(false, api::Bypass::Never, false)));
    }
}
#[test]
fn classic_hidden_and_unprotected_are_404_with_the_recorded_text() {
    for (shown, branch) in
        [(Shown::Ruleset, b"trunk".as_slice()), (Shown::Both, b"release"), (Shown::Classic, b"release")]
    {
        let mut h = harness(shown, false, false, false, false);
        let token = r::token(&mut h);
        let request = Request::ClassicProtection {
            owner: f::boxed(b"acme"),
            name: f::boxed(b"widgets"),
            branch: f::boxed(branch),
        };
        let encoded = github::request::encode(&request, &h.limits.github).unwrap();
        let response = h.ask(encoded.method, &encoded.target, Some(token.expose()), b"");
        f::refusal(&h, &response, 404, crate::texts::BRANCH_NOT_PROTECTED);
    }
    let mut h = r::harness();
    assert_eq!(read(&mut h, b"trunk"), None);
}
#[test]
fn rules_page_is_capped_and_names_the_configured_ruleset_id() {
    let mut h = harness(Shown::Ruleset, true, true, false, true);
    let token = r::token(&mut h);
    let request = Request::RulesForBranch {
        owner: f::boxed(b"acme"),
        name: f::boxed(b"widgets"),
        branch: f::boxed(b"trunk"),
        page: 1,
    };
    let encoded = github::request::encode(&request, &h.limits.github).unwrap();
    let response = h.ask(encoded.method, &encoded.target, Some(token.expose()), b"");
    assert_eq!(response.status, 200);
    assert!(response.link.is_some());
    let (rows, document) = h.decode(github::document::Kind::RuleRows, &response);
    assert_eq!(document, github::document::Document::Rows);
    assert_eq!(rows.len(), h.limits.page);
    for row in rows.into_boxed() {
        let github::document::Row::Rule(rule) = row else { panic!("rule") };
        assert_eq!(rule.ruleset, 23);
    }
}
#[test]
fn missing_ruleset_and_classic_only_do_not_invent_a_ruleset() {
    for shown in [Shown::Ruleset, Shown::Classic] {
        let mut h = harness(shown, false, false, false, true);
        let token = r::token(&mut h);
        let request = Request::Ruleset {
            owner: f::boxed(b"acme"),
            name: f::boxed(b"widgets"),
            id: if shown == Shown::Classic { 23 } else { 24 },
        };
        let encoded = github::request::encode(&request, &h.limits.github).unwrap();
        let response = h.ask(encoded.method, &encoded.target, Some(token.expose()), b"");
        f::refusal(&h, &response, 404, b"Not Found");
    }
}
