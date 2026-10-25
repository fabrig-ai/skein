//! The App's setup exchanges, without repository tables (forge-github.md, 8.1–8.4).
use super::{Body, Heard, failure, plan};
use crate::{
    api,
    github::{
        document::{self, Document, Row},
        failure::{Asked, Verdict, classify},
        header::Head,
        request,
    },
    measure,
};
use alloc::boxed::Box;
use skein_lib::List;

mod identity;
mod installations;
mod listing;
mod log_delivery;
mod log_page;
mod minting;
mod reachable;
pub use identity::Identity;
pub use installations::Installations;
pub use listing::Listing;
pub use log_delivery::LogDelivery;
pub use log_page::LogPage;
pub use minting::Minting;
pub use reachable::Reachable;

/// Each setup plan has one concrete outcome, not an outcome enum or `api::Answer`.
/// Only setup outcomes are generic; forge-operation plans use `plan::Step`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Step<T> {
    Send(plan::Exchange),
    Done(Result<T, api::Error>),
}
impl<T> Step<T> {
    #[must_use]
    pub fn send(request: request::Request) -> Self {
        Self::Send(plan::Exchange { request, retention: None, comparison: plan::Comparison::Full })
    }
    fn terminal(state: &mut State, answer: Result<T, api::Error>) -> Self {
        state.stage = Stage::Done;
        state.response = Response::None;
        Self::Done(answer)
    }
    fn settle(state: &mut State, result: Result<Option<Self>, api::Error>) -> Option<Self> {
        match result {
            Ok(step) => step,
            Err(error) => Some(Self::terminal(state, Err(error))),
        }
    }
}

/// One picker page; only a new plan may ask for the next page.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Page {
    pub rows: Box<[document::Reachable]>,
    pub next: bool,
}

