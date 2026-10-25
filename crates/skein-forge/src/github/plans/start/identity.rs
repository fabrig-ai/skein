//! Read the bot's identity on an installation token (forge-github.md, 8.4).
use super::{Event, State, Step, api, document, plan};
use crate::github::{plans::Heard, request::Request};

#[derive(Debug)]
pub struct Identity {
    state: State,
}
impl Identity {
    #[must_use]
    #[expect(clippy::new_without_default, reason = "a plan begins explicitly; the programming model excludes Default")]
    pub fn new() -> Self {
        Self { state: State::new() }
    }
    pub fn begin(&mut self, _bounds: plan::Bounds) -> Result<Step<document::Viewer>, api::Error> {
        self.state.admission()?;
        self.state.sending();
        Ok(Step::send(Request::Viewer))
    }
    pub fn pending(&mut self) -> Option<plan::Exchange> {
        self.state.request.take()
    }
    pub fn set_pending(&mut self, exchange: plan::Exchange) {
        self.state.request = Some(exchange);
    }
    pub fn heard(&mut self, heard: Heard, _bounds: plan::Bounds) -> Result<Option<Step<document::Viewer>>, api::Error> {
        self.state.order(&heard)?;
        let result = self.advance(heard);
        Ok(Step::settle(&mut self.state, result))
    }
    fn advance(&mut self, heard: Heard) -> Result<Option<Step<document::Viewer>>, api::Error> {
        match self.state.hear(heard, 200)? {
            None => Ok(None),
            Some(Event::Row(_)) => Err(api::Error::InvalidAnswer),
            Some(Event::Done { document, .. }) => match document {
                document::Document::Viewer(viewer) => Ok(Some(Step::terminal(&mut self.state, Ok(viewer)))),
                // Sections 4.3 and 7 give no viewer-specific GraphQL error map.
                document::Document::Merge(_)
                | document::Document::Enqueued(_)
                | document::Document::PullGraphql(_)
                | document::Document::Graphql(_)
                | document::Document::Delivery(_)
                | document::Document::Installation(_)
                | document::Document::Minted(_)
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
