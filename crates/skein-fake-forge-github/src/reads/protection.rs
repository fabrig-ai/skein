//! GitHub's representation is scenario data; the rule facts come from the domain.
use crate::{
    answers::{self, Body, Page},
    service::{Response, Service, Shown},
    texts,
};
use alloc::boxed::Box;
use skein_fake_forge_domain::{self as fake, api as a};
use skein_forge::{
    api::Bypass,
    github::document::{self as d, RuleKind},
};
use skein_lib::List;
#[derive(Debug)]
pub(crate) enum Read {
    Rules { branch: Box<[u8]>, page: u32, target: Box<[u8]> },
    Ruleset(u64),
    Classic(Box<[u8]>),
}
pub(crate) fn finish(service: &Service, domain: &fake::Domain, repository: &[u8], query: Read) -> Response {
    let limits = &service.limits;
    match query {
        Read::Rules { branch, page, target } => {
            let protection = domain.inspect_protection(repository, &branch);
            let mut rows = List::with_capacity(5);
            if let Some(value) = protection
                && service.shown != Shown::Classic
            {
                if !fits(value, service) {
                    return answers::failed();
                }
                let id = service.ruleset_id;
                rows.push(d::Rule {
                    ruleset: id,
                    kind: RuleKind::PullRequest {
                        approvals: value.approvals,
                        dismiss_stale: value.dismiss_stale,
                        code_owners: value.code_owners,
                    },
                })
                .expect("five rules");
                rows.push(d::Rule { ruleset: id, kind: RuleKind::StatusChecks(value.contexts.clone()) })
                    .expect("five rules");
                for (enabled, kind) in [
                    (value.queue, RuleKind::Queue),
                    (service.required_signatures, RuleKind::Signatures),
                    (value.linear, RuleKind::Linear),
                ] {
                    if enabled {
                        rows.push(d::Rule { ruleset: id, kind }).expect("five rules");
                    }
                }
            }
            let Some(page) = Page::new(page, limits.page, u64::from(rows.len())) else {
                return answers::failed();
            };
            let mut listed = List::with_capacity(5);
            for (index, row) in rows.into_boxed().into_iter().enumerate() {
                let Ok(index) = u64::try_from(index) else {
                    return answers::failed();
                };
                if page.contains(index) {
                    listed.push(row).expect("at most five rules");
                }
            }
            page.link(answers::response(200, &Body::ProtectionRules(listed.as_slice()), limits), &target, limits)
        }
        Read::Ruleset(id) => {
            let Some(value) = domain.inspect_repository_protection(repository) else {
                return answers::missing(limits);
            };
            if id != service.ruleset_id || service.shown == Shown::Classic {
                return answers::missing(limits);
            }
            let bypass = if !value.bypassing_users.contains(&service.bot) {
                Bypass::Never
            } else if service.pull_requests_only {
                Bypass::PullsOnly
            } else {
                Bypass::Always
            };
            answers::response(200, &Body::Ruleset(bypass), limits)
        }
        Read::Classic(branch) => {
            let Some(value) = domain.inspect_protection(repository, &branch) else {
                return hidden(service);
            };
            if service.shown == Shown::Ruleset {
                return hidden(service);
            }
            if !fits(value, service) {
                return answers::failed();
            }
            let value = d::Protection {
                contexts: value.contexts.clone(),
                approvals: value.approvals,
                dismiss_stale: value.dismiss_stale,
                code_owners: value.code_owners,
                linear: value.linear,
                signatures: service.required_signatures,
                queue: false,
            };
            answers::response(200, &Body::Classic(&value), limits)
        }
    }
}
fn hidden(service: &Service) -> Response {
    answers::refusal(404, texts::BRANCH_NOT_PROTECTED, &service.limits)
}
fn fits(value: &a::Protection, service: &Service) -> bool {
    let Ok(count) = u32::try_from(value.contexts.len()) else {
        return false;
    };
    if count > service.limits.github.inner {
        return false;
    }
    for context in &value.contexts {
        let Ok(length) = u32::try_from(context.len()) else {
            return false;
        };
        if length > service.limits.github.string_bytes {
            return false;
        }
    }
    true
}
