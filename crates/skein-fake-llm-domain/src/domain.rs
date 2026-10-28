//! The fake provider's state and its entry points.
//!
//! Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, sections 4.4 and 6.3.

use alloc::boxed::Box;
use core::mem;

use skein_lib::{Deadlines, Duration, Env, Id, List, Queue, ReplyTo, Rng, Set, Slab, Time, Token};

use crate::api::{Answer, Error, Failure, Query, Rule, Rules, Script, Scripts, Usage};
use crate::{cache, limits, pieces, respond};

/// How the fake behaves, handed to every step read-only.
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Config {
    /// Calls held at once. A call beyond them is refused as overloaded.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub calls: u32,
    /// Owned query bytes, including message and part arrays.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub query_bytes: u32,
    /// Joint owned script, rule, reset-state and caller-menu bytes, including their arrays, held at startup.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub script_bytes: u32,
    /// Owned bytes in each answer held until its timer fires.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub answer_bytes: u32,
    /// The time to answer is drawn from `latency_min..=latency_max`.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub latency_min: Duration,
    /// Inclusive upper bound for the fake's seeded response latency.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub latency_max: Duration,
    /// The chance, per mille, that a call fails as overloaded.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub overloaded: u32,
    /// The chance, per mille, that a call fails as rate-limited.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub rate_limited: u32,
    /// What a rate-limit failure asks the client to wait.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub retry_after: Duration,
    /// The chances, per mille, that a call fails as unavailable, as too long
    /// for the context window, or as unauthorised.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub unavailable: u32,
    /// Chance per mille that the fake refuses the context window.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub too_long: u32,
    /// Chance per mille that the fake refuses credentials.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub unauthorized: u32,
    /// The chance, per mille, that an answer is refused by the content filter.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub refused: u32,
    /// The chance, per mille, that an answer says it calls tools and calls
    /// none.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub no_calls: u32,
    /// The most tokens a final answer takes: each takes between one and this
    /// many, and is cut short at the query's `max_tokens`.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub answer_tokens: u32,
    /// The most tool calls an answer makes: each that makes some makes
    /// between one and this many.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub calls_per_answer: u32,
    /// The chance, per mille, that a tool call uses an entry from the caller's
    /// configured invalid input/name menu. An empty invalid menu leaves the
    /// offered name and configured ordinary body unchanged.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub malformed: u32,
    /// Chance per mille of choosing an offered tool outside the query's choice.
    pub outside_choice: u32,
    /// Rounds of tool calls after each of the client's messages before the
    /// fake answers it.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub tool_rounds: u32,
    /// Lifetime from a prefix's write at injected monotonic time.
    pub cache_lifetime: Duration,
    /// Maximum written prefixes retained in the cache table.
    pub cache_entries: u32,
    /// Chance per mille that a request with no scope reads existing prefixes.
    pub unscoped_reads: u32,
}

/// protocol -> domain
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// A call: answer `query`.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    Call {
        /// Single-use right to answer this call; exactly one terminal consumes it.
        ///
        /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
        reply_to: ReplyTo,
        /// The protocol's stream token, echoed on pieces and holds.
        call: Token,
        /// Owned bounded neutral provider request, independent of agent vocabulary.
        ///
        /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
        query: Query,
    },
    /// Releases every call waiting at this name; sends nothing.
    ///
    /// Contract: docs/design/fake-llm.md, section 2.2.
    Release { name: Box<[u8]> },
}

/// domain -> protocol
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// One text piece before its call's terminal reply.
    ///
    /// Contract: docs/design/fake-llm.md, section 2.2.
    Piece { call: Token, usage: Usage, text: Box<[u8]> },
    /// The call waits for its owner to release a named hold.
    ///
    /// Contract: docs/design/fake-llm.md, section 2.2.
    Hold { call: Token, usage: Usage },
    /// The answer to a `Call`: exactly one per call.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    Reply {
        /// Single-use reply right returned to the layer that issued it.
        ///
        /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
        to: ReplyTo,
        /// The one terminal value for the enclosing call; ownership passes to its receiver.
        ///
        /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
        result: Result<Answer, Failure>,
    },
}

