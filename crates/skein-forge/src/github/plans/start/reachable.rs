//! One page for the repository picker (forge-github.md, 8.3).
use super::{Event, Page, State, Step, add, api, capacity, document, plan};
use crate::github::{pin, plans::Heard, request::Request};
use skein_lib::List;

#[derive(Debug)]
pub struct Reachable {
    page: u32,
    per_page: u32,
    state: State,
    rows: List<document::Reachable>,
    bytes: u32,
}
impl Reachable {
    #[must_use]
    pub fn new(page: u32, per_page: u32) -> Self {
        Self { page, per_page, state: State::new(), rows: List::with_capacity(0), bytes: 0 }
    }
    pub fn begin(&mut self, bounds: plan::Bounds) -> Result<Step<Page>, api::Error> {
        self.state.admission()?;
        if self.page == 0 {
            return Err(api::Error::Refused);
        }
        let limit = pin::PAGE_CAP.min(bounds.page_size).min(bounds.rows);
        if self.per_page == 0 || self.per_page > limit {
            return Err(api::Error::TooLarge);
        }
        self.rows = List::with_capacity(capacity(bounds)?.min(self.per_page));
        self.state.sending();
        Ok(Step::send(Request::Reachable { page: self.page, per_page: self.per_page }))
    }
    pub fn pending(&mut self) -> Option<plan::Exchange> {
        self.state.request.take()
    }
    pub fn set_pending(&mut self, exchange: plan::Exchange) {
        self.state.request = Some(exchange);
    }
    pub fn heard(&mut self, heard: Heard, bounds: plan::Bounds) -> Result<Option<Step<Page>>, api::Error> {
        self.state.order(&heard)?;
        let result = self.advance(heard, bounds);
        Ok(Step::settle(&mut self.state, result))
    }
    fn advance(&mut self, heard: Heard, bounds: plan::Bounds) -> Result<Option<Step<Page>>, api::Error> {
        match self.state.hear(heard, 200)? {
            None => Ok(None),
            Some(Event::Row(row)) => {
                let row = match row {
                    document::Row::Reachable(row) => row,
                    document::Row::Commit(_)
                    | document::Row::File(_)
                    | document::Row::Review(_)
                    | document::Row::Remark(_)
                    | document::Row::Collaborator(_)
                    | document::Row::Log(_)
                    | document::Row::Installation(_)
                    | document::Row::Branch(_)
                    | document::Row::SkippedBranch
                    | document::Row::Issue(_)
                    | document::Row::Pull(_)
                    | document::Row::Context(_)
                    | document::Row::Run(_)
                    | document::Row::Rule(_)
                    | document::Row::Comment(_) => return Err(api::Error::InvalidAnswer),
                };
                if self.rows.len() >= self.per_page || self.rows.len() >= bounds.rows {
                    return Err(api::Error::TooLarge);
                }
                self.bytes = add(self.bytes, &row.login, &row.name, bounds)?;
                match self.rows.push(row) {
                    Ok(()) => Ok(None),
                    Err(_) => Err(api::Error::TooLarge),
                }
            }
            Some(Event::Done { document, next }) => match document {
                document::Document::Rows => {
                    let rows = core::mem::replace(&mut self.rows, List::with_capacity(0));
                    Ok(Some(Step::terminal(&mut self.state, Ok(Page { rows: rows.into_boxed(), next }))))
                }
                document::Document::Delivery(_)
                | document::Document::Installation(_)
                | document::Document::Minted(_)
                | document::Document::Viewer(_)
                | document::Document::Merge(_)
                | document::Document::Enqueued(_)
                | document::Document::PullGraphql(_)
                | document::Document::Graphql(_)
                | document::Document::App(_)
                | document::Document::HookConfig(_)
                | document::Document::Conversion(_)
                | document::Document::User(_)
                | document::Document::Compare(_)
                | document::Document::Permission(_)
                | document::Document::Id(_)
                | document::Document::Number(_)
                | document::Document::Done
                | document::Document::Ruleset(_)
                | document::Document::ClassicProtection(_)
                | document::Document::ActionsPermissions(_)
                | document::Document::Workflows { .. }
                | document::Document::BotUser(_)
                | document::Document::Repository(_)
                | document::Document::Branch(_)
                | document::Document::Pull(_)
                | document::Document::Job(_)
                | document::Document::Combined(_)
                | document::Document::Issue(_)
                | document::Document::Error(_) => Err(api::Error::InvalidAnswer),
            },
        }
    }
}
