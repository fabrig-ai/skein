//! One call's exchanges and its single terminal answer (forge.md, 3.2).
use super::{Heard, reads, tables::Tables, writes};
use crate::{api, github::request};
use alloc::boxed::Box;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Exchange {
    pub request: request::Request,
    /// Raw retention to apply while reading through the exchange body.
    pub retention: Option<Retention>,
    /// Selects comparison fields without changing request kind dispatch.
    pub comparison: Comparison,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Step {
    Send(Exchange),
    /// Follow one admitted log origin, then return its head and raw body as Heard.
    Redirect {
        redirect: crate::github::redirect::Redirect,
        retention: Retention,
    },
    Done(Result<api::Answer, api::Error>),
}
impl Step {
    #[must_use]
    pub fn send(request: request::Request) -> Self {
        Self::Send(Exchange { request, retention: None, comparison: Comparison::Full })
    }
}

/// The owner's checked values for this call; body bytes are bounded by its decoder.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Bounds {
    /// Maximum distinct protection rulesets, from the dialect document limit.
    pub inner: u32,
    pub page_size: u32,
    pub rows: u32,
    pub answer_bytes: u32,
    pub body_bytes: u32,
}
#[derive(Debug)]
pub enum Plan {
    Refused,
    Read(Box<reads::Plan>),
    Write(Box<writes::Plan>),
}
impl Plan {
    #[must_use]
    pub fn new(repository: api::Repository, op: api::Op) -> Self {
        let refused = match &op {
            api::Op::Read(read) => match read {
                api::Read::Runs { .. } | api::Read::Run { .. } | api::Read::Tree { .. } => true,
                api::Read::Locate { .. }
                | api::Read::Items { .. }
                | api::Read::Item { .. }
                | api::Read::Pull { .. }
                | api::Read::PullFor { .. }
                | api::Read::Reviews { .. }
                | api::Read::Statuses { .. }
                | api::Read::Remarks { .. }
                | api::Read::Branch { .. }
                | api::Read::Branches { .. }
                | api::Read::PullFiles { .. }
                | api::Read::Compare { .. }
                | api::Read::Checks { .. }
                | api::Read::Job { .. }
                | api::Read::File { .. }
                | api::Read::Protection { .. }
                | api::Read::Settings
                | api::Read::Collaborators { .. }
                | api::Read::Permission { .. } => false,
            },
            api::Op::Write(write) => match write {
                api::Write::CreateIssue { labels, .. } => !labels.is_empty(),
                api::Write::Labels { .. }
                | api::Write::CreateLabel { .. }
                | api::Write::Dispatch { .. }
                | api::Write::Rerun { .. }
                | api::Write::Merge { how: api::MergeHow::FastForwardOnly, .. }
                | api::Write::Update { style: api::UpdateStyle::Rebase, .. } => true,
                api::Write::OpenPull { .. }
                | api::Write::Post { .. }
                | api::Write::Review { .. }
                | api::Write::Edit { .. }
                | api::Write::SetReviewers { .. }
                | api::Write::Close { .. }
                | api::Write::Reopen { .. }
                | api::Write::Merge { how: api::MergeHow::Style, .. }
                | api::Write::Update { style: api::UpdateStyle::Merge, .. }
                | api::Write::Status { .. }
                | api::Write::CreateBranch { .. }
                | api::Write::DeleteBranch { .. } => false,
            },
        };
        if refused {
            return Self::Refused;
        }
        match op {
            api::Op::Read(read) => Self::Read(Box::new(reads::Plan::new(repository, read))),
            api::Op::Write(write) => Self::Write(Box::new(writes::Plan::new(repository, write))),
        }
    }
    pub fn begin(&mut self, bounds: Bounds, word: &[u8], tables: &mut Tables) -> Result<Step, api::Error> {
        match self {
            Self::Refused => Ok(Step::Done(Err(api::Error::Refused))),
            Self::Read(plan) => plan.begin(bounds, word, tables),
            Self::Write(plan) => plan.begin(bounds, word, tables),
        }
    }
    /// `Err` rejects an out-of-order call; a plan's verdict is `Step::Done`.
    pub fn heard(
        &mut self,
        heard: Heard,
        bounds: Bounds,
        word: &[u8],
        tables: &mut Tables,
    ) -> Result<Option<Step>, api::Error> {
        match self {
            Self::Refused => Err(api::Error::Refused),
            Self::Read(plan) => plan.heard(heard, bounds, word, tables),
            Self::Write(plan) => plan.heard(heard, bounds, word, tables),
        }
    }
    /// Takes the exchange the caller retained with `set_pending`, as in Forgejo.
    pub fn pending(&mut self) -> Option<Exchange> {
        match self {
            Self::Refused => None,
            Self::Read(plan) => plan.pending(),
            Self::Write(plan) => plan.pending(),
        }
    }
    /// The caller retains a sent exchange here if it may need to resend it.
    pub fn set_pending(&mut self, exchange: Exchange) {
        match self {
            Self::Refused => {}
            Self::Read(plan) => plan.set_pending(exchange),
            Self::Write(plan) => plan.set_pending(exchange),
        }
    }
}

/// Raw bodies are read through within the component's input bound.
/// Head and tail retain at most the cap; Whole reports overflow through `Body::Raw.more`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Retention {
    Head { bytes: u32 },
    Tail { bytes: u32 },
    Whole { bytes: u32 },
}

/// A pull's merge-base lookup skips comparison completeness fields and lists.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Comparison {
    Full,
    MergeBase,
}
impl Exchange {
    /// Selects the successful response decoder; error responses use `Kind::Error`.
    #[must_use]
    pub fn decoder(&self, limits: &crate::github::limits::Limits) -> Option<crate::github::decode::Decoder> {
        let kind = crate::github::decode::kind(&self.request)?;
        if let request::Request::LogDelivery { event, .. } = &self.request {
            return Some(crate::github::decode::log_delivery_decoder(event, limits));
        }
        if kind == crate::github::document::Kind::Compare && self.comparison == Comparison::MergeBase {
            Some(crate::github::decode::merge_base_decoder(limits))
        } else {
            Some(crate::github::decode::decoder(kind, limits))
        }
    }
}
