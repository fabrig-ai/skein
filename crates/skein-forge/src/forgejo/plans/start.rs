//! Start reads, without scheduling or identity decisions (forge.md, section 4.1).
use super::{Body, Heard as ExchangeHeard, plan::Exchange};
use crate::forgejo::{document::Document, request::Request};
use skein_lib::Duration;

/// One owner-selected start exchange.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Read {
    User,
    ApiSettings,
}

/// Send is answered with exchange events; Done is terminal for this read.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Step {
    Send(Exchange),
    Done(Heard),
}

/// What one start read heard.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Heard {
    /// `/user`'s `id`; the owner checks identity.
    User { id: u64 },
    /// A positive `/settings/api` page cap.
    ApiSettings { max_response_items: u32 },
    /// A non-200 status with its retry wait.
    Status { status: u16, retry: Option<Duration> },
    /// No usable answer.
    Unanswered,
}

/// One read, ending at most once; its owner runs `User` before `ApiSettings`.
#[derive(Debug)]
pub struct Plan {
    read: Read,
    ended: bool,
}

impl Plan {
    #[must_use]
    pub fn new(read: Read) -> Self {
        Self { read, ended: false }
    }

    pub fn begin(&mut self) -> Step {
        let request = match self.read {
            Read::User => Request::User,
            Read::ApiSettings => Request::ApiSettings,
        };
        Step::Send(Exchange { request, range: None })
    }

    pub fn heard(&mut self, heard: ExchangeHeard) -> Option<Step> {
        if self.ended {
            return None;
        }
        let answer = match heard {
            ExchangeHeard::Head { status, fields } if status != 200 => {
                Heard::Status { status, retry: fields.retry_after }
            }
            ExchangeHeard::Head { .. } | ExchangeHeard::Row(_) => return None,
            ExchangeHeard::Done(Body::Document(document)) => self.document(document),
            ExchangeHeard::Done(Body::Empty | Body::Raw { .. } | Body::Malformed(_)) | ExchangeHeard::Failed(_) => {
                Heard::Unanswered
            }
        };
        self.ended = true;
        Some(Step::Done(answer))
    }

    fn document(&self, document: Document) -> Heard {
        match document {
            Document::User(user) if self.read == Read::User => Heard::User { id: user.id },
            Document::ApiSettings(settings) if self.read == Read::ApiSettings && settings.max_response_items > 0 => {
                Heard::ApiSettings { max_response_items: settings.max_response_items }
            }
            Document::User(_)
            | Document::ApiSettings(_)
            | Document::Hook(_)
            | Document::Token(_)
            | Document::Org(_)
            | Document::CreatedFile { .. }
            | Document::CreatedRepository(_)
            | Document::Team(_)
            | Document::Repository(_)
            | Document::RepoSearch { .. }
            | Document::Search { .. }
            | Document::Listed { .. }
            | Document::Issue(_)
            | Document::Comment(_)
            | Document::Status(_)
            | Document::Review(_)
            | Document::Pull(_)
            | Document::CombinedStatus(_)
            | Document::Branch(_)
            | Document::Protection(_)
            | Document::Permission(_)
            | Document::Compare(_)
            | Document::Tree { .. }
            | Document::Runs { .. }
            | Document::Run(_)
            | Document::Job(_)
            | Document::Error(_) => Heard::Unanswered,
        }
    }
}
