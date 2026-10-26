mod app;
mod app_facts;
mod checks;
pub(crate) mod fixtures;
mod git;
mod installations;
mod job_logs;
mod keys;
mod pages_conversion;
mod pages_install;
mod pages_manifest;
mod service_app;
mod service_limits;
mod service_reads;

use crate::{
    app::App,
    installations::{Installations, Kind, Limits, Reach},
    keys::KEYS,
};
use alloc::boxed::Box;
use skein_lib::{Secret, Wall};

fn wall(seconds: u64) -> Wall {
    Wall::from_nanos(seconds.checked_mul(1_000_000_000).expect("fixture time"))
}
fn app() -> App {
    App {
        id: 7,
        slug: Box::from(*b"test-app"),
        owner: 1,
        permissions: Box::from([]),
        events: Box::from([]),
        key: KEYS.first().expect("first test key"),
        webhook: Secret::new(Box::from(*b"test-webhook")),
    }
}
fn store(seed: u64) -> Installations {
    let mut rows = Installations::new(&Limits { installations: 2, tokens: 2, listed: 2 }, seed);
    assert_eq!(rows.install(1, b"person", Kind::User, Reach::All, wall(1000)), Ok(1));
    rows
}

mod issues;
mod read_support;

mod people;
mod pulls;
mod reviews;

mod queries;

mod queue;
mod write_issues;
mod write_support;

mod write_branches;
mod write_pulls;

mod write_merge;

mod protection;

mod auth;

mod byte_faults;
mod deliveries;
mod delivery_http;

mod rates;

mod mutations;

mod comment_edit;
