//! The fake's copies of GitHub's refusal texts (forge-github.md, section 7).

// user-on-installation-403.
pub(crate) const INTEGRATION: &[u8] = b"Resource not accessible by integration";
pub(crate) const PERSON: &[u8] = b"Resource not accessible by personal access token";
// merge-stale-409.
pub(crate) const MERGE_STALE: &[u8] = b"Head branch was modified. Review and try the merge again.";
// merge-after-move-not-merged.
pub(crate) const NOT_MERGEABLE: &[u8] = b"Pull Request is not mergeable";
// ruleset-approval-405.
pub(crate) const RULES: &[u8] = b"Repository rule violations found";
// queue-direct-merge-405; live session 1917 records the queue line before
// the approval line. Context lines after it are inferred: neither shape was
// recorded with checks; live session 1917 lists every violated rule.
pub(crate) const QUEUE: &[u8] = b"Changes must be made through the merge queue";
// repository-allows-shown; from `Squash merges are not allowed on this repository.`
// in github/tests/writes/merge.rs; the fake supplies the requested method.
pub(crate) const MERGE_METHOD: &[u8] = b"merges are not allowed on this repository.";
// enqueue-stale-unprocessable; github/tests/failures.rs's complete fixture.
pub(crate) const ENQUEUE_STALE: &[u8] = b"enqueuePullRequest: expected head oid does not match the current head oid";
// enqueue-pending-check-protected; followed by the check name and CHECK_PROGRESS.
pub(crate) const ENQUEUE_CHECK: &[u8] = b"Pull request Required status check \"";
// update-stale-422; GitHub writes U+2019.
pub(crate) const UPDATE_STALE: &[u8] = "expected head sha didn’t match current head ref.".as_bytes();
// update-conflict-422.
pub(crate) const UPDATE_CONFLICT: &[u8] = b"merge conflict between base and head";
// pulls-422-no-commits; from `No commits between main and feature` in
// github/tests/failures.rs; the fake supplies the requested base and head.
pub(crate) const NO_COMMITS: &[u8] = b"No commits between ";
// pulls-422-exists; from `A pull request already exists for alice` in
// github/tests/writes.rs; the fake supplies the requested head.
pub(crate) const PULL_EXISTS: &[u8] = b"A pull request already exists for ";

// approve-own-422.
pub(crate) const OWN_REVIEW_ERROR: &[u8] = skein_forge::github::failure::OWN_REVIEW_ERROR;

// refs-422-exists.
pub(crate) const REF_EXISTS: &[u8] = b"Reference already exists";
// refs-delete-missing-422.
pub(crate) const REF_MISSING: &[u8] = b"Reference does not exist";

// no unmet rule visible; unrecorded shape (fake-forge.md, 10.4).
pub(crate) const RULES_FALLBACK: &[u8] = b"Repository rule violations found\n\n";

// o/github live 2026-10-20 records 1; live session 1917 records 2.
// Counts of 3 and up are inferred.
pub(crate) const APPROVAL_START: &[u8] = b"At least ";
// o/github live 2026-10-20 records the singular for 1.
pub(crate) const APPROVAL_SINGULAR: &[u8] = b" approving review is required by reviewers with write access.";
// live session 1917 records the plural for 2; counts of 3 and up are inferred.
pub(crate) const APPROVAL_PLURAL: &[u8] = b" approving reviews are required by reviewers with write access.";
// o/github live 2026-10-20.
pub(crate) const CHECK_START: &[u8] = b"Required status check \"";
// o/github live 2026-10-20.
pub(crate) const CHECK_EXPECTED: &[u8] = b"\" is expected.";
// o/github live 2026-10-20.
pub(crate) const CHECK_PROGRESS: &[u8] = b"\" is in progress.";
// o/github live 2026-10-20, for an Actions check; a commit status's is inferred.
pub(crate) const CHECK_FAILING: &[u8] = b"\" is failing.";

// unrecorded: GitHub's spelling as forge-github.md names it.
pub(crate) const CI_FAILURE: &[u8] = b"CI_FAILURE";
// unrecorded: GitHub's spelling as forge-github.md names it.
pub(crate) const MERGE_CONFLICT: &[u8] = b"MERGE_CONFLICT";
// classic-protection-404-under-rulesets.
pub(crate) const BRANCH_NOT_PROTECTED: &[u8] = b"Branch not protected";
// Unrecorded: HTTP admission refusals before GitHub route decoding.
pub(crate) const PAYLOAD_TOO_LARGE: &[u8] = b"Payload Too Large";
// live session 1917, followed by the approval sentence without blank lines.
pub(crate) const ENQUEUE_APPROVAL_PREFIX: &[u8] = b"Pull request ";

pub(crate) fn approval(out: &mut skein_lib::Writer, count: u32) -> Option<()> {
    out.put(APPROVAL_START).ok()?;
    out.put(skein_lib::Decimal::of(u64::from(count)).as_bytes()).ok()?;
    out.put(if count == 1 { APPROVAL_SINGULAR } else { APPROVAL_PLURAL }).ok()
}

// Unrecorded primary-rate message; inferred from forge-github.md, 7 and 8.5.
pub(crate) const PRIMARY_RATE: &[u8] = b"API rate limit exceeded";
// forge-github.md, 7, recorded secondary-limit classifier.
pub(crate) const SECONDARY_RATE: &[u8] = b"You have exceeded a secondary rate limit";

// Docs-derived installation-repository-removal-refusals; unchecked live.
// DELETE is refused for an all-repositories installation.
pub(crate) const REMOVE_ALL_REPOSITORIES: &[u8] = b"Validation Failed";
// installation-repository-removal-refusals, checked live.
// DELETE is refused when it would remove the installation's last repository.
pub(crate) const REMOVE_LAST_REPOSITORY: &[u8] = b"Cannot remove the last repository from this installation.";

// Inferred from GitHub docs, unrecorded on an installation token.
pub(crate) const RULESET_INTEGRATION: &[u8] = b"Resource not accessible by integration";
