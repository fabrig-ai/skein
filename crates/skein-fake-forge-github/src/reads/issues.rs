//! Issue routes keep the shared numbering and page before the client filters kinds.
use crate::{
    answers::{self, Body, Page},
    service::{Limits, Response},
};
use skein_fake_forge_domain::api as a;
use skein_forge::{api, forgejo::document::Body as Text, github::document as d};
use skein_lib::{List, Wall};

fn issue(row: a::Summary) -> d::Issue {
    d::Issue {
        number: row.number,
        pull: row.kind == a::Kind::Pull,
        // The listing keeps only the merge fact: merged_at carries the
        // updated time, and a reader reads only its presence.
        merged: row.merged,
        state: match row.state {
            a::State::Open => api::State::Open,
            a::State::Closed => api::State::Closed,
        },
        author: row.author,
        labels: row.labels,
        updated_at: Wall::from_nanos(row.updated.as_nanos()),
        title: row.title,
        body: Text::Kept(row.body),
    }
}
pub(super) fn items(answer: a::Answer, target: &[u8], page: u32, size: u32, limits: &Limits) -> Response {
    let a::Answer::Items { items, more, .. } = answer else {
        return answers::failed();
    };
    let mut rows = List::with_capacity(size);
    for row in items {
        if rows.push(issue(row)).is_err() {
            return answers::failed();
        }
    }
    let Some(page) = listing(page, size, rows.len(), more) else {
        return answers::failed();
    };
    page.link_next(answers::response(200, &Body::Issues(rows.as_slice()), limits), target, limits)
}
pub(super) fn item(answer: a::Answer, limits: &Limits) -> Response {
    let a::Answer::Item { item, .. } = answer else {
        return answers::failed();
    };
    answers::response(200, &Body::Issue(&issue(item)), limits)
}
pub(super) fn comments(answer: a::Answer, target: &[u8], page: u32, size: u32, limits: &Limits) -> Response {
    let a::Answer::Comments(comments) = answer else {
        return answers::failed();
    };
    let Ok(total) = u64::try_from(comments.len()) else {
        return answers::failed();
    };
    let Some(page) = Page::new(page, size, total) else {
        return answers::failed();
    };
    let mut rows = List::with_capacity(size);
    for (index, row) in comments.into_iter().enumerate() {
        if page.contains(u64::try_from(index).expect("bounded comment index")) {
            let created = Wall::from_nanos(row.created.as_nanos());
            let updated = match row.edited {
                Some(at) => Wall::from_nanos(at.as_nanos()),
                None => created,
            };
            if rows
                .push(d::Comment {
                    id: row.id,
                    author: row.author,
                    created_at: Some(created),
                    updated_at: Some(updated),
                    body: Text::Kept(row.body),
                })
                .is_err()
            {
                return answers::failed();
            }
        }
    }
    page.link(answers::response(200, &Body::Comments(rows.as_slice()), limits), target, limits)
}
// The domain supplies a page and a more bit. One row beyond it is enough
// to advertise the next page; the caller discovers the remaining pages in turn.
pub(super) fn listing(page: u32, size: u32, rows: u32, more: bool) -> Option<Page> {
    let total = u64::from(page.checked_sub(1)?)
        .checked_mul(u64::from(size))?
        .checked_add(u64::from(rows))?
        .checked_add(u64::from(more))?;
    Page::new(page, size, total)
}
