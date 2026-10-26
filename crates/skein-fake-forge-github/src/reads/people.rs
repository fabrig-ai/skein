//! User identity belongs to the GitHub tables; permissions belong to the domain.
use crate::{
    answers::{self, Body, Page},
    service::{Limits, Response, Service},
};
use alloc::boxed::Box;
use skein_fake_forge_domain::api as a;
use skein_forge::{api, github::document as d};
use skein_lib::List;

pub(crate) fn id(service: &Service, login: &[u8]) -> Option<u64> {
    if login == crate::service::joined(&service.app.slug, b"[bot]")?.as_ref() {
        return Some(service.bot);
    }
    for account in &service.accounts {
        if account.login.as_ref() == login {
            return Some(account.id);
        }
    }
    None
}
pub(super) fn user(service: &Service, id: u64, body_fits: bool) -> Response {
    let login =
        if id == service.bot { crate::service::joined(&service.app.slug, b"[bot]") } else { login(service, id) };
    let Some(login) = login else {
        return answers::missing(&service.limits);
    };
    if !body_fits {
        return answers::refusal(413, b"Payload Too Large", &service.limits);
    }
    answers::response(200, &Body::User(&d::User { id, login }), &service.limits)
}
pub(crate) fn login(service: &Service, id: u64) -> Option<Box<[u8]>> {
    for account in &service.accounts {
        if account.id == id {
            return Some(account.login.clone());
        }
    }
    None
}
pub(super) fn permission(value: a::Permission) -> api::Permission {
    match value {
        a::Permission::None => api::Permission::None,
        a::Permission::Read => api::Permission::Read,
        a::Permission::Write => api::Permission::Write,
        a::Permission::Admin => api::Permission::Admin,
    }
}
pub(super) fn collaborators(answer: a::Answer, page: u32, size: u32, target: &[u8], limits: &Limits) -> Response {
    let a::Answer::Collaborators(values) = answer else {
        return answers::failed();
    };
    let Some(page) = Page::new(page, size, u64::try_from(values.len()).expect("domain collaborators")) else {
        return answers::failed();
    };
    let mut rows = List::with_capacity(size);
    for (index, row) in values.into_iter().enumerate() {
        if page.contains(u64::try_from(index).expect("domain index")) {
            // Login is optional in the dialect's collaborator document; a
            // permission lookup resolves it separately through /user/<id>.
            if rows.push(d::Collaborator { id: row.user, permission: permission(row.permission), login: None }).is_err()
            {
                return answers::failed();
            }
        }
    }
    page.link(answers::response(200, &Body::Collaborators(rows.as_slice()), limits), target, limits)
}
