//! A bounded whole comparison, reversed before its oldest-first page (forge.md, 5.2).
use super::{
    Body, Heard,
    plan::{Bounds, Step},
    reading,
    tables::{Repository, Tables},
};
use crate::{
    api,
    forgejo::{
        document as d,
        id::ObjectId,
        request::{Repo, Request},
    },
};
use skein_lib::List;

#[derive(Debug)]
pub(super) struct Plan {
    row: Repository,
    before: api::Commit,
    after: api::Commit,
    skip: u32,
    commits: List<api::CommitMessage>,
    held: u64,
}
impl Plan {
    pub(super) fn new(
        before: api::Commit,
        after: api::Commit,
        page: u32,
        row: Repository,
        bounds: Bounds,
    ) -> Result<Self, api::Error> {
        let skip =
            page.checked_sub(1).ok_or(api::Error::Empty)?.checked_mul(bounds.page_size).ok_or(api::Error::TooLarge)?;
        if skip >= bounds.rows {
            return Err(api::Error::TooLarge);
        }
        let capacity = bounds.rows.min(
            bounds
                .answer_bytes
                .checked_div(u32::try_from(size_of::<api::CommitMessage>()).expect("commit width"))
                .expect("nonzero width"),
        );
        Ok(Self { row, before, after, skip, commits: List::with_capacity(capacity), held: 0 })
    }
    pub(super) fn begin(&mut self) -> Step {
        let at = Repo { owner: self.row.owner.clone(), name: self.row.name.clone() };
        Step::send(Request::Commits {
            at,
            before: ObjectId { bytes: self.before, format: self.row.object_format },
            after: ObjectId { bytes: self.after, format: self.row.object_format },
        })
    }
    pub(super) fn heard(
        &mut self,
        heard: Heard,
        bounds: Bounds,
        repository: api::Repository,
        tables: &mut Tables,
    ) -> Result<Option<Step>, api::Error> {
        match heard {
            Heard::Head { status, fields } => {
                if status != 200 {
                    let failure =
                        super::failure::classify(super::failure::Asked::Leaf, status, None, fields.retry_after);
                    match failure.drop {
                        super::failure::Drop::Nothing => {}
                        super::failure::Drop::Path | super::failure::Drop::Row => tables.drop_repository(repository),
                    }
                    return Err(failure.error);
                }
                Ok(None)
            }
            Heard::Failed(fault) => Err(super::failure::transport(fault)),
            Heard::Row(row) => {
                self.received(row, bounds)?;
                Ok(None)
            }
            Heard::Done(body) => self.done(body, bounds),
        }
    }

    fn received(&mut self, row: d::Row, bounds: Bounds) -> Result<(), api::Error> {
        let value = match row {
            d::Row::CommitMessage(value) => value,
            d::Row::Found(_)
            | d::Row::Reachable(_)
            | d::Row::Token(_)
            | d::Row::User(_)
            | d::Row::Issue(_)
            | d::Row::Comment(_)
            | d::Row::Review(_)
            | d::Row::Status(_)
            | d::Row::ReviewComment(_)
            | d::Row::Pull(_)
            | d::Row::Branch(_)
            | d::Row::Protection(_)
            | d::Row::Entry(_)
            | d::Row::Run(_)
            | d::Row::Hook(_)
            | d::Row::Team(_)
            | d::Row::Step(_)
            | d::Row::Job(_)
            | d::Row::File(_)
            | d::Row::Commit(_) => return Err(api::Error::InvalidAnswer),
        };
        let format = self.row.object_format;
        let id = reading::commit(value.id, format).ok_or(api::Error::InvalidAnswer)?;
        if value.parents.len() > usize::try_from(bounds.rows).expect("row bound fits") {
            return Err(api::Error::TooLarge);
        }
        let mut parents = List::with_capacity(u32::try_from(value.parents.len()).or(Err(api::Error::TooLarge))?);
        for parent in value.parents {
            parents.push(reading::commit(parent, format).ok_or(api::Error::InvalidAnswer)?).expect("one parent per id");
        }
        let value = api::CommitMessage { id, parents: parents.into_boxed(), message: value.message };
        let size = u64::try_from(size_of::<api::CommitMessage>())
            .expect("commit width")
            .checked_add(
                u64::try_from(value.parents.len())
                    .expect("bounded parents")
                    .checked_mul(32)
                    .ok_or(api::Error::TooLarge)?,
            )
            .ok_or(api::Error::TooLarge)?
            .checked_add(u64::try_from(value.message.len()).expect("bounded message"))
            .ok_or(api::Error::TooLarge)?;
        self.held = self.held.checked_add(size).ok_or(api::Error::TooLarge)?;
        if self.held > u64::from(bounds.answer_bytes) {
            return Err(api::Error::TooLarge);
        }
        self.commits.push(value).or(Err(api::Error::TooLarge))?;
        Ok(())
    }

    fn done(&mut self, body: Body, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        match body {
            Body::Document(d::Document::Compare(compare)) => {
                if compare.total_commits != u64::from(self.commits.len()) {
                    return Err(api::Error::InvalidAnswer);
                }
            }
            Body::Malformed(
                crate::forgejo::decode::Malformed::Bound { .. } | crate::forgejo::decode::Malformed::TooLong { .. },
            ) => return Err(api::Error::TooLarge),
            Body::Document(_) | Body::Empty | Body::Raw { .. } | Body::Malformed(_) => {
                return Err(api::Error::InvalidAnswer);
            }
        }
        let end = self.skip.checked_add(bounds.page_size).ok_or(api::Error::TooLarge)?;
        let more = self.commits.len() > end;
        let mut page = List::with_capacity(bounds.page_size.min(self.commits.len().saturating_sub(self.skip)));
        let commits = core::mem::replace(&mut self.commits, List::with_capacity(0)).into_boxed();
        for value in commits
            .into_iter()
            .rev()
            .skip(usize::try_from(self.skip).expect("u32 fits"))
            .take(usize::try_from(bounds.page_size).expect("u32 fits"))
        {
            page.push(value).expect("page capacity");
        }
        Ok(Some(Step::Done(Ok(api::Answer::Commits { commits: page.into_boxed(), more }))))
    }
}
