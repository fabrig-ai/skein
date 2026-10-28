//! JSON lines for the promises judge (fake-forge.md, section 2.4).
use skein_fake_forge_domain::promises::Recorded;
use skein_fake_forge_domain::{Observation, Operation, api as a};
use skein_json::writer::{Encoder, Limits as WriteLimits};
use skein_lib::{List, Writer as Buffer, base64};
mod read;
mod writer;
pub use read::{Reason, Refusal, decode, decode_with_limits};
pub use writer::Writer;

/// Appends one measured object and newline; a non-UTF-8 name writes nothing.
/// The buffer must have room for the complete line; insufficient room writes nothing.
pub fn encode(record: &Recorded, out: &mut List<u8>) -> Option<()> {
    let limits = WriteLimits { depth: 3, length: u32::MAX };
    let mut measure = Encoder::measure(&limits);
    render(&mut measure, record)?;
    let len = measure.measured().ok()?;
    let size = len.checked_add(1)?;
    if out.room() < size {
        return None;
    }
    let mut write = Encoder::write(len, &limits);
    render(&mut write, record)?;
    for byte in write.finish() {
        out.push(byte).expect("complete line admitted");
    }
    out.push(b'\n').expect("newline admitted");
    Some(())
}

/// Measures a line, including its newline, without allocating the output.
#[must_use]
pub fn encoded_len(record: &Recorded) -> Option<u32> {
    let mut measure = Encoder::measure(&WriteLimits { depth: 3, length: u32::MAX });
    render(&mut measure, record)?;
    measure.measured().ok()?.checked_add(1)
}

