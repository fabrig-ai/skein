//! The task's route subset; credentials precede body decoding.
use crate::{reads, writes};
use alloc::boxed::Box;
use skein_forge::github::{
    limits::Limits,
    request::{Method, Request},
    server,
};

#[derive(Debug)]
pub(crate) enum Route {
    App(App),
    Token(Token),
    Conversion(skein_lib::Secret),
    BotUser(Box<[u8]>),
}
#[derive(Debug)]
pub(crate) enum App {
    LogPage { ask: skein_forge::github::log::Ask, per_page: u32 },
    LogDelivery(u64),
    Identity,
    Hook,
    SetHook,
    Installations(u32),
    Installation(u64),
    Mint(u64),
    Uninstall(u64),
}
#[derive(Debug)]
pub(crate) enum Token {
    Mutation(crate::mutations::Route),
    CommentEdit(crate::comment_edit::Route),
    Reachable(u32),
    Identity,
    Revoke,
    EditReach { installation: u64, repository: u32, add: bool },
    Graphql,
    User(u64),
    Read(reads::Read),
    Write(writes::Route),
}

#[expect(clippy::too_many_lines, reason = "exhaustive selection of the served dialect routes")]
pub(crate) fn decode(method: Method, target: &[u8], limits: &Limits) -> Option<Route> {
    if target.len() > usize::try_from(limits.target_bytes).ok()? {
        return None;
    }
    if let Some(route) = crate::comment_edit::route(method, target) {
        return Some(Route::Token(Token::CommentEdit(route)));
    }
    if let Some(route) = crate::mutations::route(method, target) {
        return Some(Route::Token(Token::Mutation(route)));
    }
    if method == Method::Get && target == b"/user" {
        return Some(Route::Token(Token::Identity));
    }
    if method == Method::Delete && target == b"/installation/token" {
        return Some(Route::Token(Token::Revoke));
    }
    if let Some(tail) = target.strip_prefix(b"/user/installations/") {
        let split = skein_lib::bytes::find(tail, b"/")?;
        let installation = tail.get(..split)?;
        let repository = tail.get(split.checked_add(1)?..)?;
        let repository = repository.strip_prefix(b"repositories/")?;
        let installation = skein_forge::forgejo::id::unsigned(installation)?;
        let repository = u32::try_from(skein_forge::forgejo::id::unsigned(repository)?).ok()?;
        let add = match method {
            Method::Put => true,
            Method::Delete => false,
            Method::Get | Method::Post | Method::Patch => return None,
        };
        return Some(Route::Token(Token::EditReach { installation, repository, add }));
    }
    if method == Method::Post && target == b"/graphql" {
        return Some(Route::Token(Token::Graphql));
    }
    // REST keeps server::route's canonical targets, including explicit paging.
    Some(match server::route(method, target, limits)? {
        Request::BotUser { slug } => Route::BotUser(slug),
        Request::Conversion { code } => Route::Conversion(skein_lib::Secret::new(code)),
        Request::LogPage { ask, per_page } => Route::App(App::LogPage { ask, per_page }),
        Request::LogDelivery { id, .. } => Route::App(App::LogDelivery(id)),
        Request::App => Route::App(App::Identity),
        Request::HookConfig => Route::App(App::Hook),
        Request::SetHook { .. } => Route::App(App::SetHook),
        Request::Installations { page } if page > 0 => Route::App(App::Installations(page)),
        Request::Installation { id } => Route::App(App::Installation(id)),
        Request::Mint { installation, .. } => Route::App(App::Mint(installation)),
        Request::Uninstall { id } => Route::App(App::Uninstall(id)),
        Request::Reachable { page, .. } if page > 0 => Route::Token(Token::Reachable(page)),
        Request::Repository { id } => Route::Token(Token::Read(reads::Read::Repository(id))),
        Request::Locate { owner, name } => Route::Token(Token::Read(reads::Read::Locate { owner, name })),
        Request::Branch { owner, name, branch } => {
            Route::Token(Token::Read(reads::Read::Branch { owner, name, branch }))
        }
        Request::Branches { owner, name, page, per_page } if page > 0 && per_page > 0 => {
            Route::Token(Token::Read(reads::Read::Branches { owner, name, page, per_page }))
        }
        Request::Issues { owner, name, since, page, per_page } if page > 0 && per_page > 0 => {
            Route::Token(Token::Read(reads::Read::Issues { owner, name, since, page, per_page }))
        }
        Request::Issue { owner, name, number } => Route::Token(Token::Read(reads::Read::Issue { owner, name, number })),
        Request::Comments { owner, name, number, page, per_page } if page > 0 && per_page > 0 => {
            Route::Token(Token::Read(reads::Read::Comments { owner, name, number, page, per_page }))
        }
        Request::Pull { owner, name, number } => Route::Token(Token::Read(reads::Read::Pull { owner, name, number })),
        Request::PullsFor { owner, name, head, base, open } => {
            Route::Token(Token::Read(reads::Read::PullsFor { owner, name, head, base, open }))
        }
        Request::PullFiles { owner, name, number, page, per_page } if page > 0 && per_page > 0 => {
            Route::Token(Token::Read(reads::Read::PullFiles { owner, name, number, page, per_page }))
        }
        Request::Compare { owner, name, before, after, per_page } if per_page > 0 => {
            Route::Token(Token::Read(reads::Read::Compare { owner, name, before, after, per_page }))
        }
        Request::Raw { owner, name, commit, path } => {
            Route::Token(Token::Read(reads::Read::Raw { owner, name, commit, path }))
        }
        Request::CombinedStatus { owner, name, commit, page, per_page } if page > 0 && per_page > 0 => {
            Route::Token(Token::Read(reads::Read::Checks { owner, name, commit, page, per_page, runs: false }))
        }
        Request::CheckRuns { owner, name, commit, page, per_page } if page > 0 && per_page > 0 => {
            Route::Token(Token::Read(reads::Read::Checks { owner, name, commit, page, per_page, runs: true }))
        }
        Request::JobLog { owner, name, id } => Route::Token(Token::Read(reads::Read::JobLog { owner, name, id })),
        Request::Job { owner, name, id } => Route::Token(Token::Read(reads::Read::Job { owner, name, id })),
        Request::User { id } => Route::Token(Token::User(id)),
        Request::Collaborators { owner, name, page, per_page } if page > 0 && per_page > 0 => {
            Route::Token(Token::Read(reads::Read::Collaborators { owner, name, page, per_page }))
        }
        Request::Permission { owner, name, login } => {
            Route::Token(Token::Read(reads::Read::Permission { owner, name, login }))
        }
        Request::Reviews { owner, name, number, page, per_page } if page > 0 && per_page > 0 => {
            Route::Token(Token::Read(reads::Read::Reviews { owner, name, number, page, per_page }))
        }
        Request::ReviewComments { owner, name, number, review, page, per_page } if page > 0 && per_page > 0 => {
            Route::Token(Token::Read(reads::Read::ReviewComments { owner, name, number, review }))
        }
        Request::RulesForBranch { owner, name, branch, page } if page > 0 => {
            Route::Token(Token::Read(reads::Read::Protection {
                owner,
                name,
                query: reads::protection::Read::Rules { branch, page, target: skein_lib::bytes::copy_of(target) },
            }))
        }
        Request::Ruleset { owner, name, id } => Route::Token(Token::Read(reads::Read::Protection {
            owner,
            name,
            query: reads::protection::Read::Ruleset(id),
        })),
        Request::ClassicProtection { owner, name, branch } => Route::Token(Token::Read(reads::Read::Protection {
            owner,
            name,
            query: reads::protection::Read::Classic(branch),
        })),
        Request::Merge { owner, name, number, head, method } => {
            Route::Token(Token::Write(writes::Route::Merge { owner, name, number, head, method }))
        }
        Request::CreateBranch { owner, name, branch, commit } => {
            Route::Token(Token::Write(writes::Route::CreateBranch { owner, name, branch, commit }))
        }
        Request::DeleteBranch { owner, name, branch } => {
            Route::Token(Token::Write(writes::Route::DeleteBranch { owner, name, branch }))
        }
        Request::Status { owner, name, commit, context, check } => {
            Route::Token(Token::Write(writes::Route::Status { owner, name, commit, context, check }))
        }
        Request::OpenPull { owner, name, title, body, head, base } => {
            Route::Token(Token::Write(writes::Route::OpenPull { owner, name, title, body, head, base }))
        }
        Request::Update { owner, name, number, head } => {
            Route::Token(Token::Write(writes::Route::Update { owner, name, number, head }))
        }
        Request::Review { owner, name, number, verdict, body } => {
            Route::Token(Token::Write(writes::Route::Review { owner, name, number, verdict, body }))
        }
        Request::Reviewers { owner, name, number, add, logins } => {
            Route::Token(Token::Write(writes::Route::Reviewers { owner, name, number, add, logins }))
        }
        Request::Reopen { owner, name, number } => {
            Route::Token(Token::Write(writes::Route::Reopen { owner, name, number }))
        }
        Request::Close { owner, name, number } => {
            Route::Token(Token::Write(writes::Route::Close { owner, name, number }))
        }
        Request::Edit { owner, name, number, title, body } => {
            Route::Token(Token::Write(writes::Route::Edit { owner, name, number, title, body }))
        }
        Request::Post { owner, name, number, body } => {
            Route::Token(Token::Write(writes::Route::Post { owner, name, number, body }))
        }
        Request::CreateIssue { owner, name, title, body } => {
            Route::Token(Token::Write(writes::Route::CreateIssue { owner, name, title, body }))
        }
        Request::Installations { .. }
        | Request::Reachable { .. }
        | Request::Branches { .. }
        | Request::Viewer
        | Request::Issues { .. }
        | Request::Comments { .. }
        | Request::Reviews { .. }
        | Request::ReviewComments { .. }
        | Request::CombinedStatus { .. }
        | Request::CheckRuns { .. }
        | Request::Followed { .. }
        | Request::PullFiles { .. }
        | Request::Compare { .. }
        | Request::RulesForBranch { .. }
        | Request::Collaborators { .. }
        | Request::Enqueue { .. }
        | Request::PullGraphql { .. } => return None,
    })
}
