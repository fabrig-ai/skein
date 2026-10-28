use super::{
    BodyDecoder, Route, Unrouted,
    items::{number_value, page},
    writes::WriteKind,
};
use crate::forgejo::{
    decode::walk::{Kind, Walk},
    document,
    limits::Limits,
    path,
    request::{Method, Request},
};
use alloc::boxed::Box;
use skein_json::writer::Encoder;
use skein_lib::List;
pub(super) fn route(method: Method, bytes: &[u8], limits: &Limits) -> Result<Route, Unrouted> {
    let target = path::read(bytes, limits).ok().ok_or(Unrouted::Malformed)?;
    let [api, v1, tail @ ..] = target.segments.as_ref() else {
        return Err(Unrouted::NotFound);
    };
    if &**api != b"api" || &**v1 != b"v1" {
        return Err(Unrouted::NotFound);
    }
    let (required, kind, request) = match tail {
        [orgs, name] if &**orgs == b"orgs" => (Method::Get, None, Request::Org { name: name.clone() }),
        [admin, users, person, orgs] if &**admin == b"admin" && &**users == b"users" && &**orgs == b"orgs" => {
            (Method::Post, Some(WriteKind::NewOrg), Request::CreateOrg { person: person.clone(), name: Box::from([]) })
        }
        [orgs, org, repos] if &**orgs == b"orgs" && &**repos == b"repos" => (
            Method::Post,
            Some(WriteKind::NewRepository),
            Request::CreateRepository { org: org.clone(), name: Box::from([]), private: false },
        ),
        [orgs, org, teams] if &**orgs == b"orgs" && &**teams == b"teams" => match method {
            Method::Get => (Method::Get, None, Request::Teams { org: org.clone(), page: page(&target.query)? }),
            Method::Post => (
                Method::Post,
                Some(WriteKind::NewTeam),
                Request::CreateTeam { org: org.clone(), spec: document::team().spec },
            ),
            Method::Put | Method::Patch | Method::Delete => return Err(Unrouted::Method),
        },
        [teams, team] if &**teams == b"teams" => (
            Method::Patch,
            Some(WriteKind::NewTeam),
            Request::EditTeam { team: number_value(team)?, spec: document::team().spec },
        ),
        [teams, team, members] if &**teams == b"teams" && &**members == b"members" => {
            (Method::Get, None, Request::TeamMembers { team: number_value(team)?, page: page(&target.query)? })
        }
        [teams, team, members, login] if &**teams == b"teams" && &**members == b"members" => {
            (Method::Put, None, Request::AddMember { team: number_value(team)?, login: login.clone() })
        }
        _ => return Err(Unrouted::NotFound),
    };
    if method != required {
        return Err(Unrouted::Method);
    }
    Ok(match kind {
        Some(kind) => Route::Body(BodyDecoder {
            walk: Walk::new(Kind::Write(kind), limits),
            request,
            texts: List::with_capacity(limits.inner),
            numbers: List::with_capacity(0),
            inputs: List::with_capacity(0),
            terminal: false,
            committer: crate::forgejo::request::Author { name: Box::new([]), email: Box::new([]) },
        }),
        None => Route::Ready(request),
    })
}
pub(super) fn write(json: &mut Encoder, answer: &super::Answer) {
    match answer {
        super::Answer::Labels(_) | super::Answer::Label(_) => unreachable!("label answer has its own writer"),
        super::Answer::Org(org) => {
            json.object_start();
            json.key(b"id");
            json.unsigned(org.id);
            json.key(b"name");
            json.string(&org.name);
            json.object_end();
        }
        super::Answer::Team(value) => team(json, value),
        super::Answer::Teams(teams) => {
            json.array_start();
            for value in teams {
                team(json, value);
            }
            json.array_end();
        }
        super::Answer::Members(users) => {
            json.array_start();
            for user in users {
                super::start::user(json, user);
            }
            json.array_end();
        }
        super::Answer::CreatedFile { .. }
        | super::Answer::CreatedRepository(_)
        | super::Answer::Hook(_)
        | super::Answer::Hooks(_)
        | super::Answer::Token(_)
        | super::Answer::Tokens(_)
        | super::Answer::User(_)
        | super::Answer::ApiSettings(_)
        | super::Answer::Repository(_)
        | super::Answer::Reachable(_)
        | super::Answer::RepoSearch { .. }
        | super::Answer::Search { .. }
        | super::Answer::Issues(_)
        | super::Answer::Issue(_)
        | super::Answer::Comments(_)
        | super::Answer::Comment(_)
        | super::Answer::Status(_)
        | super::Answer::Review(_)
        | super::Answer::Error(_)
        | super::Answer::Pull(_)
        | super::Answer::Entries(_)
        | super::Answer::Files(_)
        | super::Answer::Compare { .. }
        | super::Answer::Runs { .. }
        | super::Answer::Run(_)
        | super::Answer::Job { .. }
        | super::Answer::Jobs(_)
        | super::Answer::Pulls(_)
        | super::Answer::Reviews(_)
        | super::Answer::ReviewComments(_)
        | super::Answer::Branch(_)
        | super::Answer::BranchRefs(_)
        | super::Answer::Branches(_)
        | super::Answer::Protection(_)
        | super::Answer::Protections(_)
        | super::Answer::Collaborators(_)
        | super::Answer::Permission(_)
        | super::Answer::Tree { .. }
        | super::Answer::CombinedStatus { .. } => unreachable!("organization answer selection"),
    }
}
fn team(json: &mut Encoder, value: &document::Team) {
    json.object_start();
    json.key(b"id");
    json.unsigned(value.id);
    crate::forgejo::client::orgs::specification(json, &value.spec, true);
    json.object_end();
}