/// The fake provider's state.
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
#[derive(Debug)]
pub struct Domain {
    calls: Slab<Call>,
    held: Set<Id<Call>>,
    /// When each call is answered.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    timers: Deadlines<Id<Call>>,
    rng: Rng,
    /// Tool call ids issued.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    minted: u64,
    /// The conversations it plays from a script.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    played: Played,
    rules: Rules,
    menu: crate::api::Menu,
    cache: cache::Cache,
}

/// Retained scripts in Place order: cued, asked, then otherwise when present.
#[derive(Debug)]
pub(crate) struct Played {
    pub scripts: Box<[Script]>,
    pub cued: usize,
    pub asked: usize,
    pub resets: List<respond::Reset>,
}

impl Played {
    #[cfg(test)]
    pub(crate) fn new(config: &Config, scripts: Scripts, menu: &crate::api::Menu) -> Result<Self, Error> {
        Self::by_input(config, scripts, menu, &Rules { cued: Box::new([]), asked: Box::new([]) })
    }

    pub(crate) fn by_input(
        config: &Config,
        scripts: Scripts,
        menu: &crate::api::Menu,
        rules: &Rules,
    ) -> Result<Self, Error> {
        let Scripts { cued, asked, otherwise } = scripts;
        if !valid_rules(&rules.cued, &cued) || !valid_rules(&rules.asked, &asked) {
            return Err(Error::InvalidRequest);
        }
        let otherwise = Script { cue: Box::new([]), turns: otherwise };
        let fallback = if otherwise.turns.is_empty() { &[][..] } else { core::slice::from_ref(&otherwise) };
        let mut held = limits::rules(rules);
        for group in [&*cued, &*asked, fallback] {
            if !pieces::valid(group) {
                return Err(Error::InvalidRequest);
            }
            held = match limits::scripts(group) {
                Some(bytes) => match held {
                    Some(total) => total.checked_add(bytes),
                    None => None,
                },
                None => None,
            };
        }
        held = match limits::menu(menu) {
            Some(bytes) => match held {
                Some(total) => total.checked_add(bytes),
                None => None,
            },
            None => None,
        };
        if !limits::fits(held, config.script_bytes) {
            return Err(Error::ContextTooLong);
        }
        let cued_count = cued.len();
        let asked_count = asked.len();
        let scripts = if asked.is_empty() && fallback.is_empty() {
            cued
        } else {
            let count = cued_count.checked_add(asked_count).expect("admitted script count");
            let count = count.checked_add(fallback.len()).expect("admitted script count");
            let mut flat = List::with_capacity(u32::try_from(count).expect("admitted script bytes"));
            for script in cued.into_iter().chain(asked) {
                flat.push(script).expect("room for admitted scripts");
            }
            if !otherwise.turns.is_empty() {
                flat.push(otherwise).expect("room for fallback script");
            }
            flat.into_boxed()
        };
        let resets = List::with_capacity(respond::reset_count(&scripts).expect("admitted reset state bytes"));
        Ok(Played { scripts, cued: cued_count, asked: asked_count, resets })
    }
}

fn valid_rules(group: &[Box<[Rule]>], scripts: &[Script]) -> bool {
    if group.len() > scripts.len() {
        return false;
    }
    for (rules, script) in group.iter().zip(scripts) {
        let mut default = false;
        for rule in rules {
            let Ok(turn) = usize::try_from(rule.turn) else {
                return false;
            };
            if default || turn >= script.turns.len() {
                return false;
            }
            default = rule.cue.is_empty();
        }
        if !rules.is_empty() && !default {
            return false;
        }
    }
    true
}

/// A call being answered.
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
#[derive(Debug)]
pub(crate) struct Call {
    state: State,
}

#[derive(Debug)]
pub(crate) enum State {
    Streaming(pieces::Stream),
    Held(pieces::Stream),
    /// The answer is decided, and goes out when the call's timer fires.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    Thinking {
        reply_to: ReplyTo,
        result: Result<Answer, Failure>,
    },
    /// Terminal: holds nothing.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    Closed,
}

impl Domain {
    /// Constructs empty bounded state under the supplied immutable limits; no IO or clocks are consulted.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    #[must_use]
    pub fn new(config: &Config, seed: u64) -> Domain {
        Domain::scripted(config, seed, Box::new([]))
    }

