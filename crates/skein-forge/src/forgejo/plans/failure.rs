//! Call-specific HTTP refusals and table invalidation (forge.md, 3.4).
use crate::api;
use crate::forgejo::failure::Refusal;
use crate::forgejo::plans::{Failure, Fields};
use skein_lib::Duration;
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Asked {
    Read,
    Leaf,
    Job,
    Write,
    OpenPull,
    BeforeWrite,
    Merge,
    Update,
    DeleteBranch,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Drop {
    Nothing,
    Path,
    Row,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Classified {
    pub error: api::Error,
    pub drop: Drop,
}
#[must_use]
pub fn classify(asked: Asked, status: u16, refusal: Option<Refusal>, retry: Option<Duration>) -> Classified {
    let read = asked == Asked::Read || asked == Asked::Leaf || asked == Asked::Job || asked == Asked::BeforeWrite;
    let mut drop = Drop::Nothing;
    let error = match status {
        300..=399 => {
            drop = Drop::Path;
            api::Error::Unavailable
        }
        403 if asked == Asked::DeleteBranch => api::Error::Protected,
        401 | 403 | 423 => api::Error::Forbidden,
        404 => {
            if asked != Asked::Leaf {
                drop = Drop::Path;
            }
            if asked == Asked::Job { api::Error::MissingJob } else { api::Error::Missing }
        }
        413 => api::Error::Full,
        429 => api::Error::RateLimited { after: retry.unwrap_or(Duration::ZERO) },
        409 if asked == Asked::OpenPull => api::Error::Exists,
        405 if asked == Asked::Merge => match refusal {
            Some(Refusal::MergeClosed) => api::Error::Closed,
            Some(Refusal::MergeConflict) => api::Error::Conflict,
            Some(Refusal::MergeProtected) => api::Error::Protected,
            Some(Refusal::MergeStyle) => {
                drop = Drop::Row;
                api::Error::Refused
            }
            Some(Refusal::HeadOutOfDate | Refusal::UpdateConflict | Refusal::RebaseConflict | Refusal::Empty)
            | None => api::Error::Refused,
        },
        409 if asked == Asked::Merge && refusal == Some(Refusal::HeadOutOfDate) => api::Error::Stale,
        409 if asked == Asked::Update
            && (refusal == Some(Refusal::UpdateConflict) || refusal == Some(Refusal::RebaseConflict)) =>
        {
            api::Error::Conflict
        }
        422 if !read && refusal == Some(Refusal::Empty) => api::Error::Empty,
        500..=599 => api::Error::Timeout,
        400..=499 => {
            if read {
                api::Error::InvalidAnswer
            } else {
                api::Error::Refused
            }
        }
        _ => {
            if read {
                api::Error::InvalidAnswer
            } else {
                api::Error::Timeout
            }
        }
    };
    Classified { error: before_write(asked, error), drop }
}
#[must_use]
pub fn before_write(asked: Asked, error: api::Error) -> api::Error {
    if asked == Asked::BeforeWrite && (error == api::Error::Timeout || error == api::Error::InvalidAnswer) {
        api::Error::Unavailable
    } else {
        error
    }
}
#[must_use]
pub fn read(status: u16, fields: &Fields) -> api::Error {
    classify(Asked::Read, status, None, fields.retry_after).error
}
#[must_use]
pub fn transport(failure: Failure) -> api::Error {
    match failure {
        Failure::Unseen => api::Error::Unavailable,
        Failure::Seen => api::Error::Timeout,
    }
}
