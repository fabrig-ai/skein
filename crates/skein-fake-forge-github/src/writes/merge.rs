//! REST merges enforce repository methods and describe the domain's protection.
use super::{Pending, Stage as WriteStage, dispatch};
use crate::{
    answers::{self, Body},
    reads,
    service::{Limits, Outcome, Response, Service},
    texts,
};
use alloc::boxed::Box;
use skein_fake_forge_domain::{self as fake, api as a};
use skein_forge::{api, github::plans::tables::Method};
use skein_lib::{ReplyTo, Writer};

#[derive(Debug)]
pub(super) enum Stage {
    Settings { number: u64, head: api::Commit, method: Method },
    Head { number: u64, head: api::Commit },
    Merge { number: u64 },
    Pull { number: u64, queue: bool },
    Author { pull: Box<a::Pull>, queue: bool },
}
fn call(user: u64, repository: Box<[u8]>, stage: Stage, op: a::Op, reply_to: ReplyTo) -> Outcome {
    dispatch(Pending { user, repository, stage: WriteStage::Merge(stage) }, op, reply_to)
}
pub(super) fn finish(
    service: &Service,
    user: u64,
    repository: Box<[u8]>,
    stage: Stage,
    result: Result<a::Answer, a::Error>,
    reply_to: ReplyTo,
    domain: &fake::Domain,
) -> Outcome {
    let limits = &service.limits;
    let answer = match result {
        Ok(answer) => answer,
        Err(error) => return refused(service, user, repository, stage, error, reply_to),
    };
    match stage {
        Stage::Settings { number, head, method } => {
            let a::Answer::Settings(settings) = answer else {
                return Outcome::Reply(answers::failed());
            };
            let allowed = match method {
                Method::Merge => settings.merge,
                Method::Squash => settings.squash,
                Method::Rebase => settings.rebase,
            };
            if !allowed {
                let prefix = match method {
                    Method::Merge => b"Merge ".as_slice(),
                    Method::Squash => b"Squash ",
                    Method::Rebase => b"Rebase ",
                };
                let Some(message) = crate::service::joined(prefix, texts::MERGE_METHOD) else {
                    return Outcome::Reply(answers::failed());
                };
                return Outcome::Reply(answers::refusal(405, &message, limits));
            }
            call(user, repository, Stage::Head { number, head }, a::Op::Read(a::Read::Pull { number }), reply_to)
        }
        Stage::Head { number, head } => {
            let a::Answer::Pull(pull) = answer else {
                return Outcome::Reply(answers::failed());
            };
            if let Some(commit) = pull.merged {
                return Outcome::Reply(answers::response(200, &Body::Merged(&reads::commit(commit)), limits));
            }
            if reads::commit(pull.commit) != head {
                return Outcome::Reply(answers::refusal(409, texts::MERGE_STALE, limits));
            }
            // forge-github.md, 7: a move is Conflict until mergeability settles.
            if pull.checking {
                return Outcome::Reply(answers::refusal(405, texts::NOT_MERGEABLE, limits));
            }
            if pull.queue_required {
                return queue(user, repository, number, reply_to);
            }
            // fake-forge.md, 10.4: the domain squashes every allowed method.
            call(
                user,
                repository,
                Stage::Merge { number },
                a::Op::Write(a::Write::Merge { number, head: pull.commit }),
                reply_to,
            )
        }
        Stage::Merge { .. } => {
            let a::Answer::Merged(commit) = answer else {
                return Outcome::Reply(answers::failed());
            };
            Outcome::Reply(answers::response(200, &Body::Merged(&reads::commit(commit)), limits))
        }
        Stage::Pull { number, queue } => {
            let a::Answer::Pull(pull) = answer else {
                return Outcome::Reply(fallback(limits));
            };
            call(
                user,
                repository,
                Stage::Author { pull: Box::new(pull), queue },
                a::Op::Read(a::Read::Item { number, after: 0 }),
                reply_to,
            )
        }
        Stage::Author { pull, queue } => {
            let a::Answer::Item { item, .. } = answer else {
                return Outcome::Reply(fallback(limits));
            };
            Outcome::Reply(rules(domain.inspect_protection(&repository, &pull.base), &pull, item.author, queue, limits))
        }
    }
}
fn refused(
    service: &Service,
    user: u64,
    repository: Box<[u8]>,
    stage: Stage,
    error: a::Error,
    reply_to: ReplyTo,
) -> Outcome {
    let limits = &service.limits;
    match stage {
        Stage::Merge { number } if error == a::Error::Protected => {
            return call(
                user,
                repository,
                Stage::Pull { number, queue: false },
                a::Op::Read(a::Read::Pull { number }),
                reply_to,
            );
        }
        Stage::Pull { .. } | Stage::Author { .. } => return Outcome::Reply(fallback(limits)),
        Stage::Merge { .. } | Stage::Settings { .. } | Stage::Head { .. } => {}
    }
    let response = match error {
        a::Error::Stale => answers::refusal(409, texts::MERGE_STALE, limits),
        a::Error::Conflict | a::Error::Closed | a::Error::NothingToMerge => {
            answers::refusal(405, texts::NOT_MERGEABLE, limits)
        }
        a::Error::Missing(_) | a::Error::Forbidden => answers::missing(limits),
        a::Error::Unavailable => answers::refusal(503, b"Server Error", limits),
        a::Error::Timeout => answers::refusal(504, b"Server Error", limits),
        a::Error::RateLimited { .. } | a::Error::TooLarge | a::Error::Full | a::Error::AlreadyQueued(_) => {
            answers::failed()
        }
        a::Error::Exists
        | a::Error::Circular
        | a::Error::Empty
        | a::Error::Protected
        | a::Error::Unreachable
        | a::Error::Refused => answers::refusal(422, b"Validation Failed", limits),
    };
    Outcome::Reply(response)
}
pub(crate) fn approvals(pull: &a::Pull, author: u64, protection: &a::Protection) -> u32 {
    let mut count = 0_u32;
    for (index, review) in pull.reviews.iter().enumerate() {
        if review.author == author
            || !review.official
            || review.verdict != a::Verdict::Approve
            || (protection.dismiss_stale && review.commit != pull.commit)
        {
            continue;
        }
        let mut last = true;
        for later in pull.reviews.get(index.saturating_add(1)..).unwrap_or(&[]) {
            if later.author == review.author && later.verdict != a::Verdict::Comment {
                last = false;
            }
        }
        if last {
            count = count.saturating_add(1);
        }
    }
    count
}
pub(crate) fn check(pull: &a::Pull, context: &[u8]) -> Option<a::Check> {
    for status in &pull.statuses {
        if status.context.as_ref() == context {
            return Some(status.state);
        }
    }
    None
}
fn context(out: &mut Writer, name: &[u8], state: Option<a::Check>) -> Option<()> {
    let suffix = match state {
        None => texts::CHECK_EXPECTED,
        Some(a::Check::Pending) => texts::CHECK_PROGRESS,
        Some(a::Check::Failed) => texts::CHECK_FAILING,
        Some(a::Check::Passed) => return Some(()),
    };
    out.put(texts::CHECK_START).ok()?;
    out.put(name).ok()?;
    out.put(suffix).ok()?;
    out.put(b"\n\n").ok()
}
fn message(protection: &a::Protection, pull: &a::Pull, author: u64, queue: bool, capacity: usize) -> Option<Box<[u8]>> {
    let mut out = Writer::new(capacity);
    out.put(texts::RULES).ok()?;
    out.put(b"\n\n").ok()?;
    if queue {
        out.put(texts::QUEUE).ok()?;
        out.put(b"\n\n").ok()?;
    }
    let missing_approvals = approvals(pull, author, protection) < protection.approvals;
    if missing_approvals {
        texts::approval(&mut out, protection.approvals)?;
        out.put(b"\n\n").ok()?;
    }
    let mut missing_context = false;
    for name in &protection.contexts {
        let state = check(pull, name);
        if state != Some(a::Check::Passed) {
            missing_context = true;
            context(&mut out, name, state)?;
        }
    }
    if !queue && !missing_approvals && !missing_context {
        return None;
    }
    answers::finish(out)
}
fn rules(protection: Option<&a::Protection>, pull: &a::Pull, author: u64, queue: bool, limits: &Limits) -> Response {
    let Some(protection) = protection else {
        return fallback(limits);
    };
    let Ok(capacity) = usize::try_from(limits.response) else {
        return answers::failed();
    };
    let Some(message) = message(protection, pull, author, queue, capacity) else {
        return fallback(limits);
    };
    answers::response(405, &Body::Rules(&message), limits)
}
fn fallback(limits: &Limits) -> Response {
    answers::response(405, &Body::Rules(texts::RULES_FALLBACK), limits)
}

fn queue(user: u64, repository: Box<[u8]>, number: u64, reply_to: ReplyTo) -> Outcome {
    call(user, repository, Stage::Pull { number, queue: true }, a::Op::Read(a::Read::Pull { number }), reply_to)
}
