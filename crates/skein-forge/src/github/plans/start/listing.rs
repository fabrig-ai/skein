//! Every page of the App's installations (forge-github.md, 8.3).
use super::{Event, State, Step, api, capacity, document, keep_installation, plan};
use crate::github::{plans::Heard, request::Request};
use alloc::boxed::Box;
use skein_lib::List;

#[derive(Debug)]
pub struct Listing {
    state: State,
    page: u32,
    rows: List<document::Installation>,
    bytes: u32,
    count: u32,
}
impl Listing {
    #[must_use]
    #[expect(clippy::new_without_default, reason = "a plan begins explicitly; the programming model excludes Default")]
    pub fn new() -> Self {
        Self { state: State::new(), page: 1, rows: List::with_capacity(0), bytes: 0, count: 0 }
    }
    pub fn begin(&mut self, bounds: plan::Bounds) -> Result<Step<Box<[document::Installation]>>, api::Error> {
        self.state.admission()?;
        // Zero capacity still permits an empty listing; its first row is TooLarge.
        self.rows = List::with_capacity(capacity(bounds)?);
        Ok(self.send())
    }
    pub fn pending(&mut self) -> Option<plan::Exchange> {
        self.state.request.take()
    }
    pub fn set_pending(&mut self, exchange: plan::Exchange) {
        self.state.request = Some(exchange);
    }
    fn send(&mut self) -> Step<Box<[document::Installation]>> {
        self.state.sending();
        self.count = 0;
        Step::send(Request::Installations { page: self.page })
    }
    /// Call-order errors are `Err`; exchange verdicts finish the plan.
    pub fn heard(
        &mut self,
        heard: Heard,
        bounds: plan::Bounds,
    ) -> Result<Option<Step<Box<[document::Installation]>>>, api::Error> {
        self.state.order(&heard)?;
        let result = self.advance(heard, bounds);
        Ok(Step::settle(&mut self.state, result))
    }
    fn advance(
        &mut self,
        heard: Heard,
        bounds: plan::Bounds,
    ) -> Result<Option<Step<Box<[document::Installation]>>>, api::Error> {
        match self.state.hear(heard, 200)? {
            None => Ok(None),
            Some(Event::Row(row)) => {
                keep_installation(row, &mut self.rows, &mut self.bytes, &mut self.count, bounds)?;
                Ok(None)
            }
            Some(Event::Done { document, next }) => match document {
                document::Document::Rows => {
                    if next {
                        self.page = self.page.checked_add(1).ok_or(api::Error::TooLarge)?;
                        return Ok(Some(self.send()));
                    }
                    let rows = core::mem::replace(&mut self.rows, List::with_capacity(0));
                    Ok(Some(Step::terminal(&mut self.state, Ok(rows.into_boxed()))))
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
