//! The domain layer of a fake forge, for simulation worlds (fake-forge.md, section 2).
//!
//! A forge seen from the inside (fake-forge.md, section 2), Forgejo-shaped:
//! repositories with their users' permissions, labels, issues and pull
//! requests sharing one numbering, comments, reviews, CI statuses, merges, a
//! wiki, and the git a worker clones, fetches and pushes, all in one store,
//! so a branch a worker pushes is the head the engine reads, and a merge the
//! engine makes moves the branch the next fetch sees (fake-forge.md, sections 2 and 3).
//!
//! Calls come up from its protocol layer as [`Event::Call`] (the `boundary`
//! module states the contract), each by a user
//! (the engine, a person, CI: the forge treats them alike) about one
//! repository, and each is answered with exactly one [`Request::Reply`],
//! after a latency drawn from the configuration, sometimes late. A call is
//! decided when it arrives: refused for its user's rate, failed by chance
//! before it is made or timed out after, or made. It checks its client: it
//! refuses what Forgejo refuses (too little permission, what is missing, a
//! label not defined, someone else's comment, a stale head on merge, a merge
//! its base's protection does not allow, a push that is not a fast-forward,
//! what is past its limits), and it keeps what the API guarantees: one answer
//! per call, numbers, comment ids and revisions that only grow, and listings
//! paged as Forgejo pages them, by an updated time kept in seconds that
//! moves for what moves Forgejo's (the `reads` module).
//!
//! The forge moves on its own too. CI reports on every commit that becomes a
//! head, by chance or as its content cues (the `ci` module); a repository's
//! subscriber hears of each change by webhook, as [`Request::Hook`], after a
//! drawn latency, late, or never (the `hooks` module). A world sets it up,
//! and acts on it from outside its API, with free functions (the `scenario`
//! module: [`repository`], [`grant`], [`commit`], [`advance`], ...).
//! Everything that changes is observed, content and all, and so is every
//! write the forge refused, for a referee (testing-strategy.md, section 7): a
//! bounded queue a world drains ([`Domain::pop_observation`]).
//! [`Domain::inspect`] reads the store at settle, [`Domain::tally`] counts
//! what the forge did and refused, and [`Domain::room`] says what room is
//! left, so a world can tell a forge that filled up from one that works.
//!
//! Its vocabulary ([`api`]) is its own: it shares nothing with the engine's
//! domain, the worker's, or the git a working tree keeps. Between them sits a
//! protocol layer on each side, or a world translating. It follows the
//! programming model as any other step crate does, bounded by its [`Limits`].

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod actions;
pub mod api;
mod boundary;
mod ci;
mod creation;
mod dispatch;
mod domain;
mod faults;
mod git;
mod hooks;
mod issues;
mod limits;
mod next;
mod object;
mod observe;
pub mod promises;
mod pulls;
mod queue;
mod reads;
mod scenario;
mod store;
mod trees;
mod wiki;

#[cfg(test)]
mod tests;

pub use boundary::{Event, Request};
pub use creation::{create_file, create_repository};
pub use domain::{Config, Domain, MAX_OUT, Metadata, Room, Skew, Tally, fire, step, step_admitted, time};
pub use faults::{Rate, RateDenied, RateKey, rate};
pub use git::{Object, Signature, Tree};
pub use limits::{Limits, worst_case};
pub use object::object_bytes;
pub use observe::{Branches, Observation, Operation};
pub use scenario::{
    advance, check, commit, commit_tree, grant, merge_commit, repository, set_failing, set_reachable, set_refusing,
    settings, unborn_repository,
};

mod reports;
