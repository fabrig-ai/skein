use super::{
    Answer, Route, Unrouted,
    items::{number_value, page},
};
use crate::forgejo::{
    document::{Body, Review, ReviewComment, ReviewState},
    id,
    limits::Limits,
    path,
    request::{Method, Repo, Request},
    time,
};
use skein_json::writer::{self, Encoder};
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
        [pulls, number, reviews] if &**pulls == b"pulls" && &**reviews == b"reviews" => {
            Request::Reviews { at, number: number_value(number)?, page: page(&target.query)? }
        }
        [pulls, number, reviews, review, comments]
            if &**pulls == b"pulls" && &**reviews == b"reviews" && &**comments == b"comments" =>
        {
            Request::ReviewComments { at, number: number_value(number)?, review: number_value(review)? }
        }
        [pulls] if &**pulls == b"pulls" => Request::Pulls { at, page: page(&target.query)? },
        _ => return Err(Unrouted::NotFound),
    };
    match method {
        Method::Get => Ok(Route::Ready(request)),
        Method::Post | Method::Put | Method::Patch | Method::Delete => Err(Unrouted::Method),
    }
}
pub(super) fn state(value: ReviewState) -> &'static [u8] {
    match value {
        ReviewState::Approved => b"APPROVED",
        ReviewState::RequestChanges => b"REQUEST_CHANGES",
        ReviewState::Comment => b"COMMENT",
        ReviewState::Pending => b"PENDING",
        ReviewState::RequestReview => b"REQUEST_REVIEW",
    }
}
fn body(json: &mut Encoder, value: &Body) -> Result<(), writer::Refusal> {
    match value {
        Body::Kept(text) => json.string(text),
        Body::Long(_) => return Err(writer::Refusal::TooLong),
    }
    Ok(())
}
pub(super) fn review(json: &mut Encoder, value: &Review) -> Result<(), writer::Refusal> {
    json.object_start();
    json.key(b"id");
    json.unsigned(value.id);
    json.key(b"user");
    super::start::user(json, &value.user);
    json.key(b"state");
    json.string(state(value.state));
    json.key(b"commit_id");
    json.string(&id::write(&value.commit_id));
    json.key(b"body");
    body(json, &value.body)?;
    for (key, at) in [(b"submitted_at".as_slice(), value.submitted_at), (b"updated_at", value.updated_at)] {
        json.key(key);
        match at {
            Some(at) => json.string(&time::write(at)),
            None => json.null(),
        }
    }
    json.key(b"official");
    json.boolean(value.official);
    json.key(b"dismissed");
    json.boolean(value.dismissed);
    json.object_end();
    Ok(())
}
fn comment(json: &mut Encoder, value: &ReviewComment) -> Result<(), writer::Refusal> {
    json.object_start();
    json.key(b"id");
    json.unsigned(value.id);
    json.key(b"user");
    super::start::user(json, &value.user);
    json.key(b"path");
    json.string(&value.path);
    json.key(b"position");
    json.unsigned(u64::from(value.position));
    json.key(b"body");
    body(json, &value.body)?;
    json.object_end();
    Ok(())
}
pub(super) fn write(json: &mut Encoder, answer: &Answer) -> Result<(), writer::Refusal> {
    match answer {
        Answer::Reviews(values) => {
            json.array_start();
            for value in values {
                review(json, value)?;
            }
            json.array_end();
        }
        Answer::ReviewComments(values) => {
            json.array_start();
            for value in values {
                comment(json, value)?;
            }
            json.array_end();
        }
        Answer::Pulls(values) => {
            json.array_start();
            for value in values {
                super::items::pull(json, value);
            }
            json.array_end();
        }
        Answer::Error(_)
        | Answer::Comment(_)
        | Answer::Status(_)
        | Answer::Review(_)
        | Answer::Files(_)
        | Answer::Compare { .. }
        | Answer::Tree { .. }
        | Answer::Runs { .. }
        | Answer::Run(_)
        | Answer::Job { .. }
        | Answer::Jobs(_)
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
        | Answer::Branch(_)
        | Answer::BranchRefs(_)
        | Answer::Branches(_)
        | Answer::Protection(_)
        | Answer::Protections(_)
        | Answer::Collaborators(_)
        | Answer::Permission(_) => unreachable!("review answer selection"),
    }
    Ok(())
}
