//! One page of the App's installations (forge-github.md, 8.3).
use super::{Event, InstallationPage, State, Step, api, capacity, document, keep_installation, plan};
use crate::github::{pin, plans::Heard, request::Request};
use skein_lib::List;

#[derive(Debug)]
pub struct Installations {
    page: u32,
    state: State,
    rows: List<document::Installation>,
    bytes: u32,
    count: u32,
}
impl Installations {
    #[must_use]
    pub fn new(page: u32) -> Self {
        Self { page, state: State::new(), rows: List::with_capacity(0), bytes: 0, count: 0 }
    }
    pub fn begin(&mut self, bounds: plan::Bounds) -> Result<Step<InstallationPage>, api::Error> {
        self.state.admission()?;
        if self.page == 0 {
            return Err(api::Error::Refused);
        }
        self.rows = List::with_capacity(capacity(bounds)?.min(pin::PAGE_CAP));
        self.state.sending();
        Ok(Step::send(Request::Installations { page: self.page }))
    }
    pub fn pending(&mut self) -> Option<plan::Exchange> {
        self.state.request.take()
    }
    pub fn set_pending(&mut self, exchange: plan::Exchange) {
        self.state.request = Some(exchange);
    }
    pub fn heard(&mut self, heard: Heard, bounds: plan::Bounds) -> Result<Option<Step<InstallationPage>>, api::Error> {
        self.state.order(&heard)?;
        let result = self.advance(heard, bounds);
        Ok(Step::settle(&mut self.state, result))
    }
    fn advance(&mut self, heard: Heard, bounds: plan::Bounds) -> Result<Option<Step<InstallationPage>>, api::Error> {
        match self.state.hear(heard, 200)? {
            None => Ok(None),
            Some(Event::Row(row)) => {
                keep_installation(row, &mut self.rows, &mut self.bytes, &mut self.count, bounds)?;
                Ok(None)
            }
            Some(Event::Done { document, next }) => match document {
                document::Document::Rows => {
                    let rows = core::mem::replace(&mut self.rows, List::with_capacity(0));
                    Ok(Some(Step::terminal(&mut self.state, Ok(InstallationPage { rows: rows.into_boxed(), next }))))
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
