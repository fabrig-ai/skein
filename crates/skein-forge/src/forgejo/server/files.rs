use super::{
    Answer, Route, Unrouted,
    items::{number_value, page, query_value},
};
use crate::forgejo::{
    document::{ChangedFile, FileStatus},
    id,
    limits::Limits,
    path,
    request::{Method, Repo, Request},
};
use alloc::boxed::Box;
use skein_json::writer::Encoder;
use skein_lib::Writer;
pub(super) fn route(method: Method, bytes: &[u8], limits: &Limits) -> Result<Route, Unrouted> {
    let Ok(target) = path::read(bytes, limits) else {
        return Err(Unrouted::Malformed);
    };
    let [api, v1, repos, owner, name, tail @ ..] = target.segments.as_ref() else {
        return Err(Unrouted::NotFound);
    };
    if &**api != b"api" || &**v1 != b"v1" || &**repos != b"repos" {
        return Err(Unrouted::NotFound);
    }
    let at = Repo { owner: owner.clone(), name: name.clone() };
    let request = match tail {
        [git, trees, object] if &**git == b"git" && &**trees == b"trees" => {
            let page_number = match query_value(&target.query, b"page") {
                Some(v) => Some(u32::try_from(number_value(&v)?).or(Err(Unrouted::Malformed))?),
                None => None,
            };
            let limit = match query_value(&target.query, b"per_page") {
                Some(v) => Some(u32::try_from(number_value(&v)?).or(Err(Unrouted::Malformed))?),
                None => None,
            };
            Request::Tree {
                at,
                object: id::read(object).ok_or(Unrouted::Malformed)?,
                page: crate::forgejo::request::Page { page: page_number, limit },
            }
        }
        [pulls, number, files] if &**pulls == b"pulls" && &**files == b"files" => {
            Request::PullFiles { at, number: number_value(number)?, page: page(&target.query)? }
        }
        [compare, commits] if &**compare == b"compare" => {
            let split = skein_lib::bytes::find(commits, b"...").ok_or(Unrouted::Malformed)?;
            Request::Compare {
                at,
                before: id::read(commits.get(..split).expect("split position")).ok_or(Unrouted::Malformed)?,
                after: id::read(commits.get(split.saturating_add(3)..).expect("split tail"))
                    .ok_or(Unrouted::Malformed)?,
                files: query_value(&target.query, b"files").as_deref() != Some(b"false".as_slice()),
            }
        }
        [raw, segments @ ..] if &**raw == b"raw" && !segments.is_empty() => {
            let reference = query_value(&target.query, b"ref").ok_or(Unrouted::Malformed)?;
            Request::Raw { at, path: joined(segments), reference: id::read(&reference).ok_or(Unrouted::Malformed)? }
        }
        _ => return Err(Unrouted::NotFound),
    };
    match method {
        Method::Get => Ok(Route::Ready(request)),
        Method::Post | Method::Put | Method::Patch | Method::Delete => Err(Unrouted::Method),
    }
}
fn joined(segments: &[Box<[u8]>]) -> Box<[u8]> {
    let mut length = segments.len().saturating_sub(1);
    for segment in segments {
        length = length.saturating_add(segment.len());
    }
    let mut out = Writer::new(length);
    for (i, segment) in segments.iter().enumerate() {
        if i != 0 {
            out.put(b"/").expect("measured slash");
        }
        out.put(segment).expect("measured path");
    }
    out.finish()
}
fn status(value: FileStatus) -> &'static [u8] {
    match value {
        FileStatus::Added => b"added",
        FileStatus::Deleted => b"deleted",
        FileStatus::Modified => b"modified",
        FileStatus::Renamed => b"renamed",
        FileStatus::Copied => b"copied",
    }
}
fn file(json: &mut Encoder, value: &ChangedFile) {
    json.object_start();
    json.key(b"filename");
    json.string(&value.filename);
    if let Some(previous) = &value.previous_filename {
        json.key(b"previous_filename");
        json.string(previous);
    }
    json.key(b"status");
    json.string(status(value.status));
    json.object_end();
}
#[expect(clippy::too_many_lines, reason = "the server encoder names every answer variant at its dispatch boundary")]
pub(super) fn write(json: &mut Encoder, answer: &Answer) {
    match answer {
        Answer::Commits { commits } => {
            json.object_start();
            json.key(b"total_commits");
            json.unsigned(u64::try_from(commits.len()).expect("bounded comparison"));
            json.key(b"commits");
            json.array_start();
            for commit in commits {
                json.object_start();
                json.key(b"sha");
                json.string(&id::write(&commit.id));
                json.key(b"parents");
                json.array_start();
                for parent in &commit.parents {
                    json.object_start();
                    json.key(b"sha");
                    json.string(&id::write(parent));
                    json.object_end();
                }
                json.array_end();
                json.key(b"commit");
                json.object_start();
                json.key(b"message");
                json.string(&commit.message);
                json.object_end();
                json.object_end();
            }
            json.array_end();
            json.key(b"files");
            json.array_start();
            json.array_end();
            json.object_end();
        }
        Answer::Tree { entries, truncated } => {
            json.object_start();
            json.key(b"truncated");
            json.boolean(*truncated);
            json.key(b"tree");
            if entries.is_empty() {
                json.null();
                json.object_end();
                return;
            }
            json.array_start();
            for entry in entries {
                json.object_start();
                json.key(b"path");
                json.string(&entry.path);
                json.key(b"mode");
                json.string(&mode(entry.mode));
                json.key(b"type");
                json.string(match entry.kind {
                    crate::api::EntryKind::Blob => b"blob",
                    crate::api::EntryKind::Tree => b"tree",
                    crate::api::EntryKind::Commit => b"commit",
                });
                json.key(b"sha");
                json.string(&id::write(&entry.sha));
                json.object_end();
            }
            json.array_end();
            json.object_end();
        }
        Answer::Files(values) => {
            json.array_start();
            for value in values {
                file(json, value);
            }
            json.array_end();
        }
        Answer::Compare { compare, commits, files } => {
            json.object_start();
            json.key(b"total_commits");
            json.unsigned(compare.total_commits);
            json.key(b"commits");
            json.array_start();
            for commit in commits {
                json.object_start();
                json.key(b"sha");
                json.string(&id::write(commit));
                json.object_end();
            }
            json.array_end();
            json.key(b"files");
            json.array_start();
            for value in files {
                file(json, value);
            }
            json.array_end();
            json.object_end();
        }
        Answer::Artifacts(_)
        | Answer::Error(_)
        | Answer::Comment(_)
        | Answer::Status(_)
        | Answer::Review(_)
        | Answer::Token(_)
        | Answer::Tokens(_)
        | Answer::User(_)
        | Answer::ApiSettings(_)
        | Answer::Repository(_)
        | Answer::Reachable(_)
        | Answer::RepoSearch { .. }
        | Answer::Search { .. }
        | Answer::Issues(_)
        | Answer::Issue(_)
        | Answer::Comments(_)
        | Answer::Pull(_)
        | Answer::Org(_)
        | Answer::CreatedFile { .. }
        | Answer::CreatedRepository(_)
        | Answer::Team(_)
        | Answer::Teams(_)
        | Answer::Hook(_)
        | Answer::Hooks(_)
        | Answer::Members(_)
        | Answer::CombinedStatus { .. }
        | Answer::Reviews(_)
        | Answer::ReviewComments(_)
        | Answer::Branch(_)
        | Answer::BranchRefs(_)
        | Answer::Branches(_)
        | Answer::Protection(_)
        | Answer::Protections(_)
        | Answer::Collaborators(_)
        | Answer::Permission(_)
        | Answer::Pulls(_)
        | Answer::Runs { .. }
        | Answer::Run(_)
        | Answer::Job { .. }
        | Answer::Jobs(_) => unreachable!("file answer selection"),
    }
}

fn mode(mut value: u32) -> Box<[u8]> {
    let mut digits = [b'0'; 11];
    let mut start = 10usize;
    for digit in digits.iter_mut().rev() {
        *digit = b'0'.saturating_add(u8::try_from(value % 8).expect("octal digit"));
        value = value.checked_div(8).expect("nonzero radix");
        if value == 0 {
            break;
        }
        start = start.saturating_sub(1);
    }
    skein_lib::bytes::copy_of(digits.get(start..).expect("octal range"))
}