/// One page of the App's installations; only a new plan may ask for the next page.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct InstallationPage {
    pub rows: Box<[document::Installation]>,
    pub next: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Stage {
    New,
    Read,
    Done,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Response {
    None,
    Head,
    Body { next: bool },
    Refusal { status: u16, head: Head },
}
#[derive(Debug)]
struct State {
    stage: Stage,
    response: Response,
    request: Option<plan::Exchange>,
}
#[derive(Debug)]
enum Event {
    Row(Row),
    Done { document: Document, next: bool },
}
impl State {
    fn new() -> Self {
        Self { stage: Stage::New, response: Response::None, request: None }
    }
    fn admission(&self) -> Result<(), api::Error> {
        if self.stage != Stage::New {
            return Err(api::Error::InvalidAnswer);
        }
        Ok(())
    }
    fn sending(&mut self) {
        self.stage = Stage::Read;
        self.response = Response::Head;
    }
    fn order(&self, heard: &Heard) -> Result<(), api::Error> {
        if self.stage == Stage::New || self.stage == Stage::Done {
            return Err(api::Error::InvalidAnswer);
        }
        // The read plans' ordering table: call-order errors do not consume state.
        match (heard, &self.response) {
            (Heard::Head { .. }, Response::Head)
            | (Heard::Row(_) | Heard::Done(_), Response::Body { .. } | Response::Refusal { .. })
            | (Heard::Failed(_), Response::Head | Response::Body { .. } | Response::Refusal { .. }) => Ok(()),
            (Heard::Head { .. }, Response::None | Response::Body { .. } | Response::Refusal { .. })
            | (Heard::Row(_) | Heard::Done(_), Response::None | Response::Head)
            | (Heard::Failed(_), Response::None) => Err(api::Error::InvalidAnswer),
        }
    }
    fn hear(&mut self, heard: Heard, success: u16) -> Result<Option<Event>, api::Error> {
        match heard {
            Heard::Head { status, head } => {
                if status == success {
                    self.response = Response::Body { next: head.next };
                    return Ok(None);
                }
                // Limit statuses wait for refusal text. A failed body keeps the status.
                if status == 403 || status == 429 {
                    self.response = Response::Refusal { status, head };
                    return Ok(None);
                }
                Err(refused(status, &head, None))
            }
            Heard::Row(row) => match self.response {
                Response::Refusal { .. } => Ok(None),
                Response::Body { .. } => Ok(Some(Event::Row(row))),
                Response::None | Response::Head => Err(api::Error::InvalidAnswer),
            },
            Heard::Done(body) => {
                let next = match &self.response {
                    Response::Refusal { status, head } => return Err(refused(*status, head, refusal(&body))),
                    Response::Body { next } => *next,
                    Response::None | Response::Head => return Err(api::Error::InvalidAnswer),
                };
                match body {
                    Body::Document(document) => Ok(Some(Event::Done { document, next })),
                    Body::Malformed(malformed) => Err(failure::malformed(malformed)),
                    Body::Empty | Body::Raw { .. } => Err(api::Error::InvalidAnswer),
                }
            }
            Heard::Failed(failed) => match &self.response {
                Response::Refusal { status, head } => Err(refused(*status, head, None)),
                Response::None | Response::Head | Response::Body { .. } => Err(failure::transport(failed)),
            },
        }
    }
}
fn refused(status: u16, head: &Head, refusal: Option<&document::Refusal>) -> api::Error {
    // Setup has no repository row to invalidate; the status map is the read map.
    match classify(Asked::Read, status, head, refusal) {
        Verdict::Fail(error, _) => error,
        Verdict::Enqueue | Verdict::BranchExists => api::Error::InvalidAnswer,
    }
}
fn refusal(body: &Body) -> Option<&document::Refusal> {
    match body {
        Body::Document(Document::Error(refusal)) => Some(refusal),
        Body::Document(
            Document::Delivery(_)
            | Document::Installation(_)
            | Document::Minted(_)
            | Document::Viewer(_)
            | Document::Merge(_)
            | Document::Enqueued(_)
            | Document::PullGraphql(_)
            | Document::Graphql(_)
            | Document::App(_)
            | Document::HookConfig(_)
            | Document::Conversion(_)
            | Document::User(_)
            | Document::Compare(_)
            | Document::Permission(_)
            | Document::Id(_)
            | Document::Number(_)
            | Document::Done
            | Document::Ruleset(_)
            | Document::ClassicProtection(_)
            | Document::BotUser(_)
            | Document::Repository(_)
            | Document::Branch(_)
            | Document::Pull(_)
            | Document::Job(_)
            | Document::Combined(_)
            | Document::Issue(_)
            | Document::ActionsPermissions(_)
            | Document::Workflows { .. }
            | Document::Rows,
        )
        | Body::Raw { .. }
        | Body::Empty
        | Body::Malformed(_) => None,
    }
}

fn keep_installation(
    row: Row,
    rows: &mut List<document::Installation>,
    bytes: &mut u32,
    count: &mut u32,
    bounds: plan::Bounds,
) -> Result<(), api::Error> {
    let row = match row {
        Row::Installation(row) => row,
        Row::Log(_)
        | Row::Commit(_)
        | Row::File(_)
        | Row::Review(_)
        | Row::Remark(_)
        | Row::Collaborator(_)
        | Row::Rule(_)
        | Row::Reachable(_)
        | Row::Branch(_)
        | Row::SkippedBranch
        | Row::Issue(_)
        | Row::Pull(_)
        | Row::Context(_)
        | Row::Run(_)
        | Row::Comment(_) => return Err(api::Error::InvalidAnswer),
    };
    *count = count.checked_add(1).ok_or(api::Error::TooLarge)?;
    if *count > crate::github::pin::PAGE_CAP || rows.len() >= bounds.rows {
        return Err(api::Error::TooLarge);
    }
    *bytes = add(*bytes, &row.login, b"", bounds)?;
    match rows.push(row) {
        Ok(()) => Ok(()),
        Err(_) => Err(api::Error::TooLarge),
    }
}

// Answer bytes: one STRING_BYTES overhead per row, plus login for installations,
// or login + name for reachable repositories. All lengths and sums fit u32.
fn capacity(bounds: plan::Bounds) -> Result<u32, api::Error> {
    let width = u32::try_from(measure::STRING_BYTES).or(Err(api::Error::TooLarge))?;
    let room = bounds.answer_bytes.checked_div(width).ok_or(api::Error::TooLarge)?;
    Ok(bounds.rows.min(room))
}
fn add(bytes: u32, login: &[u8], name: &[u8], bounds: plan::Bounds) -> Result<u32, api::Error> {
    let login = u32::try_from(login.len()).or(Err(api::Error::TooLarge))?;
    let name = u32::try_from(name.len()).or(Err(api::Error::TooLarge))?;
    let width = u32::try_from(measure::STRING_BYTES).or(Err(api::Error::TooLarge))?;
    let bytes = bytes
        .checked_add(login)
        .ok_or(api::Error::TooLarge)?
        .checked_add(name)
        .ok_or(api::Error::TooLarge)?
        .checked_add(width)
        .ok_or(api::Error::TooLarge)?;
    if bytes > bounds.answer_bytes {
        return Err(api::Error::TooLarge);
    }
    Ok(bytes)
}
