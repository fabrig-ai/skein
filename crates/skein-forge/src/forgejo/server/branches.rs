use super::{Answer, Route, Unrouted, items::page};
use crate::forgejo::{
    document::{Branch, BranchProtection, Permission},
    id,
    limits::Limits,
    path,
    request::{Method, Repo, Request},
};
use skein_json::writer::Encoder;
pub(super) fn route(method: Method, bytes: &[u8], limits: &Limits) -> Result<Route, Unrouted> {
    let Ok(target) = path::read(bytes, limits) else {
        return Err(Unrouted::Malformed);
    };
    let [api, v1, repos, owner, name, tail @ ..] = target.segments.as_ref() else {
        return Err(Unrouted::NotFound);
    };
    if &**api != b"api" || &**v1 != b"v1" || &**repos != b"repos" {
        return Err(Unrouted::NotFound);
    }
    let at = Repo { owner: owner.clone(), name: name.clone() };
    let request = match tail {
        [git, refs, heads, prefix] if &**git == b"git" && &**refs == b"refs" && &**heads == b"heads" => {
            Request::Branches {
                at,
                prefix: Some(prefix.clone()),
                page: crate::forgejo::request::Page { page: None, limit: None },
            }
        }
        [branches] if &**branches == b"branches" => Request::Branches { at, prefix: None, page: page(&target.query)? },
        [branches, name] if &**branches == b"branches" => Request::Branch { at, name: name.clone() },
        [rules] if &**rules == b"branch_protections" => Request::Protections { at },
        [rules, name] if &**rules == b"branch_protections" => Request::Protection { at, name: name.clone() },
        [collaborators] if &**collaborators == b"collaborators" => {
            Request::Collaborators { at, page: page(&target.query)? }
        }
        [collaborators, login, permission]
            if &**collaborators == b"collaborators" && &**permission == b"permission" =>
        {
            Request::Permission { at, login: login.clone() }
        }
        _ => return Err(Unrouted::NotFound),
    };
    match method {
        Method::Get => Ok(Route::Ready(request)),
        Method::Post | Method::Put | Method::Patch | Method::Delete => Err(Unrouted::Method),
    }
}
fn branch(json: &mut Encoder, value: &Branch) {
    json.object_start();
    json.key(b"name");
    json.string(&value.name);
    json.key(b"commit");
    json.object_start();
    json.key(b"id");
    json.string(&id::write(&value.commit));
    json.object_end();
    json.key(b"effective_branch_protection_name");
    match &value.effective_branch_protection_name {
        Some(name) => json.string(name),
        None => json.string(b""),
    }
    json.object_end();
}
fn protection(json: &mut Encoder, value: &BranchProtection) {
    json.object_start();
    json.key(b"rule_name");
    json.string(&value.rule_name);
    json.key(b"enable_status_check");
    json.boolean(value.enable_status_check);
    json.key(b"status_check_contexts");
    json.array_start();
    for context in &value.status_check_contexts {
        json.string(context);
    }
    json.array_end();
    json.key(b"required_approvals");
    json.unsigned(u64::from(value.required_approvals));
    json.key(b"dismiss_stale_approvals");
    json.boolean(value.dismiss_stale_approvals);
    json.key(b"require_signed_commits");
    json.boolean(value.require_signed_commits);
    json.object_end();
}
pub(super) fn permission(value: Permission) -> &'static [u8] {
    match value {
        Permission::None => b"none",
        Permission::Read => b"read",
        Permission::Write => b"write",
        Permission::Admin => b"admin",
        Permission::Owner => b"owner",
    }
}
pub(super) fn write(json: &mut Encoder, answer: &Answer) {
    match answer {
        Answer::Branch(value) => branch(json, value),
        Answer::Protection(value) => protection(json, value),
        Answer::BranchRefs(values) => {
            json.array_start();
            for value in values {
                json.object_start();
                json.key(b"ref");
                json.string_parts(&[b"refs/heads/", &value.name]);
                json.key(b"object");
                json.object_start();
                json.key(b"type");
                json.string(b"commit");
                json.key(b"sha");
                json.string(&id::write(&value.commit));
                json.object_end();
                json.object_end();
            }
            json.array_end();
        }
        Answer::Branches(values) => {
            json.array_start();
            for value in values {
                branch(json, value);
            }
            json.array_end();
        }
        Answer::Protections(values) => {
            json.array_start();
            for value in values {
                protection(json, value);
            }
            json.array_end();
        }
        Answer::Collaborators(values) => {
            json.array_start();
            for value in values {
                super::start::user(json, value);
            }
            json.array_end();
        }
        Answer::Permission(value) => {
            json.object_start();
            json.key(b"permission");
            json.string(permission(value.permission));
            json.key(b"user");
            super::start::user(json, &value.user);
            json.object_end();
        }
        Answer::Error(_)
        | Answer::Comment(_)
        | Answer::Status(_)
        | Answer::Review(_)
        | Answer::Files(_)
        | Answer::Compare { .. }
        | Answer::Tree { .. }
        | Answer::Runs { .. }
        | Answer::Run(_)
        | Answer::Job { .. }
        | Answer::Jobs(_)
        | Answer::Token(_)
        | Answer::Tokens(_)
        | Answer::User(_)
        | Answer::ApiSettings(_)
        | Answer::Repository(_)
        | Answer::Reachable(_)
        | Answer::RepoSearch { .. }
        | Answer::Search { .. }
        | Answer::Issues(_)
        | Answer::Issue(_)
        | Answer::Comments(_)
        | Answer::Pull(_)
        | Answer::Org(_)
        | Answer::CreatedFile { .. }
        | Answer::CreatedRepository(_)
        | Answer::Team(_)
        | Answer::Teams(_)
        | Answer::Hook(_)
        | Answer::Hooks(_)
        | Answer::Members(_)
        | Answer::CombinedStatus { .. }
        | Answer::Reviews(_)
        | Answer::ReviewComments(_)
        | Answer::Pulls(_) => unreachable!("branch answer selection"),
    }
}