#[expect(clippy::too_many_lines, reason = "one exhaustive match writes every observation in domain field order")]
fn render(json: &mut Encoder, record: &Recorded) -> Option<()> {
    json.object_start();
    match record {
        Recorded::Lost(n) => {
            json.key(b"observations_lost");
            json.unsigned(*n);
        }
        Recorded::Observed { observation, forward } => {
            json.key(b"observation");
            match observation {
                Observation::Moved { repository, branch, from, to, by } => {
                    json.string(b"moved");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"branch");
                    json.string(branch);
                    json.key(b"from");
                    optional_number(json, *from);
                    json.key(b"to");
                    json.unsigned(*to);
                    json.key(b"by");
                    json.unsigned(*by);
                    json.key(b"forward");
                    match forward {
                        Some(value) => json.boolean(*value),
                        None => json.null(),
                    }
                }
                Observation::Deleted { repository, branch, at, by } => {
                    json.string(b"deleted");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"branch");
                    json.string(branch);
                    json.key(b"at");
                    json.unsigned(*at);
                    json.key(b"by");
                    json.unsigned(*by);
                }
                Observation::Opened { repository, number, kind, title, body, labels, branches, by } => {
                    json.string(b"opened");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"number");
                    json.unsigned(*number);
                    json.key(b"kind");
                    json.string(write_kind(*kind));
                    json.key(b"title");
                    write_content(json, title)?;
                    json.key(b"body");
                    write_content(json, body)?;
                    json.key(b"labels");
                    json.array_start();
                    for name in labels {
                        json.string(name);
                    }
                    json.array_end();
                    json.key(b"branches");
                    match branches {
                        Some(branches) => {
                            json.object_start();
                            json.key(b"head");
                            json.string(&branches.head);
                            json.key(b"base");
                            json.string(&branches.base);
                            json.key(b"commit");
                            json.unsigned(branches.commit);
                            json.object_end();
                        }
                        None => json.null(),
                    }
                    json.key(b"by");
                    json.unsigned(*by);
                }
                Observation::Closed { repository, number, by } => {
                    json.string(b"closed");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"number");
                    json.unsigned(*number);
                    json.key(b"by");
                    json.unsigned(*by);
                }
                Observation::Reopened { repository, number, by } => {
                    json.string(b"reopened");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"number");
                    json.unsigned(*number);
                    json.key(b"by");
                    json.unsigned(*by);
                }
                Observation::Labelled { repository, number, labels, by } => {
                    json.string(b"labelled");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"number");
                    json.unsigned(*number);
                    json.key(b"labels");
                    json.array_start();
                    for name in labels {
                        json.string(name);
                    }
                    json.array_end();
                    json.key(b"by");
                    json.unsigned(*by);
                }
                Observation::Revised { repository, number, title, body, by } => {
                    json.string(b"revised");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"number");
                    json.unsigned(*number);
                    json.key(b"title");
                    write_content(json, title)?;
                    json.key(b"body");
                    write_content(json, body)?;
                    json.key(b"by");
                    json.unsigned(*by);
                }
                Observation::Depends { repository, number, dependencies, by } => {
                    json.string(b"depends");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"number");
                    json.unsigned(*number);
                    json.key(b"dependencies");
                    json.array_start();
                    for value in dependencies {
                        json.unsigned(*value);
                    }
                    json.array_end();
                    json.key(b"by");
                    json.unsigned(*by);
                }
                Observation::Requested { repository, number, reviewers, by } => {
                    json.string(b"requested");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"number");
                    json.unsigned(*number);
                    json.key(b"reviewers");
                    json.array_start();
                    for value in reviewers {
                        json.unsigned(*value);
                    }
                    json.array_end();
                    json.key(b"by");
                    json.unsigned(*by);
                }
                Observation::Defined { repository, label, by } => {
                    json.string(b"defined");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"label");
                    json.string(label);
                    json.key(b"by");
                    json.unsigned(*by);
                }
                Observation::Commented { repository, number, id, body, by } => {
                    json.string(b"commented");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"number");
                    json.unsigned(*number);
                    json.key(b"id");
                    json.unsigned(*id);
                    json.key(b"body");
                    write_content(json, body)?;
                    json.key(b"by");
                    json.unsigned(*by);
                }
                Observation::Edited { repository, number, id, body, by } => {
                    json.string(b"edited");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"number");
                    json.unsigned(*number);
                    json.key(b"id");
                    json.unsigned(*id);
                    json.key(b"body");
                    write_content(json, body)?;
                    json.key(b"by");
                    json.unsigned(*by);
                }
                Observation::Removed { repository, number, id, by } => {
                    json.string(b"removed");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"number");
                    json.unsigned(*number);
                    json.key(b"id");
                    json.unsigned(*id);
                    json.key(b"by");
                    json.unsigned(*by);
                }
                Observation::Reviewed { repository, number, id, commit, verdict, body, by } => {
                    json.string(b"reviewed");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"number");
                    json.unsigned(*number);
                    json.key(b"id");
                    json.unsigned(*id);
                    json.key(b"commit");
                    json.unsigned(*commit);
                    json.key(b"verdict");
                    json.string(write_verdict(*verdict));
                    json.key(b"body");
                    write_content(json, body)?;
                    json.key(b"by");
                    json.unsigned(*by);
                }
                Observation::Reported { repository, commit, context, state, by } => {
                    json.string(b"reported");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"commit");
                    json.unsigned(*commit);
                    json.key(b"context");
                    json.string(context);
                    json.key(b"state");
                    json.string(check(*state));
                    json.key(b"by");
                    json.unsigned(*by);
                }
                Observation::Merged { repository, number, base, head, commit, by } => {
                    json.string(b"merged");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"number");
                    json.unsigned(*number);
                    json.key(b"base");
                    json.string(base);
                    json.key(b"head");
                    json.unsigned(*head);
                    json.key(b"commit");
                    json.unsigned(*commit);
                    json.key(b"by");
                    json.unsigned(*by);
                }
                Observation::Enqueued { repository, number, head, position, by } => {
                    json.string(b"enqueued");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"number");
                    json.unsigned(*number);
                    json.key(b"head");
                    json.unsigned(*head);
                    json.key(b"position");
                    json.unsigned(u64::from(*position));
                    json.key(b"by");
                    json.unsigned(*by);
                }
                Observation::Group { repository, number, commit, ended, landed } => {
                    json.string(b"group");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"number");
                    json.unsigned(*number);
                    json.key(b"commit");
                    json.unsigned(*commit);
                    json.key(b"ended");
                    json.boolean(*ended);
                    json.key(b"landed");
                    json.boolean(*landed);
                }
                Observation::Dequeued { repository, number, reason } => {
                    json.string(b"dequeued");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"number");
                    json.unsigned(*number);
                    json.key(b"reason");
                    json.string(queue_removal(*reason));
                }
                Observation::Refused { repository, what, number, commit, error, by } => {
                    json.string(b"refused");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"what");
                    json.string(operation(*what));
                    json.key(b"number");
                    optional_number(json, *number);
                    json.key(b"commit");
                    optional_number(json, *commit);
                    json.key(b"error");
                    write_error(json, error);
                    json.key(b"by");
                    json.unsigned(*by);
                }
                Observation::Rejected { repository, branch, commit, by } => {
                    json.string(b"rejected");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"branch");
                    json.string(branch);
                    json.key(b"commit");
                    json.unsigned(*commit);
                    json.key(b"by");
                    json.unsigned(*by);
                }
                Observation::Wiki { repository, name, content, revision, by } => {
                    json.string(b"wiki");
                    json.key(b"repository");
                    json.string(repository);
                    json.key(b"name");
                    json.string(name);
                    json.key(b"content");
                    match content {
                        Some(bytes) => write_content(json, bytes)?,
                        None => json.null(),
                    }
                    json.key(b"revision");
                    json.unsigned(*revision);
                    json.key(b"by");
                    json.unsigned(*by);
                }
            }
        }
    }
    json.object_end();
    Some(())
}
fn optional_number(json: &mut Encoder, n: Option<u64>) {
    match n {
        Some(value) => json.unsigned(value),
        None => json.null(),
    }
}
fn write_content(json: &mut Encoder, bytes: &[u8]) -> Option<()> {
    let size = base64::encoded_len(base64::Alphabet::Standard, u32::try_from(bytes.len()).ok()?)?;
    let mut out = Buffer::new(usize::try_from(size).ok()?);
    base64::encode(base64::Alphabet::Standard, bytes, &mut out).ok()?;
    json.string(&out.finish());
    Some(())
}
fn write_error(json: &mut Encoder, error: &a::Error) {
    json.object_start();
    json.key(b"name");
    match error {
        a::Error::Unavailable => {
            json.string(b"unavailable");
        }
        a::Error::Timeout => {
            json.string(b"timeout");
        }
        a::Error::RateLimited { reset } => {
            json.string(b"rate_limited");
            json.key(b"reset");
            json.unsigned(reset.as_nanos());
        }
        a::Error::Forbidden => {
            json.string(b"forbidden");
        }
        a::Error::Missing(what) => {
            json.string(b"missing");
            json.key(b"what");
            json.string(what_name(*what));
        }
        a::Error::TooLarge => {
            json.string(b"too_large");
        }
        a::Error::Full => {
            json.string(b"full");
        }
        a::Error::Exists => {
            json.string(b"exists");
        }
        a::Error::AlreadyQueued(entry) => {
            json.string(b"already_queued");
            json.key(b"number");
            json.unsigned(entry.number);
            json.key(b"head");
            json.unsigned(entry.head);
            json.key(b"position");
            json.unsigned(u64::from(entry.position));
        }
        a::Error::Circular => {
            json.string(b"circular");
        }
        a::Error::NothingToMerge => {
            json.string(b"nothing_to_merge");
        }
        a::Error::Empty => {
            json.string(b"empty");
        }
        a::Error::Closed => {
            json.string(b"closed");
        }
        a::Error::Stale => {
            json.string(b"stale");
        }
        a::Error::Conflict => {
            json.string(b"conflict");
        }
        a::Error::Protected => {
            json.string(b"protected");
        }
        a::Error::Unreachable => {
            json.string(b"unreachable");
        }
        a::Error::Refused => {
            json.string(b"refused");
        }
    }
    json.object_end();
}
fn operation(value: Operation) -> &'static [u8] {
    match value {
        Operation::CreateIssue => b"create_issue",
        Operation::SetProtection => b"set_protection",
        Operation::EditItem => b"edit_item",
        Operation::SetDependencies => b"set_dependencies",
        Operation::Comment => b"comment",
        Operation::EditComment => b"edit_comment",
        Operation::DeleteComment => b"delete_comment",
        Operation::SetLabels => b"set_labels",
        Operation::AddLabels => b"add_labels",
        Operation::RemoveLabels => b"remove_labels",
        Operation::DefineLabel => b"define_label",
        Operation::OpenPull => b"open_pull",
        Operation::SetReviewers => b"set_reviewers",
        Operation::Review => b"review",
        Operation::Submit => b"submit",
        Operation::Merge => b"merge",
        Operation::Enqueue => b"enqueue",
        Operation::Update => b"update",
        Operation::Close => b"close",
        Operation::Reopen => b"reopen",
        Operation::DeleteBranch => b"delete_branch",
        Operation::Status => b"status",
        Operation::PutPage => b"put_page",
        Operation::DeletePage => b"delete_page",
        Operation::Clone => b"clone",
        Operation::Fetch => b"fetch",
        Operation::Push => b"push",
        Operation::Create => b"create",
    }
}
fn write_kind(value: a::Kind) -> &'static [u8] {
    match value {
        a::Kind::Issue => b"issue",
        a::Kind::Pull => b"pull",
    }
}
fn write_verdict(value: a::Verdict) -> &'static [u8] {
    match value {
        a::Verdict::Approve => b"approve",
        a::Verdict::RequestChanges => b"request_changes",
        a::Verdict::Comment => b"comment",
    }
}
fn check(value: a::Check) -> &'static [u8] {
    match value {
        a::Check::Pending => b"pending",
        a::Check::Skipped => b"skipped",
        a::Check::Passed => b"passed",
        a::Check::Failed => b"failed",
    }
}
fn queue_removal(value: a::QueueRemoval) -> &'static [u8] {
    match value {
        a::QueueRemoval::MergeConflict => b"merge_conflict",
        a::QueueRemoval::CiFailure => b"ci_failure",
    }
}
fn what_name(value: a::What) -> &'static [u8] {
    match value {
        a::What::Repository => b"repository",
        a::What::Item => b"item",
        a::What::Pull => b"pull",
        a::What::Comment => b"comment",
        a::What::Label => b"label",
        a::What::Branch => b"branch",
        a::What::Commit => b"commit",
        a::What::File => b"file",
        a::What::Page => b"page",
        a::What::Review => b"review",
        a::What::Job => b"job",
    }
}
