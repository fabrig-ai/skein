//! One trusted delivery, through the pushed-payload decoder (forge-github.md, 9.3).
use super::{Event, State, Step, api, document, plan};
use crate::github::{log::Logged, plans::Heard, request::Request};
use alloc::boxed::Box;

/// The payload is bounded by the answer bound in `bounds`; a caller matching
/// the pushed body limit sets the answer bound to it plus the log row's envelope.
#[derive(Debug)]
pub struct LogDelivery {
    id: u64,
    event: Box<[u8]>,
    state: State,
}
impl LogDelivery {
    /// Takes the id and bounded event word from the listed row.
    #[must_use]
    pub fn new(id: u64, event: Box<[u8]>) -> Self {
        Self { id, event, state: State::new() }
    }
    pub fn begin(&mut self, _bounds: plan::Bounds) -> Result<Step<Logged>, api::Error> {
        self.state.admission()?;
        self.state.sending();
        Ok(Step::send(Request::LogDelivery { id: self.id, event: core::mem::replace(&mut self.event, Box::from([])) }))
    }
    pub fn pending(&mut self) -> Option<plan::Exchange> {
        self.state.request.take()
    }
    pub fn set_pending(&mut self, exchange: plan::Exchange) {
        self.state.request = Some(exchange);
    }
    pub fn heard(&mut self, heard: Heard, _bounds: plan::Bounds) -> Result<Option<Step<Logged>>, api::Error> {
        self.state.order(&heard)?;
        let result = self.advance(heard);
        Ok(Step::settle(&mut self.state, result))
    }
    fn advance(&mut self, heard: Heard) -> Result<Option<Step<Logged>>, api::Error> {
        match self.state.hear(heard, 200)? {
            None => Ok(None),
            Some(Event::Row(_)) => Err(api::Error::InvalidAnswer),
            Some(Event::Done { document, .. }) => match document {
                document::Document::Delivery(value) => Ok(Some(Step::terminal(&mut self.state, Ok(value)))),
                document::Document::Installation(_)
                | document::Document::Minted(_)
                | document::Document::Viewer(_)
                | document::Document::Ruleset(_)
                | document::Document::ClassicProtection(_)
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
                | document::Document::BotUser(_)
                | document::Document::Repository(_)
                | document::Document::Branch(_)
                | document::Document::Pull(_)
                | document::Document::Job(_)
                | document::Document::Combined(_)
                | document::Document::Issue(_)
                | document::Document::ActionsPermissions(_)
                | document::Document::Workflows { .. }
                | document::Document::Rows
                | document::Document::Error(_) => Err(api::Error::InvalidAnswer),
            },
        }
    }
}
