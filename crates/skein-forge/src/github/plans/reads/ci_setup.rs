//! Two bounded, independent repository setup facts (forge-github.md, 4.1).
use super::{Bounds, Plan, Response, Stage, Step, Tables, api, document, failure};
use crate::github::{
    plans::{Body, Heard},
    request::Request,
};

impl Plan {
    pub(super) fn ci_setup_initial(&mut self) -> Result<Step, api::Error> {
        let row = self.row.as_ref().ok_or(api::Error::InvalidAnswer)?;
        self.stage = Stage::Actions;
        Ok(self.send(Request::ActionsPermissions { owner: row.owner.clone(), name: row.name.clone() }))
    }
    fn ci_setup_next(&mut self, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        if self.stage == Stage::Actions {
            let row = self.row.as_ref().ok_or(api::Error::InvalidAnswer)?;
            self.stage = Stage::Workflows;
            return Ok(Some(self.send(Request::Workflows { owner: row.owner.clone(), name: row.name.clone() })));
        }
        self.terminal(api::Answer::CiSetup(self.ci_setup), bounds)
    }
    pub(super) fn ci_setup_heard(
        &mut self,
        heard: Heard,
        bounds: Bounds,
        tables: &mut Tables,
    ) -> Result<Option<Step>, api::Error> {
        match heard {
            Heard::Head { status, head } => {
                if status == 401 || status == 301 || status == 307 {
                    return Err(self.read_failure(status, &head, None, tables));
                }
                self.response =
                    if status == 200 { Response::Body { next: head.next } } else { Response::Refusal { status, head } };
                Ok(None)
            }
            Heard::Row(_) => Err(api::Error::InvalidAnswer),
            Heard::Failed(failed) => match &self.response {
                Response::Refusal { status, head } => Err(self.read_failure(*status, head, None, tables)),
                Response::None | Response::Head | Response::Body { .. } => Err(failure::transport(failed)),
            },
            Heard::Done(body) => {
                match &self.response {
                    Response::Refusal { status, head } => {
                        let refusal = match &body {
                            Body::Document(document::Document::Error(refusal)) => Some(refusal),
                            Body::Document(_) | Body::Malformed(_) | Body::Empty | Body::Raw { .. } => None,
                        };
                        // Optional route refusals do not invalidate a known repository.
                        let error = failure::read(
                            crate::github::failure::Asked::Leaf,
                            *status,
                            head,
                            refusal,
                            self.repository,
                            tables,
                        );
                        match error {
                            api::Error::RateLimited { .. } => return Err(error),
                            api::Error::Unavailable
                            | api::Error::Timeout
                            | api::Error::Forbidden
                            | api::Error::Missing
                            | api::Error::MissingJob
                            | api::Error::TooLarge
                            | api::Error::Empty
                            | api::Error::Full
                            | api::Error::Exists
                            | api::Error::NothingToMerge
                            | api::Error::Closed
                            | api::Error::Stale
                            | api::Error::Conflict
                            | api::Error::Protected
                            | api::Error::Refused
                            | api::Error::InvalidAnswer
                            | api::Error::Busy
                            | api::Error::NotConnected
                            | api::Error::NotInstalled => {}
                        }
                    }
                    Response::Body { next } => match body {
                        Body::Document(document::Document::ActionsPermissions(enabled))
                            if self.stage == Stage::Actions =>
                        {
                            self.ci_setup.actions = Some(enabled);
                        }
                        Body::Document(document::Document::Workflows { total, count })
                            if self.stage == Stage::Workflows =>
                        {
                            self.ci_setup.workflows = if count == 1 && total >= 1 {
                                Some(true)
                            } else if count == 0 && total == 0 && !next {
                                Some(false)
                            } else {
                                None
                            };
                        }
                        Body::Document(_) | Body::Malformed(_) | Body::Empty | Body::Raw { .. } => {}
                    },
                    Response::None | Response::Head => return Err(api::Error::InvalidAnswer),
                }
                self.ci_setup_next(bounds)
            }
        }
    }
}
