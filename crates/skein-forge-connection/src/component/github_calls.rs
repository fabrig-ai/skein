//! Installation account routing and bounded repository discovery (forge-connection.md, 4).
use super::{CallPlan, Component, Waiting, calls};
use crate::{
    Event, Limits,
    connection::{self, Exchange, Phase},
    installations::Selected,
};
use skein_forge::{
    api,
    github::{
        self,
        plans::{
            Body, Heard,
            plan::{Bounds, Plan, Step},
            tables::Tables,
        },
        request::Auth,
    },
};
use skein_lib::{Env, Id, List, Queue, Token};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Route {
    selected: Selected,
    token_revision: u64,
    status: Option<u16>,
    pub(super) origin: Option<u32>,
}
/// One call's finite eligibility snapshot and private validated row.
#[derive(Debug)]
pub(crate) struct Discovery {
    candidates: List<Selected>,
    cursor: u32,
    skipped: Option<api::Error>,
    tables: Tables,
}
impl Discovery {
    fn new(gh: &crate::installations::GitHub) -> Self {
        let mut candidates = List::with_capacity(gh.rows.capacity());
        let mut recent = None;
        if let Some(id) = gh.recent
            && let Some(row) = gh.rows.get(&id)
            && eligible(row)
        {
            candidates.push(Selected { id, connect: gh.connect, revision: row.revision }).expect("row bound");
            recent = Some(id);
        }
        for (id, row) in &gh.rows {
            if Some(*id) != recent && eligible(row) {
                candidates.push(Selected { id: *id, connect: gh.connect, revision: row.revision }).expect("row bound");
            }
        }
        Self { candidates, cursor: 0, skipped: None, tables: Tables::new(1, 1) }
    }
    fn advance(&mut self) {
        self.cursor = self.cursor.checked_add(1).expect("bounded candidates");
    }
    fn select(&mut self, gh: &crate::installations::GitHub) -> Result<Selected, api::Error> {
        for _ in self.cursor..self.candidates.len() {
            let selected = *self.candidates.get(self.cursor).expect("candidate cursor");
            if selected.connect != gh.connect {
                return Err(api::Error::Forbidden);
            }
            if gh.valid(selected)
                && gh.rows.get(&selected.id).expect("valid row").state != crate::installations::Tokens::Gone
            {
                return Ok(selected);
            }
            self.skipped = Some(match gh.rows.get(&selected.id) {
                Some(row) if row.accepted && row.suspended => api::Error::Forbidden,
                Some(_) | None => api::Error::NotInstalled,
            });
            self.advance();
        }
        Err(self.skipped.unwrap_or(api::Error::Missing))
    }
}
fn eligible(row: &crate::installations::Row) -> bool {
    row.accepted && !row.suspended && row.state != crate::installations::Tokens::Gone
}
impl Component {
    #[expect(clippy::manual_map, reason = "the programming model excludes closures")]
    fn github_route(&mut self, id: Id<Waiting>, at: u32, env: &Env<Limits>) -> Result<bool, api::Error> {
        let waiting = self.waiting.get_mut(id).expect("call");
        let gh = self.github.get(at).expect("forge").as_ref().expect("GitHub");
        if let Some(discovery) = &mut waiting.discovery {
            if waiting.revoked {
                return Err(api::Error::Forbidden);
            }
            let selected = discovery.select(gh)?;
            if let Some(route) = waiting.route
                && (route.selected.id != selected.id || route.selected.revision != selected.revision)
            {
                waiting.plan = None;
                discovery.tables = Tables::new(1, 1);
            }
            let row = gh.rows.get(&selected.id).expect("eligible row");
            if waiting.route.is_none() || waiting.plan.is_none() {
                waiting.route =
                    Some(Route { selected, token_revision: row.token_revision, status: None, origin: None });
            }
            return Ok(gh.identity_checked && row.credential(env.now).is_some());
        }
        if let Some(route) = waiting.route {
            if waiting.revoked || !gh.valid(route.selected) {
                return Err(api::Error::Forbidden);
            }
            return Ok(route.origin.is_some()
                || (gh.identity_checked
                    && gh.rows.get(&route.selected.id).expect("eligible row").credential(env.now).is_some()));
        }
        let owner = match self.github_tables.repository(waiting.repository) {
            Some(row) => Some(row.owner_id),
            None => None,
        };
        let login = match waiting.op.as_ref() {
            Some(api::Op::Read(api::Read::Locate { owner, .. })) => Some(owner.as_ref()),
            Some(api::Op::Read(_) | api::Op::Write(_)) | None => None,
        };
        let mut chosen = None;
        if owner.is_some() || login.is_some() {
            for (installation, row) in &gh.rows {
                if row.accepted && (owner == Some(row.account.id) || login == Some(row.account.login.as_ref())) {
                    chosen = Some(*installation);
                    break;
                }
            }
        } else {
            if let Some(recent) = gh.recent
                && let Some(row) = gh.rows.get(&recent)
                && row.accepted
                && !row.suspended
                && row.state != crate::installations::Tokens::Gone
            {
                chosen = Some(recent);
            }
            if chosen.is_none() {
                for (installation, row) in &gh.rows {
                    if row.accepted && !row.suspended && row.state != crate::installations::Tokens::Gone {
                        chosen = Some(*installation);
                        break;
                    }
                }
            }
        }
        let installation = chosen.ok_or(api::Error::NotInstalled)?;
        let row = gh.rows.get(&installation).expect("chosen row");
        if row.suspended {
            return Err(api::Error::Forbidden);
        }
        waiting.route = Some(Route {
            selected: Selected { id: installation, connect: gh.connect, revision: row.revision },
            token_revision: row.token_revision,
            status: None,
            origin: None,
        });
        Ok(gh.identity_checked && row.credential(env.now).is_some())
    }
    pub(super) fn drive_github_call(
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
        let waiting = self.waiting.get(id).expect("call");
        if waiting.plan.is_none()
            && self.settings.forges.get(at).expect("forge").log_origins.is_empty()
            && let Some(api::Op::Read(api::Read::Job { .. })) = waiting.op.as_ref()
        {
            self.finish(call, id, Err(api::Error::Refused), up);
            return;
        }
        if waiting.plan.is_none() && !admitted(waiting.op.as_ref().expect("operation")) {
            self.finish(call, id, Err(api::Error::Refused), up);
            return;
        }
        if waiting.plan.is_none() && waiting.discovery.is_none() && waiting.discovered.is_none() && waiting.cost == 0 {
            let missing =
                waiting.repository.repository != 0 && self.github_tables.repository(waiting.repository).is_none();
            let waiting = self.waiting.get_mut(id).expect("call");
            waiting.route = None;
            if missing {
                let discovery = Discovery::new(self.github.get(at).expect("forge").as_ref().expect("GitHub"));
                if discovery.candidates.is_empty() {
                    self.finish(call, id, Err(api::Error::NotInstalled), up);
                    return;
                }
                waiting.discovery = Some(discovery);
            }
        }
        if self.waiting.get(id).expect("call").connection.is_none() {
            match self.github_route(id, at, env) {
                Ok(true) => {}
                Ok(false) => return,
                Err(error) => {
                    self.finish(call, id, Err(error), up);
                    return;
                }
            }
        }
        if let Some(answer) = self.waiting.get_mut(id).expect("call").discovered.take() {
            self.finish(call, id, Ok(answer), up);
            return;
        }
        if let Some(answer) = self.github_bot_permission(id, at) {
            self.finish(call, id, Ok(answer), up);
            return;
        }
        if !self.assign(call, id, at, env, up, io) {
            return;
        }
        self.pump_github_call(
            holder,
            at,
            env,
            up,
            io,
            (
                budget.0.saturating_sub(up.len().saturating_sub(above)),
                budget.1.saturating_sub(io.len().saturating_sub(below)),
            ),
        );
    }
    fn github_bot_permission(&self, id: Id<Waiting>, at: u32) -> Option<api::Answer> {
        let waiting = self.waiting.get(id).expect("call");
        let gh = self.github.get(at).expect("forge").as_ref().expect("GitHub");
        let bot = self.settings.forges.get(at).expect("forge").access.as_ref()?.user;
        if waiting.discovery.is_none()
            && let Some(api::Op::Read(api::Read::Permission { user })) = waiting.op.as_ref()
            && *user == bot
        {
            let route = waiting.route.expect("eligible route");
            let row = gh.rows.get(&route.selected.id).expect("eligible row");
            return Some(api::Answer::Permission(row.permissions.permission()));
        }
        None
    }
    pub(super) fn begin_github_call(
        &mut self,
        call: Token,
        id: Id<Waiting>,
        env: &Env<Limits>,
        up: &mut Queue<Event>,
        io: &mut Queue<skein_io::Request>,
    ) -> bool {
        let step = self.initialize_github_plan(id, env);
        match step {
            Ok(step) => !self.github_call_step((call, id), step, env, up, io, None),
            Err(error) => {
                self.finish(call, id, Err(error), up);
                false
            }
        }
    }
    fn initialize_github_plan(&mut self, id: Id<Waiting>, env: &Env<Limits>) -> Result<Step, api::Error> {
        let waiting = self.waiting.get_mut(id).expect("call");
        let op = if waiting.discovery.is_some() {
            api::Op::Read(api::Read::Settings)
        } else {
            waiting.op.take().expect("operation")
        };
        waiting.page_size =
            calls::page_size(&op, crate::settings::Kind::GitHub, github::pin::PAGE_CAP, &env.limits, &self.shared);
        let mut plan = Plan::new(waiting.repository, op);
        let tables = match &mut waiting.discovery {
            Some(discovery) => &mut discovery.tables,
            None => &mut self.github_tables,
        };
        let step = plan.begin(bounds(waiting.page_size, env, &self.shared), &self.settings.word, tables);
        waiting.plan = Some(CallPlan::GitHub(plan));
        step
    }
    fn pump_github_call(
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
        let connection = *self.connections.get(&token).expect("connection");
        for _ in 0..64_u32 {
            if up.len().saturating_sub(above).saturating_add(1) >= budget.0
                || io.len().saturating_sub(below).saturating_add(1) >= budget.1
            {
                self.wake = Some(env.now);
                break;
            }
            if self.connection_slab.get(connection).expect("connection").phase == Phase::Idle {
                match self.github_route(id, at, env) {
                    Ok(true) => {}
                    Ok(false) => {
                        self.release_github_call(id, env, io);
                        break;
                    }
                    Err(error) => {
                        self.release_github_call(id, env, io);
                        self.finish(call, id, Err(error), up);
                        break;
                    }
                }
                if self.waiting.get(id).expect("call").plan.is_none() && !self.begin_github_call(call, id, env, up, io)
                {
                    break;
                }
                if let Err(error) = self.send_github_call(id, at, env) {
                    self.finish(call, id, Err(error), up);
                    break;
                }
            }

            let c = self.connection_slab.get_mut(connection).expect("connection");
            c.pump(env, io, budget.1.saturating_sub(io.len().saturating_sub(below)).saturating_sub(1));
            let waiting = self.waiting.get_mut(id).expect("call");
            if c.written && !waiting.charged {
                waiting.cost = waiting.cost.checked_add(1).expect("bounded exchanges");
                waiting.charged = true;
                self.github.get_mut(at).expect("forge").as_mut().expect("GitHub").recent =
                    Some(waiting.route.expect("sent route").selected.id);
            }
            if let Some(heard) = c.heard() {
                let (step, valid) = self.hear_github_call(id, at, env, heard);
                match step {
                    Ok(Some(step)) => {
                        if self.github_call_step(holder, step, env, up, io, Some((at, valid))) {
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        self.connection_slab.get_mut(connection).expect("connection").close(true, io);
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
    fn send_github_call(&mut self, id: Id<Waiting>, at: u32, env: &Env<Limits>) -> Result<(), api::Error> {
        let waiting = self.waiting.get_mut(id).expect("call");
        let Some(exchange) = github_plan(waiting).pending() else {
            return Ok(());
        };
        let encoded = github::request::encode(&exchange.request, &crate::limits::github_documents(&env.limits));
        match encoded {
            Ok(encoded) => match encoded.auth {
                Auth::Installation | Auth::None => {}
                Auth::App => unreachable!("ordinary GitHub plans use Installation or no credential"),
            },
            Err(_) => return Err(api::Error::Refused),
        }
        let route = waiting.route.as_mut().expect("eligible route");
        let gh = self.github.get(at).expect("forge").as_ref().expect("GitHub");
        let row = gh.rows.get(&route.selected.id).expect("eligible row");
        let authorization = if route.origin.is_some() {
            None
        } else {
            Some(github::credential::bearer(row.credential(env.now).expect("usable Full")).expect("validated token"))
        };
        route.token_revision = row.token_revision;
        route.status = None;
        let token = waiting.connection.expect("holder");
        let c = self.connection_slab.get_mut(*self.connections.get(&token).expect("connection")).expect("connection");
        c.deadline = Some(waiting.deadline);
        match c.start(
            env,
            self.settings.forges.get(at).expect("forge"),
            &self.settings.user_agent,
            Exchange::GitHub { exchange, authorization },
        ) {
            Ok(()) => {}
            Err(connection::Refusal::GitHub(_)) => return Err(api::Error::Refused),
            Err(connection::Refusal::Forgejo(_)) => unreachable!("GitHub encoder"),
        }
        waiting.charged = false;
        Ok(())
    }
    fn hear_github_call(
        &mut self,
        id: Id<Waiting>,
        at: u32,
        env: &Env<Limits>,
        heard: connection::Heard,
    ) -> (Result<Option<Step>, api::Error>, bool) {
        let waiting = self.waiting.get_mut(id).expect("call");
        let heard = match heard {
            connection::Heard::GitHub(heard) => heard,
            connection::Heard::Forgejo(_) => unreachable!("GitHub connection"),
        };
        let refused = match &heard {
            Heard::Head { status, .. } => *status == 401,
            Heard::Done(_) | Heard::Row(_) | Heard::Failed(_) => false,
        };
        match &heard {
            Heard::Head { status, head } => {
                waiting.answered_head(*status, github::header::rate(head));
                waiting.route.as_mut().expect("sent route").status = Some(*status);
            }
            Heard::Done(Body::Malformed(_)) => {
                self.diagnostics.malformed = self.diagnostics.malformed.saturating_add(1);
            }
            Heard::Done(Body::Document(_) | Body::Raw { .. } | Body::Empty) | Heard::Row(_) | Heard::Failed(_) => {}
        }
        let route = waiting.route.expect("sent route");
        let valid =
            !waiting.revoked && self.github.get(at).expect("forge").as_ref().expect("GitHub").valid(route.selected);
        let mut discarded = Tables::new(0, 0);
        let tables = if valid {
            match &mut waiting.discovery {
                Some(discovery) => &mut discovery.tables,
                None => &mut self.github_tables,
            }
        } else {
            &mut discarded
        };
        let page_size = waiting.page_size;
        let plan = match waiting.plan.as_mut().expect("plan") {
            CallPlan::GitHub(plan) => plan,
            CallPlan::Forgejo(_) => unreachable!("GitHub plan"),
        };
        let step = plan.heard(heard, bounds(page_size, env, &self.shared), &self.settings.word, tables);
        let step =
            if refused && route.origin.is_none() { Ok(Some(Step::Done(Err(api::Error::Forbidden)))) } else { step };
        if refused && route.origin.is_none() && valid {
            self.github_token_refused(at, route.selected.id, route.token_revision, env);
        }
        (step, valid)
    }
    pub(super) fn release_github_call(
        &mut self,
        id: Id<Waiting>,
        env: &Env<Limits>,
        io: &mut Queue<skein_io::Request>,
    ) {
        let waiting = self.waiting.get_mut(id).expect("call");
        if let Some(token) = waiting.connection.take() {
            let c =
                self.connection_slab.get_mut(*self.connections.get(&token).expect("connection")).expect("connection");
            if c.phase == Phase::Idle && !c.in_flight() {
                c.deadline = Some(env.now.saturating_add(env.limits.idle));
            } else if !c.draining {
                c.close(c.in_flight(), io);
            }
            let _holder = self.holders.remove(&token);
        }
    }
    fn github_call_step(
        &mut self,
        holder: (Token, Id<Waiting>),
        step: Step,
        env: &Env<Limits>,
        up: &mut Queue<Event>,
        io: &mut Queue<skein_io::Request>,
        checked: Option<(u32, bool)>,
    ) -> bool {
        let (call, id) = holder;
        match step {
            Step::Send(exchange) => {
                github_plan(self.waiting.get_mut(id).expect("call")).set_pending(exchange);
                let token = self.waiting.get(id).expect("call").connection.expect("holder");
                let c = self
                    .connection_slab
                    .get_mut(*self.connections.get(&token).expect("connection"))
                    .expect("connection");
                if c.phase == Phase::Closed || (checked.is_some() && c.in_flight()) {
                    self.release_github_call(id, env, io);
                    self.wake = Some(env.now);
                    return true;
                }
                false
            }
            Step::Redirect { redirect, retention } => {
                self.follow_github_log(holder, redirect, retention, env, up, io);
                true
            }
            Step::Done(result) => {
                if self.waiting.get(id).expect("call").discovery.is_some() {
                    self.github_discovered(holder, result, env, up, io, checked.expect("discovery response"));
                    return true;
                }
                self.release_github_call(id, env, io);
                self.finish(call, id, result, up);
                true
            }
        }
    }
    fn github_discovered(
        &mut self,
        holder: (Token, Id<Waiting>),
        result: Result<api::Answer, api::Error>,
        env: &Env<Limits>,
        up: &mut Queue<Event>,
        io: &mut Queue<skein_io::Request>,
        checked: (u32, bool),
    ) {
        let (call, id) = holder;
        let (at, valid) = checked;
        let result = if valid { result } else { Err(api::Error::Forbidden) };
        match result {
            Ok(answer) => {
                let waiting = self.waiting.get_mut(id).expect("call");
                let mut staged = waiting.discovery.take().expect("discovery table").tables;
                let row = staged.repository(waiting.repository).expect("validated Settings row").clone();
                self.github_tables.keep_user(waiting.repository.forge, row.owner_id, row.owner.clone());
                self.github_tables.keep_repository(waiting.repository, row);
                waiting.route = None;
                let usable = match self.github_route(id, at, env) {
                    Ok(usable) => usable,
                    Err(error) => {
                        self.release_github_call(id, env, io);
                        self.finish(call, id, Err(error), up);
                        return;
                    }
                };
                let waiting = self.waiting.get_mut(id).expect("call");
                waiting.plan = None;
                if waiting.op.as_ref() == Some(&api::Op::Read(api::Read::Settings)) {
                    self.release_github_call(id, env, io);
                    if usable {
                        self.finish(call, id, Ok(answer), up);
                    } else {
                        self.waiting.get_mut(id).expect("call").discovered = Some(answer);
                    }
                    return;
                }
                if let Some(answer) = self.github_bot_permission(id, at) {
                    self.release_github_call(id, env, io);
                    if usable {
                        self.finish(call, id, Ok(answer), up);
                    } else {
                        self.wake = Some(env.now);
                    }
                    return;
                }
                match self.initialize_github_plan(id, env) {
                    Ok(Step::Send(exchange)) => {
                        github_plan(self.waiting.get_mut(id).expect("call")).set_pending(exchange);
                    }
                    Ok(Step::Redirect { redirect, retention }) => {
                        self.follow_github_log(holder, redirect, retention, env, up, io);
                        return;
                    }
                    Ok(Step::Done(result)) => {
                        self.release_github_call(id, env, io);
                        self.finish(call, id, result, up);
                        return;
                    }
                    Err(error) => {
                        self.release_github_call(id, env, io);
                        self.finish(call, id, Err(error), up);
                        return;
                    }
                }
                self.release_github_call(id, env, io);
                self.wake = Some(env.now);
            }
            Err(error) => {
                let waiting = self.waiting.get_mut(id).expect("call");
                if valid && error == api::Error::Missing && waiting.route.expect("probe route").status == Some(404) {
                    let discovery = waiting.discovery.as_mut().expect("discovery");
                    discovery.advance();
                    discovery.tables = Tables::new(1, 1);
                    waiting.plan = None;
                    waiting.route = None;
                    self.release_github_call(id, env, io);
                    self.wake = Some(env.now);
                    return;
                }
                self.release_github_call(id, env, io);
                self.finish(call, id, Err(error), up);
            }
        }
    }
}
pub(super) fn github_plan(waiting: &mut Waiting) -> &mut Plan {
    match waiting.plan.as_mut().expect("plan") {
        CallPlan::GitHub(plan) => plan,
        CallPlan::Forgejo(_) => unreachable!("GitHub plan"),
    }
}
fn bounds(page_size: u32, env: &Env<Limits>, shared: &crate::Shared) -> Bounds {
    Bounds {
        inner: env.limits.inner,
        page_size,
        rows: shared.rows,
        answer_bytes: shared.answer_bytes,
        body_bytes: env.limits.body_bytes,
    }
}
fn admitted(op: &api::Op) -> bool {
    match op {
        api::Op::Read(read) => match read {
            api::Read::Protection { .. }
            | api::Read::Runs { .. }
            | api::Read::Run { .. }
            | api::Read::Tree { .. }
            | api::Read::Job { .. }
            | api::Read::File { .. }
            | api::Read::Compare { .. }
            | api::Read::PullFiles { .. }
            | api::Read::Locate { .. }
            | api::Read::Settings
            | api::Read::Branch { .. }
            | api::Read::Branches { .. }
            | api::Read::Items { .. }
            | api::Read::Item { .. }
            | api::Read::Pull { .. }
            | api::Read::PullFor { .. }
            | api::Read::Statuses { .. }
            | api::Read::Checks { .. }
            | api::Read::Permission { .. }
            | api::Read::Collaborators { .. }
            | api::Read::Remarks { .. }
            | api::Read::Reviews { .. } => true,
        },
        api::Op::Write(write) => match write {
            api::Write::CreateIssue { labels, .. } => labels.is_empty(),
            api::Write::Labels { .. } | api::Write::CreateLabel { .. } => false,
            api::Write::SetReviewers { .. }
            | api::Write::Merge { .. }
            | api::Write::Dispatch { .. }
            | api::Write::Rerun { .. }
            | api::Write::DeleteBranch { .. }
            | api::Write::Close { .. }
            | api::Write::Reopen { .. }
            | api::Write::Edit { .. }
            | api::Write::Status { .. }
            | api::Write::CreateBranch { .. }
            | api::Write::OpenPull { .. }
            | api::Write::Post { .. }
            | api::Write::Review { .. }
            | api::Write::Update { .. } => true,
        },
    }
}
