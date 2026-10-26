//! GitHub queue admission requires checks on the pull's head before group CI.
use crate::{
    answers::{self, Body},
    reads,
    service::{Outcome, PendingKind, Service},
    texts,
    writes::merge,
};
use alloc::boxed::Box;
use skein_fake_forge_domain::{self as fake, api as a};
use skein_lib::{ReplyTo, Writer};

#[derive(Debug)]
pub(crate) struct Pending {
    repository: Box<[u8]>,
    number: u64,
    head: u64,
    user: u64,
    stage: Stage,
}
#[derive(Debug)]
enum Stage {
    Head,
    Author { pull: Box<a::Pull>, context: Box<[u8]> },
}
fn call(pending: Pending, op: a::Op, reply_to: ReplyTo) -> Outcome {
    let event = fake::Event::Call { reply_to, user: pending.user, repository: pending.repository.clone(), op };
    Outcome::Call(crate::service::Pending { rate: None, kind: PendingKind::Enqueue(pending) }, event)
}
pub(crate) fn start(repository: Box<[u8]>, number: u64, head: u64, user: u64, reply_to: ReplyTo) -> Outcome {
    call(
        Pending { repository, number, head, user, stage: Stage::Head },
        a::Op::Read(a::Read::Pull { number }),
        reply_to,
    )
}
fn admit(pending: Pending, reply_to: ReplyTo) -> Outcome {
    let event = fake::Event::Call {
        reply_to,
        user: pending.user,
        repository: pending.repository.clone(),
        op: a::Op::Write(a::Write::Enqueue { number: pending.number, head: pending.head }),
    };
    Outcome::Call(
        crate::service::Pending {
            rate: None,
            kind: PendingKind::Read(reads::Pending::Enqueue { repository: pending.repository }),
        },
        event,
    )
}
fn refused(service: &Service, context: &[u8]) -> Outcome {
    let Ok(capacity) = usize::try_from(service.limits.response) else {
        return Outcome::Reply(answers::failed());
    };
    let mut out = Writer::new(capacity);
    for part in [texts::ENQUEUE_CHECK, context, texts::CHECK_PROGRESS] {
        if out.put(part).is_err() {
            return Outcome::Reply(answers::failed());
        }
    }
    let Some(message) = answers::finish(out) else {
        return Outcome::Reply(answers::failed());
    };
    Outcome::Reply(answers::response(200, &Body::EnqueueRefusal(&message), &service.limits))
}
pub(crate) fn finish(
    service: &Service,
    pending: Pending,
    result: Result<a::Answer, a::Error>,
    reply_to: ReplyTo,
    domain: &fake::Domain,
) -> Outcome {
    let answer = match result {
        Ok(answer) => answer,
        Err(error) => {
            return Outcome::Reply(reads::enqueue_finish(Err(error), &pending.repository, &service.limits, domain));
        }
    };
    let Some(protection) = domain.inspect_repository_protection(&pending.repository) else {
        return admit(pending, reply_to);
    };
    match &pending.stage {
        Stage::Head => {
            let a::Answer::Pull(pull) = answer else {
                return Outcome::Reply(answers::failed());
            };
            // The domain retains authorization, closed, stale, duplicate and approval refusals.
            if domain.permission(&pending.repository, pending.user).ok() < Some(a::Permission::Write)
                || pull.state == a::State::Closed
                || pull.commit != pending.head
                || pull.queued.is_some()
                || !protection.queue
                || protection.branch != pull.base
                || protection.bypassing_users.contains(&pending.user)
            {
                return admit(pending, reply_to);
            }
            for context in &protection.contexts {
                if merge::check(&pull, context) != Some(a::Check::Passed) {
                    if protection.approvals == 0 {
                        return refused(service, context);
                    }
                    let number = pending.number;
                    return call(
                        Pending { stage: Stage::Author { pull: Box::new(pull), context: context.clone() }, ..pending },
                        a::Op::Read(a::Read::Item { number, after: 0 }),
                        reply_to,
                    );
                }
            }
            admit(pending, reply_to)
        }
        Stage::Author { pull, context } => {
            let a::Answer::Item { item, .. } = answer else {
                return Outcome::Reply(answers::failed());
            };
            if merge::approvals(pull, item.author, protection) < protection.approvals {
                admit(pending, reply_to)
            } else {
                refused(service, context)
            }
        }
    }
}