    /// A provider that plays the conversations `scripts` cue from them, and
    /// the others at random.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    #[must_use]
    pub fn scripted(config: &Config, seed: u64, scripts: Box<[Script]>) -> Domain {
        Domain::try_scripted(config, seed, scripts).expect("scripts fit the provider limits")
    }

    /// Refuses a script collection exceeding the configured owned-byte cap.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub fn try_scripted(config: &Config, seed: u64, scripts: Box<[Script]>) -> Result<Domain, Error> {
        Self::configured(config, seed, scripts, crate::api::Menu { arguments: Box::new([]), invalid: Box::new([]) })
    }

    /// Admits caller-owned scripts and random input menus under one aggregate byte cap.
    /// No tool path, command, body or invalid name is supplied by this generic fake.
    pub fn configured(
        config: &Config,
        seed: u64,
        scripts: Box<[Script]>,
        menu: crate::api::Menu,
    ) -> Result<Domain, Error> {
        Self::played(config, seed, Scripts { cued: scripts, asked: Box::new([]), otherwise: Box::new([]) }, menu)
    }

    /// Admits system-cued scripts, request-cued scripts and fallback turns,
    /// together with the random menu under one byte cap.
    ///
    /// Contract: docs/design/fake-llm.md, section 2.4.
    pub fn played(config: &Config, seed: u64, scripts: Scripts, menu: crate::api::Menu) -> Result<Domain, Error> {
        Self::played_by_input(config, seed, scripts, menu, Rules { cued: Box::new([]), asked: Box::new([]) })
    }

    /// Admits scripts, menu and fresh-input rules under one byte cap.
    /// Missing or empty rule entries retain assistant-message counting.
    ///
    /// Contract: docs/design/fake-llm.md, section 2.4.
    pub fn played_by_input(
        config: &Config,
        seed: u64,
        scripts: Scripts,
        menu: crate::api::Menu,
        rules: Rules,
    ) -> Result<Domain, Error> {
        let played = Played::by_input(config, scripts, &menu, &rules)?;
        Ok(Domain {
            calls: Slab::with_capacity(config.calls),
            held: Set::with_capacity(config.calls),
            timers: Deadlines::with_capacity(config.calls),
            rng: Rng::new(seed),
            minted: 0,
            played,
            rules,
            menu,
            cache: cache::Cache::new(config.cache_entries, config.cache_lifetime),
        })
    }

    /// The admitted scripts, borrowed by the peer for route validation.
    #[must_use]
    pub fn scripts(&self) -> &[Script] {
        &self.played.scripts
    }

    /// Calls present, answered ones included until they are reclaimed.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    #[must_use]
    pub fn calls(&self) -> u32 {
        self.calls.len()
    }

    /// Earliest injected deadline still pending, or `None` when no timed work remains.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        self.timers.next()
    }

    /// Whether injected now reaches the earliest pending deadline; no live clock is read.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    #[must_use]
    pub fn is_due(&self, now: Time) -> bool {
        match self.timers.next() {
            Some(at) => at <= now,
            None => false,
        }
    }

    /// Iteration-end reclamation of retired entries; it must follow delivery of owned outputs.
    ///
    /// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
    pub fn reclaim(&mut self) {
        self.calls.reclaim();
    }
}

/// Handles one event, emitting requests into an uncapped queue.
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
pub fn step(domain: &mut Domain, env: &Env<Config>, event: Event, out: &mut Queue<Request>) {
    match event {
        Event::Call { reply_to, call: token, query } => call(domain, env, reply_to, token, &query, out),
        Event::Release { name } => release(domain, env, &name),
    }
}

/// Answers the earliest call due at `env.now`, if there is one.
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
pub fn fire(domain: &mut Domain, env: &Env<Config>, out: &mut Queue<Request>) {
    let Some(id) = domain.timers.expire(env.now) else {
        return;
    };
    let call = domain.calls.get_mut(id).expect("a call lives until its timer fires");
    let state = mem::replace(&mut call.state, State::Closed);
    call.state = match state {
        State::Thinking { reply_to, result } => answer(reply_to, result, out),
        State::Streaming(stream) => pieces::fire(stream, &domain.played.scripts, out),
        State::Held(_) => unreachable!("a held call has no timer"),
        State::Closed => unreachable!("a closed call has no timer"),
    };
    match &call.state {
        State::Streaming(_) => arm(&mut domain.timers, &mut domain.rng, id, env),
        State::Held(_) => {
            domain.held.insert(id).expect("at most one hold per call");
        }
        State::Closed => {
            domain.calls.retire(id);
        }
        State::Thinking { .. } => unreachable!("a fired answer ends its thinking"),
    }
}

