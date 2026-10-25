//! Mint an installation token on the App's JWT (forge-github.md, 8.2).
use super::{Event, State, Step, api, document, plan};
use crate::github::{
    plans::Heard,
    request::{Request, Scope},
};

#[derive(Debug)]
pub struct Minting {
    installation: u64,
    scope: Scope,
    state: State,
}
impl Minting {
    #[must_use]
    pub fn new(installation: u64, scope: Scope) -> Self {
        Self { installation, scope, state: State::new() }
    }
    pub fn begin(&mut self, _bounds: plan::Bounds) -> Result<Step<document::Minted>, api::Error> {
        self.state.admission()?;
        self.state.sending();
        Ok(Step::send(Request::Mint { installation: self.installation, scope: self.scope }))
    }
    pub fn pending(&mut self) -> Option<plan::Exchange> {
        self.state.request.take()
    }
    pub fn set_pending(&mut self, exchange: plan::Exchange) {
        self.state.request = Some(exchange);
    }
    pub fn heard(&mut self, heard: Heard, _bounds: plan::Bounds) -> Result<Option<Step<document::Minted>>, api::Error> {
        self.state.order(&heard)?;
        let result = self.advance(heard);
        Ok(Step::settle(&mut self.state, result))
    }
    fn advance(&mut self, heard: Heard) -> Result<Option<Step<document::Minted>>, api::Error> {
        match self.state.hear(heard, 201)? {
            None => Ok(None),
            Some(Event::Row(_)) => Err(api::Error::InvalidAnswer),
            Some(Event::Done { document, .. }) => match document {
                document::Document::Minted(minted) => Ok(Some(Step::terminal(&mut self.state, Ok(minted)))),
                document::Document::Delivery(_)
                | document::Document::Installation(_)
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
