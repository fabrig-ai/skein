//! Calls share the component's connections and entry-point output reservations.
use super::{Body, CallPlan, Component, Heard, Phase, Plan, Start, Step, Waiting, api};
use crate::connection::{self, Exchange};
use crate::{
    boundary::{Answered, Event},
    limits::{self, Limits, Listing, Shared},
};
use skein_forge::forgejo::plans::plan::Bounds;
use skein_lib::{Env, Id, List, Queue, Token};
impl Component {
    pub(super) fn expire(
        &mut self,
        call: Token,
        id: Id<Waiting>,
        up: &mut Queue<Event>,
        io: &mut Queue<skein_io::Request>,
    ) {
        let waiting = self.waiting.get(id).expect("indexed call");
        let error = if waiting.cost == 0 { api::Error::Unavailable } else { api::Error::Timeout };
        let error = match &waiting.plan {
            Some(CallPlan::Forgejo(plan)) => plan.error(error),
            Some(CallPlan::GitHub(_)) | None => error,
        };
        if let Some(token) = waiting.connection {
            self.connection_slab
                .get_mut(*self.connections.get(&token).expect("holder"))
                .expect("connection")
                .close(true, io);
        }
        self.finish(call, id, Err(error), up);
    }
    pub(super) fn finish(
        &mut self,
        call: Token,
        id: Id<Waiting>,
        result: Result<api::Answer, api::Error>,
        up: &mut Queue<Event>,
    ) {
        let waiting = self.waiting.get(id).expect("indexed call");
        let cost = waiting.cost;
        let free = waiting.free;
        let rate = waiting.rate;
        if let Some(token) = waiting.connection {
            let _holder = self.holders.remove(&token);
        }
        self.waiting.retire(id);
        let _call = self.calls.remove(&call);
        up.push(Event::Answered(Answered { call, cost, free, rate, result }));
    }
    pub(super) fn drive_calls(
        &mut self,
        env: &Env<Limits>,
        up: &mut Queue<Event>,
        io: &mut Queue<skein_io::Request>,
        above_room: u32,
        below_room: u32,
    ) {
        let above = up.len();
        let below = io.len();
        let mut calls = List::with_capacity(self.calls.capacity());
        for (call, id) in &self.calls {
            calls.push((*call, *id)).expect("bounded calls");
        }
        for (call, id) in calls.into_boxed() {
            if up.len().saturating_sub(above).saturating_add(1) >= above_room
                || io.len().saturating_sub(below).saturating_add(1) >= below_room
            {
                self.wake = Some(env.now);
                break;
            }
            if self.closed {
                break;
            }
            let waiting = self.waiting.get(id).expect("indexed call");
            if waiting.deadline <= env.now {
                self.expire(call, id, up, io);
                continue;
            }
            let forge = waiting.repository.forge;
            let mut at = None;
            for n in 0..self.settings.forges.len() {
                if self.settings.forges.get(n).expect("forge").number == forge {
                    at = Some(n);
                    break;
                }
            }
            let Some(at) = at else {
                self.finish(call, id, Err(api::Error::Unavailable), up);
                continue;
            };
            if waiting.connection.is_none() {
                if waiting.revoked {
                    let error = match waiting.plan.as_ref() {
                        Some(CallPlan::Forgejo(plan)) => plan.error(api::Error::Forbidden),
                        Some(CallPlan::GitHub(_)) | None => api::Error::Forbidden,
                    };
                    self.finish(call, id, Err(error), up);
                    continue;
                }
                if waiting.plan.is_none() {
                    match *self.starts.get(at).expect("forge start") {
                        Start::Unconnected | Start::Stopped(_) => {
                            self.finish(call, id, Err(api::Error::NotConnected), up);
                            continue;
                        }
                        Start::Listing { .. }
                        | Start::Minting { .. }
                        | Start::Identity { .. }
                        | Start::Checking { .. }
                        | Start::Settings { .. }
                        | Start::Waiting { .. } => continue,
                        Start::Ready { .. } => {}
                    }
                }
            }
            if self.settings.forges.get(at).expect("forge").kind == crate::settings::Kind::GitHub {
                self.drive_github_call(
                    (call, id),
                    at,
                    env,
                    up,
                    io,
                    (
                        above_room.saturating_sub(up.len().saturating_sub(above)),
                        below_room.saturating_sub(io.len().saturating_sub(below)),
                    ),
                );
                continue;
            }
            if !self.assign(call, id, at, env, up, io) {
                continue;
            }
            self.pump_call(
                (call, id),
                at,
                env,
                up,
                io,
                (
                    above_room.saturating_sub(up.len().saturating_sub(above)),
                    below_room.saturating_sub(io.len().saturating_sub(below)),
                ),
            );
        }
        for (_, id) in &self.connections {
            if io.len().saturating_sub(below) >= below_room {
                self.wake = Some(env.now);
                break;
            }
            let c = self.connection_slab.get_mut(*id).expect("indexed connection");
            if c.drain_redirect(env, io, below_room.saturating_sub(io.len().saturating_sub(below)).saturating_sub(1)) {
                self.wake = Some(env.now);
            }
            if self.closed || (c.phase == Phase::Idle && c.deadline <= Some(env.now)) || c.phase == Phase::Closed {
                c.close(c.in_flight(), io);
            }
        }
    }
    pub(super) fn assign(
        &mut self,
        call: Token,
        id: Id<Waiting>,
        at: u32,
        env: &Env<Limits>,
        up: &mut Queue<Event>,
        io: &mut Queue<skein_io::Request>,
    ) -> bool {
        let waiting = self.waiting.get(id).expect("call");
        if waiting.connection.is_some() {
            return true;
        }
        let forge = waiting.repository.forge;
        let cap = match *self.starts.get(at).expect("start") {
            Start::Ready { cap } => cap,
            Start::Listing { .. }
            | Start::Minting { .. }
            | Start::Identity { .. }
            | Start::Unconnected
            | Start::Checking { .. }
            | Start::Settings { .. }
            | Start::Waiting { .. }
            | Start::Stopped(_) => {
                return false;
            }
        };
        let mut token = None;

        let mut other = None;
        let call_connections = self.call_connections();
        for (owner, connection) in &self.connections {
            let c = self.connection_slab.get(*connection).expect("indexed connection");
            let mut reserved = false;
            for n in 0..self.own_tokens.len() {
                if self.own_tokens.get(n) == Some(&Some(*owner))
                    && self.settings.forges.get(n).expect("forge").kind == crate::settings::Kind::GitHub
                {
                    reserved = true;
                }
            }
            if reserved {
                continue;
            }
            if c.phase == Phase::Idle && !c.closing && !self.holders.contains_key(owner) {
                if c.forge == forge && c.origin == waiting.origin() {
                    token = Some(*owner);
                    break;
                }
                let mut start = false;
                for start_token in &self.own_tokens {
                    if *start_token == Some(*owner) {
                        start = true;
                    }
                }
                if !start {
                    other = Some(*connection);
                }
            }
        }
        if token.is_none()
            && call_connections < env.limits.connections
            && self.connections.len() < self.connections.capacity()
        {
            let origin = waiting.origin();
            token = Some(self.open_call_origin(at, origin, env, io));
        }
        if token.is_none() {
            if let Some(other) = other {
                self.connection_slab.get_mut(other).expect("idle other forge").close(false, io);
            }
            return false;
        }
        let token = token.expect("selected connection");
        if self.own_tokens.get(at) == Some(&Some(token)) {
            *self.own_tokens.get_mut(at).expect("forge slot") = None;
        }
        let _old = self.holders.insert(token, call).expect("one holder per connection");
        let waiting = self.waiting.get_mut(id).expect("indexed call");
        waiting.connection = Some(token);
        if waiting.plan.is_some() {
            return true;
        }
        if self.settings.forges.get(at).expect("forge").kind == crate::settings::Kind::GitHub {
            return self.begin_github_call(call, id, env, up, io);
        }
        let op = waiting.op.take().expect("waiting operation");
        waiting.page_size = page_size(&op, cap, &env.limits, &self.shared);
        let mut plan = CallPlan::Forgejo(Plan::new(waiting.repository, op));
        let step = match &mut plan {
            CallPlan::Forgejo(plan) => plan.begin(
                bounds(waiting.page_size, &env.limits, &self.shared),
                &self.settings.word,
                &mut self.forgejo_tables,
            ),
            CallPlan::GitHub(_) => unreachable!("Forgejo admission"),
        };
        waiting.plan = Some(plan);
        match step {
            Ok(step) => {
                if self.call_step(id, step, env, up, io, call) {
                    return false;
                }
            }
            Err(error) => {
                self.finish(call, id, Err(error), up);
                return false;
            }
        }
        true
    }
    fn pump_call(
        &mut self,
        holder: (Token, Id<Waiting>),
        at: u32,
        env: &Env<Limits>,
        up: &mut Queue<Event>,
        io: &mut Queue<skein_io::Request>,
        budget: (u32, u32),
    ) {
        let (call, id) = holder;
        let above = up.len();
        let below = io.len();
        let token = self.waiting.get(id).expect("call").connection.expect("holder");
        let connection = *self.connections.get(&token).expect("indexed connection");
        for _ in 0..64_u32 {
            if up.len().saturating_sub(above) >= budget.0
                || io.len().saturating_sub(below).saturating_add(1) >= budget.1
            {
                self.wake = Some(env.now);
                break;
            }
            let c = self.connection_slab.get_mut(connection).expect("holding connection");
            if c.phase == Phase::Idle {
                let waiting = self.waiting.get_mut(id).expect("indexed call");
                if waiting.revoked {
                    c.deadline = Some(env.now.saturating_add(env.limits.idle));
                    let error = match waiting.plan.as_ref().expect("call plan") {
                        CallPlan::Forgejo(plan) => plan.error(api::Error::Forbidden),
                        CallPlan::GitHub(_) => api::Error::Forbidden,
                    };
                    self.finish(call, id, Err(error), up);
                    break;
                }
                let deadline = waiting.deadline;
                let plan = forgejo_plan(waiting);
                if let Some(request) = plan.pending() {
                    c.deadline = Some(deadline);
                    c.raw_room(plan.raw_room(self.shared.answer_bytes));
                    let forge = self.settings.forges.get(at).expect("forge");
                    let authorization = skein_forge::forgejo::credential::authorization(
                        &forge.access.as_ref().expect("held access").credential,
                    )
                    .expect("checked credential length");
                    match c.start(
                        env,
                        forge,
                        &self.settings.user_agent,
                        Exchange::Forgejo { request: request.request, range: request.range, authorization },
                    ) {
                        Ok(()) => {
                            c.keep_after(plan.after());
                        }
                        Err(connection::Refusal::Forgejo(_)) => {
                            self.finish(call, id, Err(api::Error::Refused), up);
                            break;
                        }
                        Err(connection::Refusal::GitHub(_)) => {
                            unreachable!("Forgejo call has a Forgejo encoder");
                        }
                    }
                    self.waiting.get_mut(id).expect("call").charged = false;
                }
            }
            c.pump(env, io, budget.1.saturating_sub(io.len().saturating_sub(below)).saturating_sub(1));
            let waiting = self.waiting.get_mut(id).expect("indexed call");
            if c.written && !waiting.charged {
                waiting.cost = waiting.cost.checked_add(1).expect("bounded exchanges");
                waiting.charged = true;
            }
            if let Some(heard) = c.heard() {
                let heard = call_heard(heard, waiting, &mut self.diagnostics);
                let (step, cut) = match waiting.plan.as_mut().expect("call plan") {
                    CallPlan::Forgejo(plan) => {
                        let step = plan.heard(
                            heard,
                            bounds(waiting.page_size, &env.limits, &self.shared),
                            &self.settings.word,
                            &mut self.forgejo_tables,
                        );
                        (step, plan.take_cut())
                    }
                    CallPlan::GitHub(_) => unreachable!("Forgejo heard"),
                };
                if cut {
                    c.cut(env);
                }
                match step {
                    Ok(Some(step)) => {
                        if self.call_step(id, step, env, up, io, call) {
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        c.close(true, io);
                        self.finish(call, id, Err(error), up);
                        break;
                    }
                }
                continue;
            }
            if c.phase == Phase::Closed {
                c.close(true, io);
            }
            if !c.ready() {
                break;
            }
            self.wake = Some(env.now);
        }
    }
    fn call_step(
        &mut self,
        id: Id<Waiting>,
        step: Step,
        env: &Env<Limits>,
        up: &mut Queue<Event>,
        io: &mut Queue<skein_io::Request>,
        call: Token,
    ) -> bool {
        match step {
            Step::Send(exchange) => {
                match self.waiting.get_mut(id).expect("indexed call").plan.as_mut().expect("call plan") {
                    CallPlan::Forgejo(plan) => plan.set_pending(exchange),
                    CallPlan::GitHub(_) => unreachable!("Forgejo step"),
                }
                let token = self.waiting.get(id).expect("call").connection.expect("holder");
                let c = self
                    .connection_slab
                    .get_mut(*self.connections.get(&token).expect("connection"))
                    .expect("connection");
                if c.phase == Phase::Closed {
                    c.close(false, io);
                    let _holder = self.holders.remove(&token);
                    self.waiting.get_mut(id).expect("call").connection = None;
                    self.wake = Some(env.now);
                    return true;
                }
                false
            }
            Step::Done(result) => {
                let token = self.waiting.get(id).expect("call").connection.expect("holder");
                let c = self
                    .connection_slab
                    .get_mut(*self.connections.get(&token).expect("connection"))
                    .expect("connection");
                if c.phase == Phase::Closed {
                    c.close(false, io);
                } else {
                    c.deadline = Some(env.now.saturating_add(env.limits.idle));
                }
                self.finish(call, id, result, up);
                true
            }
        }
    }
}

fn call_heard(heard: connection::Heard, waiting: &mut Waiting, diagnostics: &mut super::Diagnostics) -> Heard {
    let heard = match heard {
        connection::Heard::Forgejo(heard) => heard,
        connection::Heard::GitHub(_) => unreachable!("Forgejo call has a Forgejo connection"),
    };
    match &heard {
        Heard::Head { status, .. } => waiting.answered_head(*status, None),
        Heard::Done(_) | Heard::Row(_) | Heard::Failed(_) => {}
    }
    if skein_forge::forgejo::plans::reads::keep::long(&heard) {
        diagnostics.long_bodies = diagnostics.long_bodies.saturating_add(1);
    }
    let malformed = match &heard {
        Heard::Done(Body::Malformed(_)) => true,
        Heard::Done(Body::Document(_) | Body::Raw { .. } | Body::Empty)
        | Heard::Head { .. }
        | Heard::Row(_)
        | Heard::Failed(_) => false,
    };
    if malformed {
        diagnostics.malformed = diagnostics.malformed.saturating_add(1);
    }
    heard
}

fn bounds(page_size: u32, l: &Limits, s: &Shared) -> Bounds {
    Bounds { page_size, rows: s.rows, answer_bytes: s.answer_bytes, body_bytes: l.body_bytes }
}

pub(super) fn page_size(op: &api::Op, cap: u32, l: &Limits, s: &Shared) -> u32 {
    let listing = match op {
        api::Op::Read(read) => match read {
            api::Read::Items { .. } | api::Read::PullFor { .. } => Listing::Items,
            api::Read::Reviews { .. } => Listing::Reviews,
            api::Read::Statuses { .. } | api::Read::Checks { .. } => Listing::Statuses,
            api::Read::Remarks { .. } => Listing::Remarks,
            api::Read::Branches => Listing::Branches,
            api::Read::Collaborators { .. } => Listing::Collaborators,
            api::Read::PullFiles { .. } => Listing::PullFiles,
            api::Read::Compare { .. } => Listing::Compare,
            api::Read::Locate { .. }
            | api::Read::Item { .. }
            | api::Read::Pull { .. }
            | api::Read::Branch { .. }
            | api::Read::Protection { .. }
            | api::Read::Permission { .. }
            | api::Read::File { .. }
            | api::Read::Job { .. }
            | api::Read::Settings => return 1,
        },
        api::Op::Write(_) => return 1,
    };
    limits::page_size(listing, cap, l, s)
}

fn forgejo_plan(waiting: &mut Waiting) -> &mut Plan {
    match waiting.plan.as_mut().expect("call plan") {
        CallPlan::Forgejo(plan) => plan,
        CallPlan::GitHub(_) => unreachable!("Forgejo pump"),
    }
}
