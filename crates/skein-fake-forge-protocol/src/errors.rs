//! Forgejo's status and refusal vocabulary for the fake's failures.
use crate::Route;
use skein_fake_forge_domain::api::{Error, What};
use skein_forge::forgejo::failure::{self, Refusal};
use skein_lib::Time;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Failure {
    pub status: u16,
    pub message: &'static [u8],
    pub retry_after: Option<u64>,
}
#[must_use]
pub fn refusal(why: Refusal) -> Failure {
    let (status, message) = failure::answer(why);
    Failure { status, message, retry_after: None }
}
#[must_use]
pub fn answer(error: Error, route: Route, now: Time) -> Failure {
    let mut failure = Failure { status: 422, message: b"", retry_after: None };
    failure.status = match error {
        Error::Missing(What::Branch) if route == Route::DeleteBranch => 500,
        Error::Missing(_) => 404,
        Error::Refused if route.is_git() => 403,
        Error::Refused if route == Route::Rerun => 400,
        Error::Full
            if route == Route::Git(crate::git::Name::Commit)
                || route == Route::Git(crate::git::Name::Merge)
                || route == Route::Git(crate::git::Name::CommitTree) =>
        {
            507
        }
        Error::Unreachable if route.is_git() => 502,
        Error::RateLimited { reset } => {
            let nanos = reset.saturating_since(now).as_nanos();
            failure.retry_after = Some(nanos.div_ceil(1_000_000_000));
            429
        }
        Error::Exists | Error::AlreadyQueued(_) => 409,
        Error::Stale => return refusal(Refusal::HeadOutOfDate),
        Error::Conflict => {
            return refusal(if route == Route::Merge { Refusal::MergeConflict } else { Refusal::UpdateConflict });
        }
        Error::Closed => return refusal(Refusal::MergeClosed),
        Error::Protected if route == Route::Merge => return refusal(Refusal::MergeProtected),
        Error::Forbidden | Error::Protected => 403,
        Error::NothingToMerge | Error::Refused | Error::Circular => 422,
        Error::Empty => return refusal(Refusal::Empty),
        Error::Full | Error::TooLarge => 413,
        Error::Unavailable | Error::Unreachable => 503,
        Error::Timeout => 500,
    };
    failure
}
