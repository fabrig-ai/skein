//! A named artifact, its run commit, and one bounded ZIP (forge.md, 5.2).
use super::{
    Body, Heard,
    plan::{Bounds, Step},
    tables::{Repository, Tables},
};
use crate::{
    api,
    forgejo::{
        document as d,
        request::{Repo, Request},
    },
};
use alloc::boxed::Box;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    Listing,
    Run,
    Zip,
}
#[derive(Debug)]
pub(super) struct Plan {
    row: Repository,
    run: u64,
    name: Box<[u8]>,
    max: u32,
    stage: Stage,
    id: Option<u64>,
    head: Option<api::Commit>,
}
impl Plan {
    pub(super) fn new(row: Repository, run: u64, name: Box<[u8]>, max: u32) -> Self {
        Self { row, run, name, max, stage: Stage::Listing, id: None, head: None }
    }
    fn at(&self) -> Repo {
        Repo { owner: self.row.owner.clone(), name: self.row.name.clone() }
    }
    pub(super) fn begin(&self) -> Step {
        Step::send(Request::Artifacts { at: self.at(), run: self.run, name: Some(self.name.clone()) })
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
            d::Row::Artifact(value) => value,
            d::Row::CommitMessage(_)
            | d::Row::Found(_)
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
        if self.stage != Stage::Listing
            || self.id.is_some()
            || value.id == 0
            || value.run_id != self.run
            || value.name != self.name
        {
            return Err(api::Error::InvalidAnswer);
        }
        if self.max > bounds.answer_bytes || value.size_in_bytes > u64::from(bounds.answer_bytes) {
            return Err(api::Error::TooLarge);
        }
        self.id = Some(value.id);
        Ok(())
    }
    fn done(&mut self, body: Body, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        match body {
            Body::Document(d::Document::Listed { skipped: 0 }) if self.stage == Stage::Listing => {
                self.id.ok_or(api::Error::Missing)?;
                self.stage = Stage::Run;
                Ok(Some(Step::send(Request::Run { at: self.at(), run: self.run })))
            }
            Body::Document(d::Document::Run(run)) if self.stage == Stage::Run => {
                if run.id != self.run {
                    return Err(api::Error::InvalidAnswer);
                }
                self.head = Some(
                    super::reading::commit(run.commit_sha, self.row.object_format).ok_or(api::Error::InvalidAnswer)?,
                );
                self.stage = Stage::Zip;
                Ok(Some(Step::send(Request::ArtifactZip { at: self.at(), id: self.id.expect("listed artifact") })))
            }
            Body::Raw { bytes, more } if self.stage == Stage::Zip => {
                if more {
                    return Err(api::Error::TooLarge);
                }
                let file = crate::forgejo::zip::read(&bytes, self.max, bounds.answer_bytes)?;
                Ok(Some(Step::Done(Ok(api::Answer::File {
                    head: self.head.expect("read run commit"),
                    path: file.path,
                    bytes: file.bytes,
                    truncated: file.truncated,
                }))))
            }
            Body::Malformed(
                crate::forgejo::decode::Malformed::Bound { .. } | crate::forgejo::decode::Malformed::TooLong { .. },
            ) => Err(api::Error::TooLarge),
            Body::Empty | Body::Raw { .. } | Body::Document(_) | Body::Malformed(_) => Err(api::Error::InvalidAnswer),
        }
    }
}
