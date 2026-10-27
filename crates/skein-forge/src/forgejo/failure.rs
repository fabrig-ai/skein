//! The pinned Forgejo's refusal messages, as the committed transcripts spell them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Refusal {
    MergeClosed,
    MergeConflict,
    MergeProtected,
    MergeStyle,
    HeadOutOfDate,
    UpdateConflict,
    RebaseConflict,
    Empty,
}
/// Names a refusal only when both the status and the entire message match.
#[must_use]
pub fn refusal(status: u16, message: &[u8]) -> Option<Refusal> {
    for refusal in [
        Refusal::MergeClosed,
        Refusal::MergeConflict,
        Refusal::MergeProtected,
        Refusal::MergeStyle,
        Refusal::HeadOutOfDate,
        Refusal::UpdateConflict,
        Refusal::RebaseConflict,
        Refusal::Empty,
    ] {
        let (known_status, known_message) = answer(refusal);
        if status == known_status && message == known_message {
            return Some(refusal);
        }
    }
    None
}
#[must_use]
pub fn answer(refusal: Refusal) -> (u16, &'static [u8]) {
    match refusal {
        Refusal::MergeClosed => (405, b""),
        Refusal::RebaseConflict => (409, b"rebase failed because of conflict"),
        Refusal::MergeConflict => (405, b"Please try again later"),
        Refusal::MergeProtected => (405, b"not allowed to merge [reason: Does not have enough approvals]"),
        Refusal::MergeStyle => (405, b"squash is not an allowed merge style for this repository"),
        Refusal::HeadOutOfDate => (409, b"head out of date"),
        Refusal::UpdateConflict => (409, b"merge failed because of conflict"),
        Refusal::Empty => (422, b"[Title]: Required"),
    }
}

/// The pinned fast-forward refusal includes Git's diagnostic (forge.md, 9).
pub const DIVERGING_FAST_FORWARD: &[u8] = b"Merge DivergingFastForwardOnly Error: exit status 128: hint: Diverging branches can't be fast-forwarded, you need to either:\nhint:\nhint: \tgit merge --no-ff\nhint:\nhint: or:\nhint:\nhint: \tgit rebase\nhint:\nhint: Disable this message with \"git config advice.diverging false\"\nfatal: Not possible to fast-forward, aborting.\n\n";
