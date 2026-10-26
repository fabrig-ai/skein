//! Submitted reviews and the shared domain's empty inline threads.
use super::commit;
use crate::{
    answers::{self, Body, Page},
    service::{Limits, Response},
};
use skein_fake_forge_domain::api as a;
use skein_forge::{api, forgejo::document::Body as Text, github::document as d};
use skein_lib::{List, Wall};

pub(super) fn reviews(answer: a::Answer, page: u32, size: u32, target: &[u8], limits: &Limits) -> Response {
    let a::Answer::Pull(pull) = answer else {
        return answers::failed();
    };
    let Some(page) = Page::new(page, size, u64::try_from(pull.reviews.len()).expect("domain reviews")) else {
        return answers::failed();
    };
    let mut rows = List::with_capacity(size);
    for (index, row) in pull.reviews.into_iter().enumerate() {
        if !page.contains(u64::try_from(index).expect("domain review index")) {
            continue;
        }
        let verdict = match row.verdict {
            a::Verdict::Approve => api::Verdict::Approve,
            a::Verdict::RequestChanges => api::Verdict::RequestChanges,
            a::Verdict::Comment => api::Verdict::Comment,
        };
        if rows
            .push(d::Review {
                id: row.id,
                author: row.author,
                verdict: Some(verdict),
                commit: commit(row.commit),
                body: Text::Kept(row.body),
                submitted: Some(Wall::from_nanos(row.at.as_nanos())),
                official: row.official,
            })
            .is_err()
        {
            return answers::failed();
        }
    }
    page.link(answers::response(200, &Body::Reviews(rows.as_slice()), limits), target, limits)
}
pub(super) fn comments(answer: a::Answer, review: u64, limits: &Limits) -> Response {
    let a::Answer::Pull(pull) = answer else {
        return answers::failed();
    };
    for row in &pull.reviews {
        if row.id == review {
            // The shared domain has no inline review-comment store. As in
            // the Forgejo layer, an existing review owns an empty thread.
            return answers::response(200, &Body::Remarks(&[]), limits);
        }
    }
    answers::missing(limits)
}
