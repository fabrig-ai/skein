use super::{Answer, Route, Unrouted};
use crate::forgejo::{
    document::{self, User},
    id::{self, ObjectFormat},
    limits::Limits,
    path::{self, Pair},
    request::{Method, Repo, Request},
};
use alloc::boxed::Box;
use skein_json::writer::Encoder;
pub(super) fn route(method: Method, target: &[u8], limits: &Limits) -> Result<Route, Unrouted> {
    let Ok(target) = path::read(target, limits) else {
        return Err(Unrouted::Malformed);
    };
    let request = match target.segments.as_ref() {
        [api, v1, repos, search]
            if &**api == b"api" && &**v1 == b"v1" && &**repos == b"repos" && &**search == b"search" =>
        {
            for pair in &target.query {
                if (pair.name.as_ref() == b"sort" && pair.value.as_ref() != b"id")
                    || (pair.name.as_ref() == b"order" && pair.value.as_ref() != b"asc")
                {
                    return Err(Unrouted::Malformed);
                }
            }
            Request::RepoSearch { page: super::items::page(&target.query)? }
        }
        [api, v1, user, repos] if &**api == b"api" && &**v1 == b"v1" && &**user == b"user" && &**repos == b"repos" => {
            Request::UserRepos { page: super::items::page(&target.query)? }
        }
        [api, v1, user] if &**api == b"api" && &**v1 == b"v1" && &**user == b"user" => Request::User,
        [api, v1, settings, name]
            if &**api == b"api" && &**v1 == b"v1" && &**settings == b"settings" && &**name == b"api" =>
        {
            Request::ApiSettings
        }
        [api, v1, repositories, number] if &**api == b"api" && &**v1 == b"v1" && &**repositories == b"repositories" => {
            Request::RepositoryById { id: id::unsigned(number).ok_or(Unrouted::Malformed)? }
        }
        [api, v1, repos, owner, name] if &**api == b"api" && &**v1 == b"v1" && &**repos == b"repos" => {
            Request::Repository { at: Repo { owner: owner.clone(), name: name.clone() } }
        }
        [api, v1, users, search]
            if &**api == b"api" && &**v1 == b"v1" && &**users == b"users" && &**search == b"search" =>
        {
            let uid = match query(&target.query, b"uid") {
                Some(text) => id::unsigned(&text).ok_or(Unrouted::Malformed)?,
                None => 0,
            };
            let limit = match query(&target.query, b"limit") {
                Some(text) => {
                    let n = id::unsigned(&text).ok_or(Unrouted::Malformed)?;
                    Some(match u32::try_from(n) {
                        Ok(n) => n,
                        Err(_) => return Err(Unrouted::Malformed),
                    })
                }
                None => None,
            };
            Request::UserSearch { uid, limit }
        }
        _ => return Err(Unrouted::NotFound),
    };
    match method {
        Method::Get => Ok(Route::Ready(request)),
        Method::Post | Method::Put | Method::Patch | Method::Delete => Err(Unrouted::Method),
    }
}
fn query(pairs: &[Pair], key: &[u8]) -> Option<Box<[u8]>> {
    for pair in pairs {
        if pair.name.as_ref() == key {
            return Some(pair.value.clone());
        }
    }
    None
}
pub(super) fn user(json: &mut Encoder, user: &User) {
    json.object_start();
    json.key(b"id");
    json.unsigned(user.id);
    json.key(b"login");
    json.string(&user.login);
    json.object_end();
}
pub(super) fn write(json: &mut Encoder, answer: &Answer) {
    match answer {
        Answer::Reachable(rows) => reachable(json, rows),
        Answer::RepoSearch { ok, data } => repo_search(json, *ok, data),
        Answer::User(value) => {
            json.object_start();
            json.key(b"id");
            json.unsigned(value.id);
            json.key(b"login");
            json.string(&value.login);
            json.key(b"is_admin");
            json.boolean(value.is_admin);
            json.key(b"full_name");
            json.string(&value.full_name);
            json.object_end();
        }
        Answer::ApiSettings(value) => {
            json.object_start();
            json.key(b"max_response_items");
            json.unsigned(u64::from(value.max_response_items));
            json.key(b"default_paging_num");
            json.unsigned(u64::from(value.default_paging_num));
            json.object_end();
        }
        Answer::Repository(value) => {
            json.object_start();
            json.key(b"id");
            json.unsigned(value.id);
            json.key(b"owner");
            user(json, &value.owner);
            json.key(b"name");
            json.string(&value.name);
            json.key(b"object_format_name");
            json.string(match value.object_format {
                ObjectFormat::Sha1 => b"sha1",
                ObjectFormat::Sha256 => b"sha256",
            });
            json.key(b"default_branch");
            json.string(&value.default_branch);
            json.key(b"default_merge_style");
            json.string(document::style_text(value.default_merge_style));
            json.key(b"allow_merge_commits");
            json.boolean(value.allow_merge_commits);
            json.key(b"allow_squash_merge");
            json.boolean(value.allow_squash_merge);
            json.key(b"allow_rebase");
            json.boolean(value.allow_rebase);
            json.key(b"html_url");
            json.string(&value.html_url);
            json.object_end();
        }
        Answer::Search { ok, data } => {
            json.object_start();
            json.key(b"data");
            json.array_start();
            for value in data {
                user(json, value);
            }
            json.array_end();
            json.key(b"ok");
            json.boolean(*ok);
            json.object_end();
        }
        Answer::Token(_)
        | Answer::Tokens(_)
        | Answer::Error(_)
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
        | Answer::Reviews(_)
        | Answer::ReviewComments(_)
        | Answer::Branch(_)
        | Answer::BranchRefs(_)
        | Answer::Branches(_)
        | Answer::Protection(_)
        | Answer::Protections(_)
        | Answer::Collaborators(_)
        | Answer::Permission(_)
        | Answer::Pulls(_)
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
        | Answer::CombinedStatus { .. } => unreachable!("start answer dispatch"),
    }
}

fn reachable(json: &mut Encoder, rows: &[document::Reachable]) {
    json.array_start();
    for row in rows {
        json.object_start();
        json.key(b"id");
        json.unsigned(row.repository);
        json.key(b"name");
        json.string(&row.name);
        json.key(b"private");
        json.boolean(row.private);
        json.key(b"owner");
        json.object_start();
        json.key(b"id");
        json.unsigned(row.owner);
        json.key(b"login");
        json.string(&row.login);
        json.object_end();
        json.key(b"permissions");
        json.object_start();
        json.key(b"push");
        json.boolean(row.push);
        json.object_end();
        json.object_end();
    }
    json.array_end();
}

fn repo_search(json: &mut Encoder, ok: bool, rows: &[document::Reachable]) {
    json.object_start();
    json.key(b"ok");
    json.boolean(ok);
    json.key(b"data");
    reachable(json, rows);
    json.object_end();
}