fn call(
    domain: &mut Domain,
    env: &Env<Config>,
    reply_to: ReplyTo,
    token: Token,
    query: &Query,
    out: &mut Queue<Request>,
) {
    if domain.calls.is_full() {
        out.push(Request::Reply { to: reply_to, result: Err(Error::Overloaded.into()) });
        return;
    }
    let config = &env.limits;
    let mut place = None;
    let mut result = if !cache::valid(query) {
        Err(Error::InvalidRequest.into())
    } else if limits::fits(limits::query(query), config.query_bytes) {
        match respond::by_input(
            &mut domain.rng,
            &mut domain.minted,
            config,
            &mut domain.played,
            &domain.rules,
            &domain.menu,
            query,
            env.now,
        ) {
            Ok((answer, position)) if limits::fits(limits::answer(&answer), config.answer_bytes) => {
                place = position;
                Ok(answer)
            }
            Err((failure, position)) if limits::fits(limits::failure(&failure), config.answer_bytes) => {
                place = position;
                Err(failure)
            }
            Ok(_) | Err(_) => Err(Error::ContextTooLong.into()),
        }
    } else {
        Err(Error::ContextTooLong.into())
    };
    if let Ok(answer) = &mut result {
        let allowed = match query.caching {
            crate::api::Caching::Unscoped => domain.rng.chance(config.unscoped_reads),
            crate::api::Caching::Scope(_) | crate::api::Caching::Marks(_) => true,
        };
        let read = if allowed { cache::read(&domain.cache, query, env.now) } else { 0 };
        let written = cache::write(&mut domain.cache, query, env.now, read);
        answer.usage.input = Some(
            cache::tokens(query)
                .checked_sub(read)
                .expect("read within prompt")
                .checked_sub(written)
                .expect("write within uncached prompt"),
        );
        answer.usage.cache_read = Some(read);
        answer.usage.cache_write = Some(written);
    }
    let state = match place {
        Some(place) => {
            State::Streaming(pieces::Stream { reply_to, call: token, result, place, next: 0, passed: false })
        }
        None => State::Thinking { reply_to, result },
    };
    let call = Call { state };
    let id = domain.calls.insert(call).expect("checked for room above");
    arm(&mut domain.timers, &mut domain.rng, id, env);
}

/// Thinking, timer: the answer goes out.
///
/// Contract: docs/design/fake-llm.md, sections 2–5; programming-model.md, section 4.4.
fn answer(reply_to: ReplyTo, result: Result<Answer, Failure>, out: &mut Queue<Request>) -> State {
    out.push(Request::Reply { to: reply_to, result });
    State::Closed
}

fn arm(timers: &mut Deadlines<Id<Call>>, rng: &mut Rng, id: Id<Call>, env: &Env<Config>) {
    let latency = rng.between(env.limits.latency_min.as_nanos(), env.limits.latency_max.as_nanos());
    timers.arm(id, env.now.saturating_add(Duration::from_nanos(latency))).expect("one timer per call fits");
}

fn release(domain: &mut Domain, env: &Env<Config>, name: &[u8]) {
    for _ in 0..domain.held.len() {
        let mut found = None;
        for id in &domain.held {
            if pieces::held_at(&domain.calls.get(*id).expect("held call lives").state, &domain.played.scripts, name) {
                found = Some(*id);
                break;
            }
        }
        let Some(id) = found else {
            break;
        };
        domain.held.remove(&id);
        let call = domain.calls.get_mut(id).expect("held call lives");
        let state = mem::replace(&mut call.state, State::Closed);
        call.state = match state {
            State::Held(mut stream) => {
                stream.passed = true;
                State::Streaming(stream)
            }
            State::Streaming(_) | State::Thinking { .. } | State::Closed => unreachable!("found a held call"),
        };
        arm(&mut domain.timers, &mut domain.rng, id, env);
    }
}
