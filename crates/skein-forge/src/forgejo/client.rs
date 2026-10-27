//! The engine's side: request spelling and streaming answer decoding.
use crate::forgejo::{
    decode::{
        Step,
        walk::{Item, Kind, Walk},
    },
    limits::Limits,
    request::{Method, Request},
};
use alloc::boxed::Box;
use skein_json::tokenizer;
pub(crate) mod actions;
pub(crate) mod branches;
mod create_file;
pub mod delivery;
pub(crate) mod files;
pub(crate) mod items;
use actions::ActionKind;
use files::FileKind;
pub(crate) mod reviews;
use branches::BranchKind;
use reviews::ReviewKind;
pub(crate) mod hooks;
pub(crate) mod orgs;
pub(crate) mod reachable;
pub(crate) mod repositories;
pub(crate) mod setup;
mod start;
pub(crate) mod writes;
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Encoded {
    pub method: Method,
    pub target: Box<[u8]>,
    pub body: Option<Box<[u8]>>,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    Text,
    TooLong,
}
pub fn encode(request: &Request, limits: &Limits) -> Result<Encoded, Refusal> {
    match request {
        Request::CreateFile { .. } => create_file::encode(request, limits),
        Request::AddCollaborator { .. }
        | Request::RepoHooks { .. }
        | Request::CreateRepoHook { .. }
        | Request::EditRepoHook { .. }
        | Request::DeleteRepoHook { .. }
        | Request::OrgHooks { .. }
        | Request::CreateOrgHook { .. }
        | Request::EditOrgHook { .. }
        | Request::DeleteOrgHook { .. }
        | Request::Org { .. }
        | Request::CreateOrg { .. }
        | Request::CreateRepository { .. }
        | Request::Teams { .. }
        | Request::CreateTeam { .. }
        | Request::EditTeam { .. }
        | Request::TeamMembers { .. }
        | Request::AddMember { .. } => orgs::encode(request, limits),
        Request::Tokens { .. }
        | Request::CreateToken { .. }
        | Request::DeleteToken { .. }
        | Request::UserOf { .. }
        | Request::CreateUser { .. } => setup::encode(request, limits),
        Request::CreateIssue { .. }
        | Request::OpenPull { .. }
        | Request::PostComment { .. }
        | Request::PostReview { .. }
        | Request::EditIssue { .. }
        | Request::RequestReviewers { .. }
        | Request::RemoveReviewers { .. }
        | Request::Dispatch { .. }
        | Request::Rerun { .. }
        | Request::Merge { .. }
        | Request::Update { .. }
        | Request::PostStatus { .. }
        | Request::CreateBranch { .. }
        | Request::DeleteBranch { .. } => writes::encode(request, limits),
        Request::PullFiles { .. }
        | Request::Compare { .. }
        | Request::Commits { .. }
        | Request::Raw { .. }
        | Request::Tree { .. }
        | Request::WorkflowRuns { .. }
        | Request::Runs { .. }
        | Request::Run { .. }
        | Request::Job { .. }
        | Request::Jobs { .. }
        | Request::JobLog { .. } => files::encode(request, limits),
        Request::Reviews { .. }
        | Request::ReviewComments { .. }
        | Request::Branch { .. }
        | Request::Branches { .. }
        | Request::Protection { .. }
        | Request::Protections { .. }
        | Request::Collaborators { .. }
        | Request::Permission { .. }
        | Request::Pulls { .. } => reviews::encode(request, limits),
        Request::User
        | Request::ApiSettings
        | Request::RepositoryById { .. }
        | Request::Repository { .. }
        | Request::UserRepos { .. }
        | Request::RepoSearch { .. }
        | Request::UserSearch { .. } => start::encode(request, limits),
        Request::Issues { .. }
        | Request::Issue { .. }
        | Request::Comments { .. }
        | Request::Pull { .. }
        | Request::PullFor { .. }
        | Request::CombinedStatus { .. } => items::encode(request, limits),
    }
}
#[derive(Debug)]
pub struct Decoder {
    walk: Walk,
    state: DecoderState,
}
#[derive(Debug)]
enum DecoderState {
    Error(writes::Error),
    Start(start::Start),
    CreatedRepository(repositories::CreatedRepository),
    CreatedFile(Option<crate::forgejo::id::ObjectId>),
    Reachable(reachable::Reachable),
    Setup(setup::Setup),
    Orgs(orgs::Orgs),
    Hooks(hooks::Hooks),
    Items(Box<items::Items>),
    Files(Box<files::Files>),
    Actions(actions::Actions),
    Reviews(Box<reviews::Reviews>),
    Branches(Box<branches::Branches>),
}
#[must_use]
pub fn decoder(request: &Request, limits: &Limits) -> Option<Decoder> {
    let kind = match request {
        Request::Commits { .. } => Kind::File(FileKind::Range),
        Request::CreateFile { .. } => Kind::Setup(setup::SetupKind::CreatedFile),
        Request::CreateRepository { .. } => Kind::Setup(setup::SetupKind::CreatedRepository),
        Request::Org { .. } | Request::CreateOrg { .. } => Kind::Setup(setup::SetupKind::Org),
        Request::CreateTeam { .. } | Request::EditTeam { .. } => Kind::Setup(setup::SetupKind::Team),
        Request::Teams { .. } => Kind::Setup(setup::SetupKind::Teams),
        Request::OrgHooks { .. } | Request::RepoHooks { .. } => Kind::Setup(setup::SetupKind::Hooks),
        Request::CreateOrgHook { .. }
        | Request::EditOrgHook { .. }
        | Request::CreateRepoHook { .. }
        | Request::EditRepoHook { .. } => Kind::Setup(setup::SetupKind::Hook),
        Request::DeleteRepoHook { .. }
        | Request::AddCollaborator { .. }
        | Request::DeleteOrgHook { .. }
        | Request::DeleteToken { .. }
        | Request::AddMember { .. }
        | Request::Raw { .. }
        | Request::JobLog { .. }
        | Request::Update { .. }
        | Request::DeleteBranch { .. }
        | Request::Dispatch { .. }
        | Request::Rerun { .. }
        | Request::Merge { .. }
        | Request::RequestReviewers { .. }
        | Request::RemoveReviewers { .. } => return None,
        Request::Tree { .. } => Kind::File(FileKind::Tree),
        Request::PullFiles { .. } => Kind::File(FileKind::Files),
        Request::Compare { .. } => Kind::File(FileKind::Compare),
        Request::WorkflowRuns { .. } | Request::Runs { .. } => Kind::Action(ActionKind::Listing),
        Request::Run { .. } => Kind::Action(ActionKind::Run),
        Request::Job { .. } => Kind::Action(ActionKind::Job),
        Request::Jobs { .. } => Kind::Action(ActionKind::Jobs),
        Request::PostComment { .. } => Kind::Item(Item::Comment),
        Request::PostStatus { .. } => Kind::Item(Item::Status),
        Request::PostReview { .. } => Kind::Review(ReviewKind::Review),
        Request::Reviews { .. } => Kind::Review(ReviewKind::Reviews),
        Request::ReviewComments { .. } => Kind::Review(ReviewKind::Comments),
        Request::Branch { .. } | Request::CreateBranch { .. } => Kind::Branch(BranchKind::Branch),
        Request::Branches { prefix, .. } => {
            Kind::Branch(if prefix.is_some() { BranchKind::Refs } else { BranchKind::Branches })
        }
        Request::Protection { .. } => Kind::Branch(BranchKind::Protection),
        Request::Protections { .. } => Kind::Branch(BranchKind::Protections),
        Request::Permission { .. } => Kind::Branch(BranchKind::Permission),
        Request::TeamMembers { .. } | Request::Collaborators { .. } => Kind::Users,
        Request::Pulls { .. } => Kind::Item(Item::Pulls),
        Request::UserRepos { .. } => Kind::Reachable(reachable::ReachableKind::Listing),
        Request::UserOf { .. } | Request::CreateUser { .. } | Request::User => Kind::Setup(setup::SetupKind::Account),
        Request::Tokens { .. } => Kind::Setup(setup::SetupKind::Tokens),
        Request::CreateToken { .. } => Kind::Setup(setup::SetupKind::Token),
        Request::ApiSettings => Kind::Settings,
        Request::RepositoryById { .. } | Request::Repository { .. } => Kind::Repository,
        Request::RepoSearch { .. } => Kind::Reachable(reachable::ReachableKind::Search),
        Request::UserSearch { .. } => Kind::Search,
        Request::Issues { .. } => Kind::Item(Item::Issues),
        Request::Issue { .. } | Request::CreateIssue { .. } | Request::EditIssue { .. } => Kind::Item(Item::Issue),
        Request::Comments { .. } => Kind::Item(Item::Comments),
        Request::Pull { .. } | Request::PullFor { .. } | Request::OpenPull { .. } => Kind::Item(Item::Pull),
        Request::CombinedStatus { .. } => Kind::Item(Item::Combined),
    };
    let state = match kind {
        Kind::Setup(
            kind @ (setup::SetupKind::Org | setup::SetupKind::Team | setup::SetupKind::Teams | setup::SetupKind::Units),
        ) => DecoderState::Orgs(orgs::Orgs::new(kind)),
        Kind::Setup(kind @ (setup::SetupKind::Hook | setup::SetupKind::Hooks)) => {
            DecoderState::Hooks(hooks::Hooks::new(kind, limits))
        }
        Kind::Setup(setup::SetupKind::CreatedFile | setup::SetupKind::FileCommit) => DecoderState::CreatedFile(None),
        Kind::Setup(setup::SetupKind::CreatedRepository) => {
            DecoderState::CreatedRepository(repositories::CreatedRepository::new())
        }
        Kind::Setup(kind) => DecoderState::Setup(setup::Setup::new(kind, limits)),
        Kind::Reachable(kind) => DecoderState::Reachable(reachable::Reachable::new(kind)),
        Kind::Error => DecoderState::Error(writes::Error::new()),
        Kind::Write(_) | Kind::Delivery(_) => unreachable!("answer kind selection"),
        Kind::File(kind) => DecoderState::Files(Box::new(files::Files::new(kind, limits.inner))),
        Kind::Action(kind) => DecoderState::Actions(actions::Actions::new(kind)),
        Kind::Review(kind) => DecoderState::Reviews(Box::new(reviews::Reviews::new(kind))),
        Kind::Branch(kind) => DecoderState::Branches(Box::new(branches::Branches::new(kind, limits))),
        Kind::Item(item) => DecoderState::Items(Box::new(items::Items::new(item, limits))),
        Kind::User | Kind::Settings | Kind::Repository | Kind::Search | Kind::Users => {
            DecoderState::Start(start::Start::new(kind))
        }
    };
    Some(Decoder { walk: Walk::new(kind, limits), state })
}
#[must_use]
pub fn first(_decoder: &Decoder) -> tokenizer::Request {
    tokenizer::Request::Next
}
/// Decodes error JSON. A malformed body yields no document and no message;
/// the caller reads its status alone, including for an empty or non-JSON body.
#[must_use]
pub fn error_decoder(limits: &Limits) -> Decoder {
    Decoder { walk: Walk::new(Kind::Error, limits), state: DecoderState::Error(writes::Error::new()) }
}
pub fn step(decoder: &mut Decoder, limits: &Limits, event: &tokenizer::Event) -> Step {
    match decoder.walk.step(event, limits) {
        Err(error) => Step::Malformed(error),
        Ok(movement) => match &mut decoder.state {
            DecoderState::Hooks(state) => state.step(movement, limits, &decoder.walk),
            DecoderState::Orgs(state) => state.step(movement),
            DecoderState::Setup(state) => state.step(movement, limits, &decoder.walk),
            DecoderState::Error(state) => state.step(movement),
            DecoderState::Files(state) => state.step(movement, &decoder.walk),
            DecoderState::Actions(state) => state.step(movement, &decoder.walk),
            DecoderState::Reachable(state) => state.step(movement, limits),
            DecoderState::CreatedRepository(state) => state.step(movement),
            DecoderState::CreatedFile(commit) => create_file::step(commit, movement),
            DecoderState::Start(state) => state.step(movement, decoder.walk.parent()),
            DecoderState::Reviews(state) => state.step(movement, &decoder.walk),
            DecoderState::Branches(state) => state.step(movement, &decoder.walk, limits),
            DecoderState::Items(state) => state.step(movement, &mut decoder.walk),
        },
    }
}
/// Skips earlier comments, including their bodies, before the first step.
pub fn keep_after(decoder: &mut Decoder, after: u64) {
    match &mut decoder.state {
        DecoderState::Items(state) => state.keep_after(after),
        DecoderState::Reachable(_)
        | DecoderState::Hooks(_)
        | DecoderState::Orgs(_)
        | DecoderState::Setup(_)
        | DecoderState::Error(_)
        | DecoderState::Files(_)
        | DecoderState::Actions(_)
        | DecoderState::CreatedFile(_)
        | DecoderState::CreatedRepository(_)
        | DecoderState::Start(_)
        | DecoderState::Reviews(_)
        | DecoderState::Branches(_) => {}
    }
}
/// Stops emitting rows from an unpaged thread and counts its remaining elements.
pub fn cut(decoder: &mut Decoder) -> tokenizer::Request {
    decoder.walk.cut()
}
