//! The App's install link, with host-issued authority (forge-setup.md, 5.4 and 5.7).
use super::{
    app,
    given::{self, Given, Refused},
    manifest,
};
use crate::{
    item::{self, Item},
    reconcile::{Event, Need, Outcome, Stage, Why},
};
use alloc::boxed::Box;
use skein_forge::github as dialect;
use skein_lib::Queue;

#[derive(Debug)]
struct Inputs {
    given: Given,
    state: Box<[u8]>,
}
#[derive(Debug)]
enum Phase {
    Begin(Inputs),
    Slug(Inputs),
    Ended,
}
/// Reads only the slug and returns an installation link (forge-setup.md, 5.4).
#[derive(Debug)]
pub struct Install {
    phase: Phase,
}
impl Install {
    /// Checks the given values and 5.7's state spelling before copying them.
    pub fn new(given: &Given, state: &[u8]) -> Result<Self, Refused> {
        given::check(given)?;
        manifest::check_state(state)?;
        Ok(Self { phase: Phase::Begin(Inputs { given: given.clone(), state: state.into() }) })
    }
}
/// Consumes one event and emits its next need; ended machines are silent.
pub fn step(machine: &mut Install, event: Event, out: &mut Queue<Need>) {
    let phase = core::mem::replace(&mut machine.phase, Phase::Ended);
    let need = match phase {
        Phase::Begin(inputs) => match event {
            Event::Begin => {
                machine.phase = Phase::Slug(inputs);
                Need::Read(Item::AppSlug)
            }
            Event::Read(_) | Event::Reply(_) | Event::Written | Event::Drawn(_) => {
                app::stop(inputs.given.forge, Stage::App, b"slug", Why::Unexpected)
            }
        },
        Phase::Slug(inputs) => match event {
            Event::Read(read) => {
                let slug = match &read {
                    Some(value) => item::app_slug(value.expose(), inputs.given.limits.name_bytes),
                    None => None,
                };
                match slug {
                    Some(slug) => match dialect::setup::install_page(
                        &inputs.given.web,
                        slug,
                        &inputs.state,
                        inputs.given.limits.page_bytes,
                    ) {
                        Some(url) => Need::Done(Outcome::Install { state: inputs.state, url }),
                        None => app::stop(inputs.given.forge, Stage::App, b"install", Why::Unexpected),
                    },
                    None => app::stop(inputs.given.forge, Stage::App, b"slug", Why::Missing),
                }
            }
            Event::Begin | Event::Reply(_) | Event::Written | Event::Drawn(_) => {
                app::stop(inputs.given.forge, Stage::App, b"slug", Why::Unexpected)
            }
        },
        Phase::Ended => return,
    };
    out.push(need);
}
