//! The browser and its operations, each advanced by one reply or event.

#![expect(
    clippy::disallowed_types,
    reason = "bounded entity and command tables reuse Vec slots; capacities are fixed at construction"
)]
#![expect(clippy::manual_find, clippy::manual_map, reason = "the step subset excludes closure-based combinators")]

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

use skein_lib::stream::{self, Delimiter, Read};
use skein_lib::{Env, List, Queue, Time, Token};

use crate::boundary::{Below, Down, Event, Expect, Go, Key, Match, Point, Query, Rect, Refusal, Seen, States, Trouble};
use crate::limits::Limits;
use crate::params::Params;
use crate::wire::decode::{Document, Value};
use crate::wire::encode;
use crate::wire::frame::Framer;

/// Each step emits at most one event and two stream requests. Further
/// terminals are retained for the next call to `fire`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaxOut {
    pub above: u32,
    pub below: u32,
}

pub const UP_MAX_OUT: MaxOut = MaxOut { above: 1, below: 3 };
pub const DOWN_MAX_OUT: MaxOut = MaxOut { above: 1, below: 2 };
pub const FIRE_MAX_OUT: MaxOut = MaxOut { above: 1, below: 3 };
/// CDP's modifier bit for Shift.
const SHIFT: u64 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Starting,
    Ready,
    Closing,
    Closed,
}

#[derive(Debug)]
struct Person {
    token: Token,
    context: Option<Box<[u8]>>,
    downloads: Option<Box<[u8]>>,
    ready: bool,
    closing: bool,
    closed: bool,
}

#[derive(Debug)]
struct Download {
    guid: Box<[u8]>,
    person: Token,
    page: Token,
    name: Box<[u8]>,
}

#[derive(Debug)]
struct Page {
    token: Token,
    person: Token,
    target: Option<Box<[u8]>>,
    session: Option<Box<[u8]>>,
    initial_url: Box<[u8]>,
    frame: Option<Box<[u8]>>,
    navigating: bool,
    opening: bool,
    load: Option<Load>,
    load_due: Option<Time>,
    closed: bool,
    css: look::Css,
    // The loader id whose inspector sheet draws the caret steady (browser.md, 5.1).
    caret_document: Option<Box<[u8]>>,
}

#[derive(Clone, Copy, Debug)]
enum Load {
    Opening,
    Go(Token),
}

#[derive(Debug)]
struct Operation {
    token: Token,
    page: Token,
    kind: OpKind,
    due: Option<Time>,
    warned: bool,
}

#[derive(Clone, Copy, Debug)]
enum Entry {
    Insert,
    Compose,
    Keys,
}

#[derive(Debug)]
enum OpKind {
    Go(Go),
    Find { query: Query, expect: Option<(Expect, Time)>, seen: List<Seen>, more: u32, next: u32 },
    Press { node: u64, point: Option<Point>, at: Option<(i64, i64)>, hit: Option<u64> },
    Type { text: Box<[u8]>, entry: Entry },
    Key { key: Key, shift: bool },
    Address,
    Attribute { selector: Box<[u8]>, name: Box<[u8]>, until: Time },
    Look { look: crate::boundary::Look, state: look::State },
    Snapshot,
    Screenshot,
}

#[derive(Debug)]
struct Queued {
    read: Option<Token>,
    person: Option<Token>,
    id: u64,
    bytes: Box<[u8]>,
}

#[derive(Debug)]
struct Pending {
    id: u64,
    due: Time,
    kind: PendingKind,
}

#[derive(Clone, Copy, Debug)]
enum PendingKind {
    Version,
    Context(Token),
    Downloads(Token),
    CancelPerson(Token),
    CancelBrowser,
    Target(Token),
    Attach(Token),
    Enable(Token, u8),
    Viewport(Token),
    Navigate(Token),
    Step(Token, Step),
    CloseBrowser,
    Release,
    Dialog,
    ClosePage(Token),
    ClosePerson(Token),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    History,
    Go,
    Query,
    Box,
    Scroll,
    Accessibility,
    Quads,
    Metrics,
    Hit,
    Describe,
    MouseMove,
    MouseDown,
    MouseUp,
    TypeNode { node: u64 },
    Focus,
    Insert,
    KeyDown,
    KeyUp,
    Character { offset: usize, down: bool },
    Address,
    AttributeRoot,
    AttributeSelector,
    AttributeValue,
    Look(look::Step),
    Snapshot,
    Screenshot,
}

/// A Chromium pipe peer. The owner drives `fire` at `next_deadline` and
/// whenever `work_pending` is true, and settles the child after `Closed`.
#[expect(clippy::struct_excessive_bools, reason = "each pipe demand and room grant is independent")]
#[derive(Debug)]
pub struct Browser {
    owner: Token,
    phase: Phase,
    persons: Vec<Option<Person>>,
    pages: Vec<Option<Page>>,
    ops: Vec<Option<Operation>>,
    pending: Vec<Pending>,
    downloads: Vec<Download>,
    next_id: u64,
    framing: Framer,
    outgoing: VecDeque<Queued>,
    events: VecDeque<Event>,
    room: bool,
    command_demanded: bool,
    replies_demanded: bool,
    errors_demanded: bool,
    stderr: Vec<u8>,
}

impl Browser {
    /// The browser token is also the owner of its terminal `Closed` event.
    #[must_use]
    pub fn new(owner: Token, limits: &Limits) -> Browser {
        assert!(crate::limits::valid(limits), "browser limits must be representable");
        Browser {
            owner,
            phase: Phase::Starting,
            persons: Vec::with_capacity(usize::try_from(limits.persons).expect("u32 fits usize")),
            pages: Vec::with_capacity(usize::try_from(limits.pages).expect("u32 fits usize")),
            ops: Vec::with_capacity(usize::try_from(limits.ops).expect("u32 fits usize")),
            pending: Vec::with_capacity(usize::try_from(limits.commands).expect("u32 fits usize")),
            downloads: Vec::with_capacity(usize::try_from(limits.downloads).expect("u32 fits usize")),
            next_id: 1,
            framing: Framer::new(limits.message),
            outgoing: VecDeque::with_capacity(usize::try_from(limits.commands).expect("u32 fits usize")),
            events: VecDeque::with_capacity(
                usize::try_from(
                    limits
                        .persons
                        .saturating_add(limits.pages)
                        .saturating_add(limits.ops)
                        .saturating_add(limits.downloads)
                        .saturating_add(4),
                )
                .expect("u32 fits usize"),
            ),
            room: false,
            command_demanded: false,
            replies_demanded: false,
            errors_demanded: false,
            stderr: Vec::with_capacity(usize::try_from(limits.stderr).expect("u32 fits usize")),
        }
    }

    #[must_use]
    pub fn work_pending(&self) -> bool {
        !self.events.is_empty()
            || (self.phase == Phase::Starting && self.pending.is_empty())
            || (self.room && self.sendable().is_some())
    }

    #[must_use]
    pub fn next_deadline(&self) -> Option<Time> {
        if self.work_pending() {
            return Some(Time::ZERO);
        }
        let mut next: Option<Time> = None;
        for pending in &self.pending {
            next = Some(match next {
                Some(at) => at.min(pending.due),
                None => pending.due,
            });
        }
        for op in &self.ops {
            let at = match op {
                Some(op) => op.due,
                None => None,
            };
            if let Some(at) = at {
                next = Some(match next {
                    Some(old) => old.min(at),
                    None => at,
                });
            }
        }
        for page in self.pages.iter().flatten() {
            if let Some(at) = page.load_due {
                next = Some(match next {
                    Some(old) => old.min(at),
                    None => at,
                });
            }
        }
        next
    }

    #[must_use]
    pub fn stderr_tail(&self) -> &[u8] {
        &self.stderr
    }

    pub fn reclaim(&mut self) {
        for person in &mut self.persons {
            if let Some(state) = person
                && state.closed
            {
                *person = None;
            }
        }
        for page in &mut self.pages {
            if let Some(state) = page
                && state.closed
            {
                *page = None;
            }
        }
    }

    fn person(&self, token: Token) -> Option<&Person> {
        for person in self.persons.iter().flatten() {
            if person.token == token && !person.closed {
                return Some(person);
            }
        }
        None
    }
    fn person_mut(&mut self, token: Token) -> Option<&mut Person> {
        for person in self.persons.iter_mut().flatten() {
            if person.token == token && !person.closed {
                return Some(person);
            }
        }
        None
    }
    fn page(&self, token: Token) -> Option<&Page> {
        for page in self.pages.iter().flatten() {
            if page.token == token && !page.closed {
                return Some(page);
            }
        }
        None
    }
    fn page_mut(&mut self, token: Token) -> Option<&mut Page> {
        for page in self.pages.iter_mut().flatten() {
            if page.token == token && !page.closed {
                return Some(page);
            }
        }
        None
    }
    fn op(&self, token: Token) -> Option<&Operation> {
        for op in self.ops.iter().flatten() {
            if op.token == token {
                return Some(op);
            }
        }
        None
    }
    fn op_mut(&mut self, token: Token) -> Option<&mut Operation> {
        for op in self.ops.iter_mut().flatten() {
            if op.token == token {
                return Some(op);
            }
        }
        None
    }
    fn remove_op(&mut self, token: Token) -> Option<Operation> {
        for op in &mut self.ops {
            if let Some(candidate) = op
                && candidate.token == token
            {
                return op.take();
            }
        }
        None
    }
    fn insert_person(&mut self, person: Person, limit: u32) -> bool {
        for slot in &mut self.persons {
            if match slot {
                Some(state) => state.closed,
                None => true,
            } {
                *slot = Some(person);
                return true;
            }
        }
        if self.persons.len() >= usize::try_from(limit).expect("u32 fits usize") {
            return false;
        }
        self.persons.push(Some(person));
        true
    }
    fn insert_page(&mut self, page: Page, limit: u32) -> bool {
        for slot in &mut self.pages {
            if match slot {
                Some(state) => state.closed,
                None => true,
            } {
                *slot = Some(page);
                return true;
            }
        }
        if self.pages.len() >= usize::try_from(limit).expect("u32 fits usize") {
            return false;
        }
        self.pages.push(Some(page));
        true
    }
    fn insert_op(&mut self, op: Operation, limit: u32) -> bool {
        for slot in &mut self.ops {
            if slot.is_none() {
                *slot = Some(op);
                return true;
            }
        }
        if self.ops.len() >= usize::try_from(limit).expect("u32 fits usize") {
            return false;
        }
        self.ops.push(Some(op));
        true
    }

    fn queue_command(
        &mut self,
        env: &Env<Limits>,
        method: &[u8],
        session: Option<&[u8]>,
        args: &Params<'_>,
        kind: PendingKind,
    ) -> bool {
        if self.pending.len() >= usize::try_from(env.limits.commands).expect("u32 fits usize") {
            return false;
        }
        let Some(params) = args.encode(env.limits.command) else {
            return false;
        };
        let Some(id) = self.next_id.checked_add(1) else {
            return false;
        };
        let Ok(bytes) = encode::command(self.next_id, method, session, &params, env.limits.command) else {
            return false;
        };
        self.pending.push(Pending { id: self.next_id, due: env.now.saturating_add(env.limits.answer), kind });
        let person = if let PendingKind::Target(page) = kind {
            match self.page(page) {
                Some(page) => Some(page.person),
                None => None,
            }
        } else {
            None
        };
        let command = Queued { read: None, person, id: self.next_id, bytes };
        self.next_id = id;
        self.outgoing.push_back(command);
        true
    }

    fn queue_page(
        &mut self,
        env: &Env<Limits>,
        page: Token,
        method: &[u8],
        args: &Params<'_>,
        kind: PendingKind,
    ) -> bool {
        let session = match self.page(page) {
            Some(page) => page.session.clone(),
            None => None,
        };
        let queued = self.queue_command(env, method, session.as_deref(), args, kind);
        let address = if let PendingKind::Step(_, step) = kind { step == Step::Address } else { false };
        if queued
            && (method.starts_with(b"Accessibility.")
                || method.starts_with(b"DOM.")
                // The shot's CSS setup can run during navigation; CSS reads wait.
                || method.starts_with(b"CSS.get")
                || method == b"Runtime.evaluate"
                || address)
        {
            if let Some(command) = self.outgoing.back_mut() {
                command.read = Some(page);
            }
            if let PendingKind::Step(op, _) = kind
                && let Some(Operation { kind: OpKind::Find { expect: Some((_, until)), .. }, .. }) = self.op(op)
            {
                let until = *until;
                if let Some(pending) = self.pending.last_mut() {
                    pending.due = pending.due.min(until);
                }
            }
        }
        queued
    }

    fn sendable(&self) -> Option<usize> {
        for (index, command) in self.outgoing.iter().enumerate() {
            if let Some(person) = command.person {
                let waiting = match self.person(person) {
                    Some(person) => !person.ready || person.closing,
                    None => true,
                };
                if waiting {
                    continue;
                }
            }
            if let Some(page) = command.read
                && let Some(page) = self.page(page)
                && page.navigating
            {
                continue;
            }
            return Some(index);
        }
        None
    }

    fn flush(&mut self, below: &mut Queue<Down>, limit: u32) {
        if self.room
            && let Some(index) = self.sendable()
            && let Some(command) = self.outgoing.remove(index)
        {
            below.push(Down::Commands(stream::Down::Send(command.bytes)));
            self.room = false;
            below.push(Down::Commands(stream::Down::Demand { read: Read::Nothing, room: limit.saturating_add(1) }));
            self.command_demanded = true;
        }
    }

    fn emit(&mut self, above: &mut Queue<Event>) {
        if let Some(event) = self.events.pop_front() {
            above.push(event);
        }
    }

    fn start(&mut self, env: &Env<Limits>, below: &mut Queue<Down>) {
        if self.phase != Phase::Starting || !self.pending.is_empty() {
            return;
        }
        let sent = self.queue_command(env, b"Browser.getVersion", None, &Params::Empty, PendingKind::Version);
        assert!(sent, "version command fits startup limits");
        if !self.command_demanded {
            below.push(Down::Commands(stream::Down::Demand {
                read: Read::Nothing,
                room: env.limits.command.saturating_add(1),
            }));
            self.command_demanded = true;
        }
        if !self.replies_demanded {
            let nul = Delimiter::new(b"\0").expect("NUL is a delimiter");
            below.push(Down::Replies(stream::Down::Demand {
                read: Read::Scan { until: nul, max: env.limits.message.saturating_add(1) },
                room: 0,
            }));
            self.replies_demanded = true;
        }
        if !self.errors_demanded {
            below.push(Down::Errors(stream::Down::Demand {
                read: Read::Line { max: env.limits.stderr.max(1) },
                room: 0,
            }));
            self.errors_demanded = true;
        }
    }

    #[expect(clippy::too_many_lines, reason = "one match dispatches each public browser request")]
    fn request(&mut self, env: &Env<Limits>, request: crate::boundary::Request) {
        use crate::boundary::Request;
        match request {
            Request::Person { person, downloads } => {
                if self.phase != Phase::Ready || self.person(person).is_some() {
                    self.events.push_back(Event::Closed { owner: person });
                    return;
                }
                if let Some(directory) = &downloads
                    && (!directory.starts_with(b"/")
                        || directory.contains(&0)
                        || directory.len() > usize::try_from(env.limits.command).expect("u32 fits usize"))
                {
                    self.events.push_back(Event::Closed { owner: person });
                    return;
                }
                let ok = self.insert_person(
                    Person { token: person, context: None, downloads, ready: false, closing: false, closed: false },
                    env.limits.persons,
                );
                if !ok
                    || !self.queue_command(
                        env,
                        b"Target.createBrowserContext",
                        None,
                        &Params::Empty,
                        PendingKind::Context(person),
                    )
                {
                    self.close_person_local(person);
                }
            }
            Request::Page { person, page, url } => {
                let context = match self.person(person) {
                    Some(person) if !person.closing => person.context.clone(),
                    Some(_) | None => None,
                };
                let Some(context) = context else {
                    self.events.push_back(Event::Closed { owner: page });
                    return;
                };
                if self.page(page).is_some() {
                    self.events.push_back(Event::Closed { owner: page });
                    return;
                }
                let added = self.insert_page(
                    Page {
                        token: page,
                        person,
                        target: None,
                        session: None,
                        initial_url: url,
                        frame: None,
                        navigating: false,
                        opening: true,
                        css: look::Css::Disabled,
                        caret_document: None,
                        load: None,
                        load_due: None,
                        closed: false,
                    },
                    env.limits.pages,
                );
                if !added
                    || !self.queue_command(
                        env,
                        b"Target.createTarget",
                        None,
                        &Params::Target { url: b"about:blank", context: &context },
                        PendingKind::Target(page),
                    )
                {
                    self.close_page_local(page);
                }
            }
            Request::Go { page, op, to } => {
                if !self.admit_op(env, page, op, OpKind::Go(to)) {
                    return;
                }
                self.start_go(env, op);
            }
            Request::Find { page, op, query } => {
                let seen = List::with_capacity(env.limits.matches);
                if !self.admit_op(env, page, op, OpKind::Find { query, expect: None, seen, more: 0, next: 0 }) {
                    return;
                }
                self.start_find(env, op);
            }
            Request::Await { page, op, query, expect, within } => {
                let seen = List::with_capacity(env.limits.matches);
                let until = env.now.saturating_add(within);
                if !self.admit_op(
                    env,
                    page,
                    op,
                    OpKind::Find { query, expect: Some((expect, until)), seen, more: 0, next: 0 },
                ) {
                    return;
                }
                self.start_find(env, op);
            }
            Request::Press { page, op, node, at } => {
                if !self.admit_op(env, page, op, OpKind::Press { node, point: at, at: None, hit: None }) {
                    return;
                }
                self.issue_step(env, op, b"DOM.scrollIntoViewIfNeeded", &Params::Backend(node), Step::Scroll);
            }
            Request::Type { page, op, node, text } => {
                let inspect = text.contains(&b'\r') || text.contains(&b'\n');
                if !self.admit_op(env, page, op, OpKind::Type { text, entry: Entry::Insert }) {
                    return;
                }
                if inspect {
                    self.issue_step(env, op, b"DOM.describeNode", &Params::Backend(node), Step::TypeNode { node });
                } else {
                    self.issue_step(env, op, b"DOM.focus", &Params::Backend(node), Step::Focus);
                }
            }
            Request::Compose { page, op, node, text } => {
                if !self.admit_op(env, page, op, OpKind::Type { text, entry: Entry::Compose }) {
                    return;
                }
                self.issue_step(env, op, b"DOM.focus", &Params::Backend(node), Step::Focus);
            }
            Request::Keys { page, op, node, text } => {
                if !self.admit_op(env, page, op, OpKind::Type { text, entry: Entry::Keys }) {
                    return;
                }
                self.issue_step(env, op, b"DOM.focus", &Params::Backend(node), Step::Focus);
            }
            Request::Key { page, op, key, shift } => {
                if let Key::Letter { letter, .. } = key
                    && !letter.is_ascii_lowercase()
                {
                    self.events.push_back(Event::Refused { op, why: Refusal::Protocol });
                    return;
                }
                if !self.admit_op(env, page, op, OpKind::Key { key, shift }) {
                    return;
                }
                self.issue_key(env, op, key, shift, true);
            }
            Request::Address { page, op } => {
                if !self.admit_op(env, page, op, OpKind::Address) {
                    return;
                }
                self.issue_step(env, op, b"Page.getNavigationHistory", &Params::Empty, Step::Address);
            }
            Request::Attribute { page, op, selector, name } => {
                let limit = usize::try_from(env.limits.text).expect("text fits usize");
                if selector.len() > limit || name.len() > limit {
                    self.events.push_back(Event::Refused { op, why: Refusal::Limit });
                    return;
                }
                let until = env.now.saturating_add(env.limits.answer);
                if self.admit_op(env, page, op, OpKind::Attribute { selector, name, until }) {
                    self.start_attribute(env, op);
                }
            }
            Request::Snapshot { page, op } => {
                if !self.admit_op(env, page, op, OpKind::Snapshot) {
                    return;
                }
                self.issue_step(env, op, b"Accessibility.getFullAXTree", &Params::Empty, Step::Snapshot);
            }
            Request::Screenshot { page, op } => {
                if !self.admit_op(env, page, op, OpKind::Screenshot) {
                    return;
                }
                self.issue_step(env, op, b"Page.captureScreenshot", &Params::Empty, Step::Screenshot);
            }
            Request::Look { page, op, look } => self.request_look(env, page, op, look),
            Request::Close { entity } => self.close(env, entity),
        }
    }

    fn admit_op(&mut self, env: &Env<Limits>, page: Token, token: Token, kind: OpKind) -> bool {
        let unavailable = match self.page(page) {
            Some(page) => page.opening,
            None => true,
        };
        if self.phase != Phase::Ready || unavailable || self.op(token).is_some() {
            self.events.push_back(Event::Refused { op: token, why: Refusal::Gone });
            return false;
        }
        if let OpKind::Go(_) = &kind {
            for active in self.ops.iter().flatten() {
                if active.page == page
                    && let OpKind::Go(_) = active.kind
                {
                    self.events.push_back(Event::Refused { op: token, why: Refusal::Limit });
                    return false;
                }
            }
        }
        let accepted = self.insert_op(Operation { token, page, kind, due: None, warned: false }, env.limits.ops);
        if !accepted {
            self.events.push_back(Event::Refused { op: token, why: Refusal::Limit });
        }
        accepted
    }

    fn start_go(&mut self, env: &Env<Limits>, op: Token) {
        let Some(op_state) = self.op(op) else {
            return;
        };
        let OpKind::Go(to) = &op_state.kind else {
            return;
        };
        let page = op_state.page;
        let to = to.clone();
        if let Some(page_state) = self.page_mut(page)
            && let Go::Address(_) | Go::Reload = to
        {
            page_state.load = Some(Load::Go(op));
            page_state.load_due = Some(env.now.saturating_add(env.limits.answer));
        }
        if let Some(op_state) = self.op_mut(op) {
            op_state.due = Some(env.now.saturating_add(env.limits.answer));
        }
        match to {
            Go::Address(url) => self.issue_step(env, op, b"Page.navigate", &Params::Url(&url), Step::Go),
            Go::Reload => self.issue_step(env, op, b"Page.reload", &Params::Empty, Step::Go),
            Go::Back | Go::Forward => {
                self.issue_step(env, op, b"Page.getNavigationHistory", &Params::Empty, Step::History);
            }
        }
    }

    fn start_find(&mut self, env: &Env<Limits>, op: Token) {
        let Some(op_state) = self.op(op) else {
            return;
        };
        let OpKind::Find { query, .. } = &op_state.kind else {
            return;
        };
        match query.within {
            Some(root) => self.issue_step(env, op, b"DOM.resolveNode", &Params::Backend(root), Step::Query),
            None => self.issue_step(env, op, b"Runtime.evaluate", &Params::Document, Step::Query),
        }
    }

    fn issue_step(&mut self, env: &Env<Limits>, op: Token, method: &[u8], params: &Params<'_>, step: Step) {
        let Some(op_state) = self.op(op) else {
            return;
        };
        let page = op_state.page;
        if !self.queue_page(env, page, method, params, PendingKind::Step(op, step)) {
            self.finish_refused(op, Refusal::Limit);
        }
    }

    fn issue_key(&mut self, env: &Env<Limits>, op: Token, key: Key, shift: bool, down: bool) {
        let (mut name, mut code, mut windows, mut text) = key_info(key);
        let mut modifiers = if shift { SHIFT } else { 0 };
        let letter_name;
        let letter_code;
        if let Key::Letter { letter, alt } = key {
            letter_name = [if shift { letter.to_ascii_uppercase() } else { letter }];
            letter_code = [b'K', b'e', b'y', letter.to_ascii_uppercase()];
            name = &letter_name;
            code = &letter_code;
            windows = u64::from(letter.to_ascii_uppercase());
            text = if alt { b"" } else { name };
            if alt {
                modifiers |= 1;
            }
        }
        let kind = if down { b"keyDown".as_slice() } else { b"keyUp".as_slice() };
        let step = if down { Step::KeyDown } else { Step::KeyUp };
        self.issue_step(
            env,
            op,
            b"Input.dispatchKeyEvent",
            &Params::Key {
                kind,
                key: name,
                code,
                windows,
                text,
                modifiers: if key == Key::SelectAll { 2 } else { modifiers },
                command: (key == Key::SelectAll && down).then_some(b"selectAll".as_slice()),
            },
            step,
        );
    }

    #[expect(clippy::disallowed_methods, reason = "keys decode the requested UTF-8 characters at the browser boundary")]
    fn issue_character(&mut self, env: &Env<Limits>, op: Token, offset: usize, down: bool) {
        let Some((text, Entry::Keys)) = self.type_detail(op) else { return };
        let Ok(value) = core::str::from_utf8(&text) else {
            self.finish_refused(op, Refusal::Protocol);
            return;
        };
        let character = match value.get(offset..) {
            Some(tail) => tail.chars().next(),
            None => None,
        };
        let Some(character) = character else {
            self.finish_done(op);
            return;
        };
        let mut bytes = [0; 4];
        let text = character.encode_utf8(&mut bytes).as_bytes();
        let ascii = u8::try_from(character.to_ascii_uppercase()).unwrap_or(0);
        let digit = [b'D', b'i', b'g', b'i', b't', ascii];
        let letter = [b'K', b'e', b'y', ascii];
        let code = if ascii.is_ascii_digit() {
            digit.as_slice()
        } else if ascii.is_ascii_alphabetic() {
            letter.as_slice()
        } else {
            b""
        };
        let windows = if ascii.is_ascii_alphanumeric() { u64::from(ascii) } else { 0 };
        let kind = if down { b"keyDown".as_slice() } else { b"keyUp".as_slice() };
        let next = if down { offset } else { offset.checked_add(text.len()).expect("character within text") };
        self.issue_step(
            env,
            op,
            b"Input.dispatchKeyEvent",
            &Params::Key { kind, key: text, code, windows, text, modifiers: 0, command: None },
            Step::Character { offset: next, down },
        );
    }

    fn finish_refused(&mut self, op: Token, why: Refusal) {
        if let Some(operation) = self.remove_op(op) {
            if let Some(page) = self.page_mut(operation.page)
                && let Some(Load::Go(waiting)) = page.load
                && waiting == op
            {
                page.load = None;
                page.load_due = None;
            }
            self.events.push_back(Event::Refused { op, why });
        }
    }
    fn finish_done(&mut self, op: Token) {
        if self.remove_op(op).is_some() {
            self.events.push_back(Event::Done { op });
        }
    }

    fn close_page_local(&mut self, token: Token) {
        let mut closing = Vec::new();
        for op in self.ops.iter().flatten() {
            if op.page == token {
                closing.push(op.token);
            }
        }
        for op in closing {
            self.finish_refused(op, Refusal::Crashed);
        }
        if let Some(page) = self.page_mut(token) {
            page.closed = true;
            page.load = None;
            page.load_due = None;
            self.events.push_back(Event::Closed { owner: token });
        }
    }
    fn close_person_local(&mut self, token: Token) {
        let mut closing = Vec::new();
        for page in self.pages.iter().flatten() {
            if page.person == token && !page.closed {
                closing.push(page.token);
            }
        }
        for page in closing {
            self.close_page_local(page);
        }
        self.downloads.retain(|download| download.person != token);
        if let Some(person) = self.person_mut(token) {
            person.closed = true;
            self.events.push_back(Event::Closed { owner: token });
        }
    }
    fn close_browser_local(&mut self) {
        if self.phase == Phase::Closed {
            return;
        }
        let mut closing = Vec::new();
        for person in self.persons.iter().flatten() {
            if !person.closed {
                closing.push(person.token);
            }
        }
        for person in closing {
            self.close_person_local(person);
        }
        self.phase = Phase::Closed;
        self.events.push_back(Event::Closed { owner: self.owner });
    }
    fn close(&mut self, env: &Env<Limits>, entity: Token) {
        if entity == self.owner {
            if self.phase == Phase::Closed || self.phase == Phase::Closing {
                return;
            }
            self.phase = Phase::Closing;
            for person in self.persons.iter_mut().flatten() {
                person.closing = true;
            }
            self.close_browser_downloads(env);
            return;
        }
        let target = match self.page(entity) {
            Some(page) => page.target.clone(),
            None => None,
        };
        if let Some(target) = target {
            if !self.queue_command(
                env,
                b"Target.closeTarget",
                None,
                &Params::TargetId(&target),
                PendingKind::ClosePage(entity),
            ) {
                self.close_page_local(entity);
            }
            return;
        }
        if let Some(person) = self.person_mut(entity) {
            if person.closing {
                return;
            }
            person.closing = true;
            self.close_person_downloads(env, entity);
        }
    }
}

fn field_text(value: Value<'_>, key: &[u8]) -> Option<Box<[u8]>> {
    value.get(key)?.text()
}

fn field_u64(value: Value<'_>, key: &[u8]) -> Option<u64> {
    value.get(key)?.u64()
}

fn field_i64(value: Value<'_>, key: &[u8]) -> Option<i64> {
    value.get(key)?.i64()
}

fn field_array<'a>(value: Value<'a>, key: &[u8]) -> Option<crate::wire::decode::Array<'a>> {
    value.get(key)?.array()
}

fn opt_field<'a>(value: Option<Value<'a>>, key: &[u8]) -> Option<Value<'a>> {
    value?.get(key)
}

fn opt_text(value: Option<Value<'_>>) -> Option<Box<[u8]>> {
    value?.text()
}

fn opt_bool(value: Option<Value<'_>>) -> Option<bool> {
    value?.bool()
}

fn opt_array(value: Option<Value<'_>>) -> Option<crate::wire::decode::Array<'_>> {
    value?.array()
}

fn parse_seen(node: Value<'_>, text_limit: u32) -> Option<Seen> {
    let backend = field_u64(node, b"backendDOMNodeId")?;
    let role = ax_text(node, b"role", text_limit);
    let name = ax_text(node, b"name", text_limit);
    let value = ax_text(node, b"value", text_limit);
    let states = ax_states(node);
    Some(Seen { node: backend, role, name, value, states, rect: None })
}

fn ax_states(node: Value<'_>) -> States {
    let mut states = States { focused: false, disabled: false, checked: false, expanded: false, selected: false };
    if let Some(properties) = field_array(node, b"properties") {
        for property in properties {
            let name = field_text(property, b"name");
            let value = ax_property_bool(property);
            match name.as_deref() {
                Some(b"focused") => states.focused = value,
                Some(b"disabled") => states.disabled = value,
                Some(b"checked") => states.checked = value,
                Some(b"expanded") => states.expanded = value,
                Some(b"selected") => states.selected = value,
                _ => {}
            }
        }
    }
    states
}

fn ax_text(node: Value<'_>, key: &[u8], limit: u32) -> Box<[u8]> {
    let mut text = opt_text(opt_field(node.get(key), b"value")).unwrap_or_default();
    let max = usize::try_from(limit).expect("u32 fits usize");
    if text.len() > max {
        text = Box::from(text.get(..max).expect("text is at least the limit"));
    }
    text
}

fn ax_disabled(node: Value<'_>) -> bool {
    if let Some(properties) = field_array(node, b"properties") {
        for property in properties {
            if field_text(property, b"name").as_deref() == Some(b"disabled") {
                return ax_property_bool(property);
            }
        }
    }
    false
}

fn ax_property_bool(property: Value<'_>) -> bool {
    let value = opt_field(property.get(b"value"), b"value");
    match opt_bool(value) {
        Some(value) => value,
        None => opt_text(value).as_deref() == Some(b"true"),
    }
}

fn parse_rect(result: Value<'_>) -> Option<Rect> {
    let mut quads = result.get(b"quads")?.array()?;
    let quad = quads.next()?;
    let mut points = quad.array()?;
    let mut left = i64::MAX;
    let mut top = i64::MAX;
    let mut right = i64::MIN;
    let mut bottom = i64::MIN;
    for _ in 0_u8..4_u8 {
        let x = points.next()?.whole_pixels()?;
        let y = points.next()?.whole_pixels()?;
        left = left.min(x);
        right = right.max(x);
        top = top.min(y);
        bottom = bottom.max(y);
    }
    Some(Rect {
        x: left,
        y: top,
        width: right.checked_sub(left)?.try_into().ok()?,
        height: bottom.checked_sub(top)?.try_into().ok()?,
    })
}

fn viewport_offset(result: Value<'_>) -> Option<(i64, i64)> {
    let viewport = match result.get(b"cssVisualViewport") {
        Some(viewport) => viewport,
        None => match result.get(b"cssLayoutViewport") {
            Some(viewport) => viewport,
            None => result.get(b"layoutViewport")?,
        },
    };
    Some((viewport.get(b"pageX")?.whole_pixels()?, viewport.get(b"pageY")?.whole_pixels()?))
}

fn subtree_has(root: Option<Value<'_>>, backend: u64) -> bool {
    let Some(root) = root else {
        return false;
    };
    let mut nodes = Vec::new();
    nodes.push(root);
    while let Some(node) = nodes.pop() {
        if field_u64(node, b"backendNodeId") == Some(backend) {
            return true;
        }
        for key in [b"children".as_slice(), b"shadowRoots".as_slice(), b"pseudoElements".as_slice()] {
            if let Some(children) = opt_array(node.get(key)) {
                for child in children {
                    nodes.push(child);
                }
            }
        }
    }
    false
}

fn snapshot(result: Value<'_>, limit: u32, text_limit: u32) -> Box<[u8]> {
    let mut out = Vec::new();
    let cap = usize::try_from(limit).expect("u32 fits usize");
    if let Some(nodes) = field_array(result, b"nodes") {
        for node in nodes {
            let role = ax_text(node, b"role", text_limit);
            let name = ax_text(node, b"name", text_limit);
            let value = ax_text(node, b"value", text_limit);
            for part in [role.as_ref(), b" ", name.as_ref(), b" ", value.as_ref(), b"\n"] {
                let room = cap.saturating_sub(out.len());
                if room == 0 {
                    break;
                }
                out.extend_from_slice(part.get(..room.min(part.len())).expect("within part"));
            }
            if out.len() == cap {
                break;
            }
        }
    }
    out.into_boxed_slice()
}

/// A request from the test.
pub fn down(
    browser: &mut Browser,
    env: &Env<Limits>,
    request: crate::boundary::Request,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    if browser.phase == Phase::Closed {
        return;
    }
    browser.request(env, request);
    browser.flush(below, env.limits.command);
    browser.emit(above);
}

/// An event from a child pipe.
pub fn up(browser: &mut Browser, env: &Env<Limits>, event: Below, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    if browser.phase == Phase::Closed && browser.events.is_empty() {
        return;
    }
    match event {
        Below::Commands(stream::Up::Room) => {
            browser.room = true;
            browser.command_demanded = false;
        }
        Below::Commands(stream::Up::Failed(_) | stream::Up::End | stream::Up::Bytes(_))
        | Below::Replies(stream::Up::End | stream::Up::Failed(_) | stream::Up::Room) => browser.crash(),
        Below::Replies(stream::Up::Bytes(bytes)) => {
            browser.replies_demanded = false;
            match browser.framing.push(&bytes) {
                Ok(messages) => {
                    for message in messages {
                        browser.message(env, &message);
                    }
                }
                Err(_) => browser.crash(),
            }
            if browser.phase != Phase::Closed {
                let nul = Delimiter::new(b"\0").expect("NUL is a delimiter");
                below.push(Down::Replies(stream::Down::Demand {
                    read: Read::Scan { until: nul, max: env.limits.message.saturating_add(1) },
                    room: 0,
                }));
                browser.replies_demanded = true;
            }
        }
        Below::Errors(stream::Up::Bytes(bytes)) => {
            browser.errors_demanded = false;
            for byte in bytes {
                if browser.stderr.len() == usize::try_from(env.limits.stderr).expect("u32 fits usize")
                    && !browser.stderr.is_empty()
                {
                    browser.stderr.remove(0);
                }
                if env.limits.stderr > 0 {
                    browser.stderr.push(byte);
                }
            }
            if browser.phase != Phase::Closed {
                below.push(Down::Errors(stream::Down::Demand {
                    read: Read::Line { max: env.limits.stderr.max(1) },
                    room: 0,
                }));
                browser.errors_demanded = true;
            }
        }
        Below::Errors(stream::Up::End | stream::Up::Failed(_)) => browser.errors_demanded = false,
        Below::Errors(stream::Up::Room) => {}
    }
    if below.room() >= 2 {
        browser.flush(below, env.limits.command);
    }
    browser.emit(above);
}

/// One deadline or queued output, called while the loop reports work due.
pub fn fire(browser: &mut Browser, env: &Env<Limits>, now: Time, above: &mut Queue<Event>, below: &mut Queue<Down>) {
    if browser.phase == Phase::Starting && browser.pending.is_empty() {
        browser.start(env, below);
        browser.emit(above);
        return;
    }
    let mut expired = None;
    for (index, pending) in browser.pending.iter().enumerate() {
        if pending.due <= now {
            expired = Some(index);
            break;
        }
    }
    if let Some(index) = expired {
        let pending = browser.pending.swap_remove(index);
        browser.outgoing.retain(|command| command.id != pending.id);
        match pending.kind {
            PendingKind::Step(op, _) => {
                if let Some(Operation { kind: OpKind::Find { expect: Some((_, until)), .. }, .. }) = browser.op(op)
                    && *until <= now
                {
                    if let Some(Operation { kind: OpKind::Find { seen, more, .. }, .. }) = browser.remove_op(op) {
                        browser.events.push_back(Event::Missed { op, seen, more });
                    }
                } else {
                    browser.finish_refused(op, Refusal::Timeout);
                }
            }
            PendingKind::Release | PendingKind::Dialog => {}
            PendingKind::Version | PendingKind::CancelBrowser | PendingKind::CloseBrowser => browser.crash(),
            PendingKind::Context(person)
            | PendingKind::Downloads(person)
            | PendingKind::CancelPerson(person)
            | PendingKind::ClosePerson(person) => browser.close_person_local(person),
            PendingKind::Target(page)
            | PendingKind::Attach(page)
            | PendingKind::Enable(page, _)
            | PendingKind::Viewport(page)
            | PendingKind::Navigate(page)
            | PendingKind::ClosePage(page) => browser.close_page_local(page),
        }
    } else {
        let mut due = None;
        for op in browser.ops.iter().flatten() {
            if let Some(at) = op.due
                && at <= now
            {
                due = Some(op.token);
                break;
            }
        }
        if let Some(op) = due {
            let is_find = match browser.op(op) {
                Some(Operation { kind: OpKind::Find { .. }, .. }) => true,
                Some(Operation {
                    kind:
                        OpKind::Go(_)
                        | OpKind::Press { .. }
                        | OpKind::Type { .. }
                        | OpKind::Key { .. }
                        | OpKind::Attribute { .. }
                        | OpKind::Look { .. }
                        | OpKind::Address
                        | OpKind::Snapshot
                        | OpKind::Screenshot,
                    ..
                })
                | None => false,
            };
            if let Some(op_state) = browser.op_mut(op) {
                op_state.due = None;
            }
            if is_find {
                browser.start_find(env, op);
            } else if let Some(Operation { kind: OpKind::Attribute { .. }, .. }) = browser.op(op) {
                browser.start_attribute(env, op);
            } else if let Some(Operation { kind: OpKind::Look { .. }, .. }) = browser.op(op) {
                browser.start_look(env, op);
            } else {
                browser.finish_refused(op, Refusal::Timeout);
            }
        } else {
            let mut page_due = None;
            for page in browser.pages.iter().flatten() {
                if let Some(at) = page.load_due
                    && at <= now
                {
                    page_due = Some(page.token);
                    break;
                }
            }
            if let Some(page) = page_due {
                browser.close_page_local(page);
            }
        }
    }
    if below.room() >= 2 {
        browser.flush(below, env.limits.command);
    }
    browser.emit(above);
}

fn key_info(key: Key) -> (&'static [u8], &'static [u8], u64, &'static [u8]) {
    match key {
        Key::Enter => (b"Enter", b"Enter", 13, b"\r"),
        Key::Escape => (b"Escape", b"Escape", 27, b""),
        Key::Tab => (b"Tab", b"Tab", 9, b"\t"),
        Key::Backspace => (b"Backspace", b"Backspace", 8, b""),
        Key::ArrowUp => (b"ArrowUp", b"ArrowUp", 38, b""),
        Key::ArrowDown => (b"ArrowDown", b"ArrowDown", 40, b""),
        Key::ArrowLeft => (b"ArrowLeft", b"ArrowLeft", 37, b""),
        Key::ArrowRight => (b"ArrowRight", b"ArrowRight", 39, b""),
        Key::SelectAll => (b"a", b"KeyA", 65, b""),
        Key::Letter { .. } => (b"", b"", 0, b""),
    }
}

impl Browser {
    fn message(&mut self, env: &Env<Limits>, bytes: &[u8]) {
        let Ok(document) = Document::parse(bytes, env.limits.message) else {
            self.crash();
            return;
        };
        let root = document.root();
        if let Some(id) = field_u64(root, b"id") {
            let mut at = None;
            for (index, pending) in self.pending.iter().enumerate() {
                if pending.id == id {
                    at = Some(index);
                    break;
                }
            }
            let Some(at) = at else {
                // A command that reached its deadline or whose operation
                // closed may still answer later. Its id is never reused.
                if id >= self.next_id {
                    self.crash();
                } else if let Some(result) = root.get(b"result")
                    && let Some(object) = result_object(result)
                {
                    let session = field_text(root, b"sessionId");
                    self.queue_command(
                        env,
                        b"Runtime.releaseObject",
                        session.as_deref(),
                        &Params::Object(&object),
                        PendingKind::Release,
                    );
                }
                return;
            };
            let pending = self.pending.swap_remove(at);
            if let Some(error) = root.get(b"error") {
                self.command_error(env, pending.kind, error);
                return;
            }
            let Some(result) = root.get(b"result") else {
                self.command_error(env, pending.kind, root);
                return;
            };
            self.reply(env, pending.kind, result);
        } else if let Some(method) = field_text(root, b"method") {
            self.notification(env, &method, root);
        } else {
            self.crash();
        }
    }

    fn command_error(&mut self, env: &Env<Limits>, kind: PendingKind, error: Value<'_>) {
        let message = field_text(error, b"message").unwrap_or_default();
        if let PendingKind::Step(op, Step::Query) = kind
            && message.as_ref() == b"Node with given id does not belong to the document"
        {
            self.finish_refused(op, Refusal::Gone);
            return;
        }
        if let PendingKind::Step(op, Step::Query) = kind
            && (contains(&message, b"context") || contains(&message, b"object"))
        {
            if let Some(op) = self.op_mut(op) {
                op.due = Some(env.now.saturating_add(env.limits.poll));
            }
            return;
        }
        if let PendingKind::Step(op, Step::AttributeSelector | Step::AttributeValue) = kind
            && look::node_missing(&message)
        {
            // A render replaced the element between the selector and its
            // read: the read starts again from the document until its
            // deadline.
            let again = env.now.saturating_add(env.limits.poll);
            if let Some(Operation { kind: OpKind::Attribute { until, .. }, due, .. }) = self.op_mut(op)
                && again < *until
            {
                *due = Some(again);
            } else {
                self.finish_refused(op, Refusal::Gone);
            }
            return;
        }
        if let PendingKind::Step(op, Step::Look(step)) = kind {
            self.look_error(env, op, step, &message);
            return;
        }
        let why = if contains(&message, b"No node found at given location") {
            Refusal::Covered
        } else if contains(&message, b"No node found") {
            Refusal::Gone
        } else {
            Refusal::Protocol
        };
        match kind {
            PendingKind::Release | PendingKind::Dialog => {}
            PendingKind::Version | PendingKind::CancelBrowser | PendingKind::CloseBrowser => self.crash(),
            PendingKind::Context(person)
            | PendingKind::Downloads(person)
            | PendingKind::CancelPerson(person)
            | PendingKind::ClosePerson(person) => self.close_person_local(person),
            PendingKind::Target(page)
            | PendingKind::Attach(page)
            | PendingKind::Enable(page, _)
            | PendingKind::Viewport(page)
            | PendingKind::Navigate(page)
            | PendingKind::ClosePage(page) => self.close_page_local(page),
            PendingKind::Step(op, step) => {
                let reason = if step == Step::AttributeSelector {
                    Refusal::Protocol
                } else if step == Step::Hit && why == Refusal::Gone {
                    Refusal::Covered
                } else {
                    why
                };
                self.finish_refused(op, reason);
            }
        }
    }

    fn reply(&mut self, env: &Env<Limits>, kind: PendingKind, result: Value<'_>) {
        match kind {
            PendingKind::Release | PendingKind::Dialog => {}
            PendingKind::Version => {
                let version = field_text(result, b"product").unwrap_or_default();
                self.phase = Phase::Ready;
                self.events.push_back(Event::Ready { version });
            }
            PendingKind::Context(person) => {
                let context = field_text(result, b"browserContextId");
                if let (Some(state), Some(context)) = (self.person_mut(person), context) {
                    state.context = Some(context.clone());
                    let directory = state.downloads.clone();
                    if !self.queue_command(
                        env,
                        b"Browser.setDownloadBehavior",
                        None,
                        &Params::Downloads { context: &context, directory: directory.as_deref() },
                        PendingKind::Downloads(person),
                    ) {
                        self.close_person_local(person);
                    }
                } else {
                    self.close_person_local(person);
                }
            }
            PendingKind::Downloads(person) => {
                if let Some(state) = self.person_mut(person) {
                    state.ready = true;
                }
            }
            PendingKind::CancelPerson(person) => self.close_person_downloads(env, person),
            PendingKind::CancelBrowser => self.close_browser_downloads(env),
            PendingKind::Target(page) => {
                let target = field_text(result, b"targetId");
                if let Some(target) = target {
                    if let Some(page_state) = self.page_mut(page) {
                        page_state.target = Some(target.clone());
                    }
                    if !self.queue_command(
                        env,
                        b"Target.attachToTarget",
                        None,
                        &Params::AttachTarget(&target),
                        PendingKind::Attach(page),
                    ) {
                        self.close_page_local(page);
                    }
                } else {
                    self.close_page_local(page);
                }
            }
            PendingKind::Attach(page) => {
                let session = field_text(result, b"sessionId");
                if let Some(session) = session {
                    if let Some(page_state) = self.page_mut(page) {
                        page_state.session = Some(session);
                    }
                    self.enable(env, page, 0);
                } else {
                    self.close_page_local(page);
                }
            }
            PendingKind::Enable(page, index) => self.enable(env, page, index.saturating_add(1)),
            PendingKind::Viewport(page) => {
                let url = match self.page(page) {
                    Some(page) => Some(page.initial_url.clone()),
                    None => None,
                };
                if let Some(url) = url {
                    if let Some(page_state) = self.page_mut(page) {
                        page_state.load = Some(Load::Opening);
                        page_state.load_due = Some(env.now.saturating_add(env.limits.answer));
                    }
                    if !self.queue_page(env, page, b"Page.navigate", &Params::Url(&url), PendingKind::Navigate(page)) {
                        self.close_page_local(page);
                    }
                }
            }
            PendingKind::Navigate(page) => {
                if result.get(b"errorText").is_some() {
                    self.close_page_local(page);
                }
            }
            PendingKind::Step(op, step) => self.step_reply(env, op, step, result),
            PendingKind::CloseBrowser => self.close_browser_local(),
            PendingKind::ClosePage(page) => self.close_page_local(page),
            PendingKind::ClosePerson(person) => self.close_person_local(person),
        }
    }

    fn enable(&mut self, env: &Env<Limits>, page: Token, index: u8) {
        const DOMAINS: [&[u8]; 6] = [
            b"Page.enable",
            b"Runtime.enable",
            b"Log.enable",
            b"Accessibility.enable",
            b"DOM.enable",
            b"Inspector.enable",
        ];
        if let Some(method) = DOMAINS.get(usize::from(index)) {
            if !self.queue_page(env, page, method, &Params::Empty, PendingKind::Enable(page, index)) {
                self.close_page_local(page);
            }
        } else if !self.queue_page(
            env,
            page,
            b"Emulation.setDeviceMetricsOverride",
            &Params::Viewport { width: 1280, height: 800, mobile: false },
            PendingKind::Viewport(page),
        ) {
            self.close_page_local(page);
        }
    }

    fn dialog(&mut self, env: &Env<Limits>, page: Token, params: Value<'_>) {
        let kind = field_text(params, b"type").unwrap_or_default();
        let mut go = None;
        if kind.as_ref() == b"beforeunload" {
            for operation in self.ops.iter_mut().flatten() {
                if operation.page != page {
                    continue;
                }
                match operation.kind {
                    OpKind::Go(_) => {
                        go = Some(operation.token);
                        if !operation.warned {
                            operation.warned = true;
                            self.events.push_back(Event::Warned { op: operation.token });
                        }
                        break;
                    }
                    OpKind::Find { .. }
                    | OpKind::Press { .. }
                    | OpKind::Type { .. }
                    | OpKind::Key { .. }
                    | OpKind::Attribute { .. }
                    | OpKind::Look { .. }
                    | OpKind::Address
                    | OpKind::Snapshot
                    | OpKind::Screenshot => {}
                }
            }
        }
        if !self.queue_page(
            env,
            page,
            b"Page.handleJavaScriptDialog",
            &Params::Dialog { accept: go.is_some() },
            PendingKind::Dialog,
        ) {
            self.close_page_local(page);
        }
        if go.is_none() {
            let message = field_text(params, b"message").unwrap_or_default();
            let limit = usize::try_from(env.limits.text).expect("text limit fits usize");
            let length = kind.len().saturating_add(2).saturating_add(message.len()).min(limit);
            let mut text = skein_lib::Writer::new(length);
            for part in [kind.as_ref(), b": ", message.as_ref()] {
                let kept = part.len().min(text.room());
                text.put(part.get(..kept).expect("prefix of part")).expect("text room checked");
            }
            self.events.push_back(Event::Trouble { page, trouble: Trouble::Dialog, text: text.finish() });
        }
    }

    fn notification_page(&self, session: Option<&[u8]>) -> Option<Token> {
        for candidate in self.pages.iter().flatten() {
            if !candidate.closed && candidate.session.as_deref() == session {
                return Some(candidate.token);
            }
        }
        None
    }

    fn notification(&mut self, env: &Env<Limits>, method: &[u8], root: Value<'_>) {
        let session = field_text(root, b"sessionId");
        let page = self.notification_page(session.as_deref());
        let params = root.get(b"params");
        match method {
            b"Browser.downloadWillBegin" => {
                if let Some(params) = params {
                    self.download_begin(env, params);
                }
            }
            b"Browser.downloadProgress" => {
                if let Some(params) = params {
                    self.download_progress(env, params);
                }
            }
            b"Page.javascriptDialogOpening" => {
                if let (Some(page), Some(params)) = (page, params) {
                    self.dialog(env, page, params);
                }
            }
            b"Page.frameRequestedNavigation" | b"Page.frameStartedLoading" => {
                if let (Some(page), Some(params)) = (page, params) {
                    let frame = frame_id(params, b"frameId", env.limits.text);
                    let mut began = false;
                    if let Some(page) = self.page_mut(page)
                        && (page.frame.is_none() || page.frame == frame)
                        && !page.navigating
                    {
                        page.navigating = true;
                        began = true;
                    }
                    if began {
                        self.restart_finds(env, page);
                    }
                }
            }
            b"Page.frameNavigated" => self.frame_navigated(env, page, params),
            b"Page.loadEventFired" | b"Page.navigatedWithinDocument" => {
                if let Some(page) = page {
                    self.loaded(page);
                }
            }
            b"Runtime.exceptionThrown" => {
                if let (Some(page), Some(params)) = (page, params) {
                    let text = opt_text(opt_field(params.get(b"exceptionDetails"), b"text")).unwrap_or_default();
                    self.trouble(page, Trouble::Exception, text, env.limits.text);
                }
            }
            b"Runtime.consoleAPICalled" => {
                if let (Some(page), Some(params)) = (page, params)
                    && field_text(params, b"type").as_deref() == Some(b"error")
                {
                    let first = match field_array(params, b"args") {
                        Some(mut args) => args.next(),
                        None => None,
                    };
                    let content = match first {
                        Some(arg) => match arg.get(b"value") {
                            Some(value) => Some(value),
                            None => arg.get(b"description"),
                        },
                        None => None,
                    };
                    let text = opt_text(content).unwrap_or_default();
                    self.trouble(page, Trouble::Console, text, env.limits.text);
                }
            }
            b"Log.entryAdded" => {
                if let (Some(page), Some(params)) = (page, params) {
                    let entry = params.get(b"entry");
                    if opt_text(opt_field(entry, b"level")).as_deref() == Some(b"error") {
                        let text = opt_text(opt_field(entry, b"text")).unwrap_or_default();
                        self.trouble(page, Trouble::Log, text, env.limits.text);
                    }
                }
            }
            b"Inspector.targetCrashed" => {
                if let Some(page) = page {
                    self.trouble(page, Trouble::Crash, Box::default(), env.limits.text);
                    self.close_page_local(page);
                }
            }
            b"Target.targetCrashed" => {
                if let Some(params) = params {
                    let target = field_text(params, b"targetId");
                    let mut page = None;
                    for candidate in self.pages.iter().flatten() {
                        if !candidate.closed && candidate.target.as_deref() == target.as_deref() {
                            page = Some(candidate.token);
                            break;
                        }
                    }
                    if let Some(page) = page {
                        self.trouble(page, Trouble::Crash, Box::default(), env.limits.text);
                        self.close_page_local(page);
                    }
                }
            }
            _ => {}
        }
    }

    fn frame_navigated(&mut self, env: &Env<Limits>, page: Option<Token>, params: Option<Value<'_>>) {
        if let (Some(page), Some(frame)) = (page, opt_field(params, b"frame"))
            && frame.get(b"parentId").is_none()
            && let Some(page) = self.page_mut(page)
        {
            page.frame = frame_id(frame, b"id", env.limits.text);
        }
        if let Some(page) = page
            && opt_text(opt_field(params, b"type")).as_deref() == Some(b"BackForwardCacheRestore")
            && opt_field(opt_field(params, b"frame"), b"parentId").is_none()
        {
            self.loaded(page);
        }
    }

    fn trouble(&mut self, page: Token, trouble: Trouble, mut text: Box<[u8]>, limit: u32) {
        let limit = usize::try_from(limit).expect("u32 fits usize");
        if text.len() > limit {
            text = Box::from(text.get(..limit).expect("text is longer than limit"));
        }
        self.events.push_back(Event::Trouble { page, trouble, text });
    }

    fn restart_finds(&mut self, env: &Env<Limits>, page: Token) {
        for op in self.ops.iter_mut().flatten() {
            if op.page == page && look::waits_for_document(&op.kind) {
                op.due = Some(env.now);
            }
        }
        let ops = &self.ops;
        let outgoing = &mut self.outgoing;
        self.pending.retain(|pending| {
            if let PendingKind::Step(token, _) = pending.kind {
                for op in ops.iter().flatten() {
                    if op.token == token && op.page == page && look::waits_for_document(&op.kind) {
                        outgoing.retain(|command| command.id != pending.id);
                        return false;
                    }
                }
            }
            true
        });
    }

    fn loaded(&mut self, page: Token) {
        let load = match self.page_mut(page) {
            Some(page) => {
                page.navigating = false;
                page.load_due = None;
                page.load.take()
            }
            None => None,
        };
        match load {
            Some(Load::Opening) => {
                if let Some(page_state) = self.page_mut(page) {
                    page_state.opening = false;
                }
                self.events.push_back(Event::Opened { page });
            }
            Some(Load::Go(op)) => self.finish_done(op),
            None => {}
        }
    }

    fn crash(&mut self) {
        self.pending.clear();
        self.outgoing.clear();
        self.close_browser_local();
    }
}

fn result_object(value: Value<'_>) -> Option<Box<[u8]>> {
    let object = match value.get(b"result") {
        Some(object) => object,
        None => value.get(b"object")?,
    };
    field_text(object, b"objectId")
}

fn frame_id(value: Value<'_>, key: &[u8], limit: u32) -> Option<Box<[u8]>> {
    let text = field_text(value, key)?;
    let length = text.len().min(usize::try_from(limit).expect("u32 fits usize"));
    Some(Box::from(text.get(..length).expect("length is within frame identity")))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    for window in haystack.windows(needle.len()) {
        if window == needle {
            return true;
        }
    }
    false
}

impl Browser {
    fn op_page(&self, token: Token) -> Option<Token> {
        Some(self.op(token)?.page)
    }

    fn press_node(&self, token: Token) -> Option<u64> {
        if let OpKind::Press { node, .. } = &self.op(token)?.kind { Some(*node) } else { None }
    }

    fn press_hit(&self, token: Token) -> Option<u64> {
        if let OpKind::Press { hit, .. } = &self.op(token)?.kind { *hit } else { None }
    }

    fn type_detail(&self, token: Token) -> Option<(Box<[u8]>, Entry)> {
        if let OpKind::Type { text, entry } = &self.op(token)?.kind { Some((text.clone(), *entry)) } else { None }
    }

    fn key_state(&self, token: Token) -> Option<(Key, bool)> {
        if let OpKind::Key { key, shift } = &self.op(token)?.kind { Some((*key, *shift)) } else { None }
    }

    fn query_document(&mut self, env: &Env<Limits>, token: Token, object: &[u8]) {
        let Some(Operation { page, kind: OpKind::Find { query, .. }, .. }) = self.op(token) else { return };
        let page = *page;
        let query = query.clone();
        let name = match query.matching {
            Match::Exact => Some(query.name.as_ref()),
            Match::Words => None,
        };
        self.issue_step(
            env,
            token,
            b"Accessibility.queryAXTree",
            &Params::Query { object, role: &query.role, name },
            Step::Query,
        );
        if !self.queue_page(env, page, b"Runtime.releaseObject", &Params::Object(object), PendingKind::Release) {
            self.close_page_local(page);
        }
    }

    #[expect(clippy::too_many_lines, reason = "one match lists the reply transition for each operation step")]
    fn step_reply(&mut self, env: &Env<Limits>, token: Token, step: Step, result: Value<'_>) {
        match step {
            Step::History => {
                let current = field_i64(result, b"currentIndex");
                let entries = field_array(result, b"entries");
                let direction = if let Some(Operation { kind: OpKind::Go(Go::Back), .. }) = self.op(token) {
                    Some(-1_i64)
                } else if let Some(Operation { kind: OpKind::Go(Go::Forward), .. }) = self.op(token) {
                    Some(1_i64)
                } else {
                    None
                };
                let Some(current) = current else {
                    self.finish_refused(token, Refusal::Protocol);
                    return;
                };
                let Some(entries) = entries else {
                    self.finish_refused(token, Refusal::Protocol);
                    return;
                };
                let Some(direction) = direction else {
                    self.finish_refused(token, Refusal::Protocol);
                    return;
                };
                let Some(wanted) = current.checked_add(direction) else {
                    self.finish_refused(token, Refusal::Gone);
                    return;
                };
                let mut entry_id = None;
                for (index, entry) in entries.enumerate() {
                    if i64::try_from(index).ok() == Some(wanted) {
                        entry_id = field_u64(entry, b"id");
                        break;
                    }
                }
                if let Some(id) = entry_id {
                    if let Some(page) = self.op_page(token)
                        && let Some(page_state) = self.page_mut(page)
                    {
                        page_state.load = Some(Load::Go(token));
                        page_state.load_due = Some(env.now.saturating_add(env.limits.answer));
                    }
                    self.issue_step(env, token, b"Page.navigateToHistoryEntry", &Params::HistoryEntry(id), Step::Go);
                } else {
                    self.finish_refused(token, Refusal::Gone);
                }
            }
            Step::Go => {
                if result.get(b"errorText").is_some() {
                    self.finish_refused(token, Refusal::Protocol);
                    return;
                }
                let page = self.op_page(token);
                if let Some(page) = page
                    && let Some(page_state) = self.page_mut(page)
                {
                    page_state.load = Some(Load::Go(token));
                }
            }
            Step::Query => {
                if let Some(object) = result_object(result) {
                    self.query_document(env, token, &object);
                    return;
                }
                let Some(nodes) = field_array(result, b"nodes") else {
                    self.finish_refused(token, Refusal::Protocol);
                    return;
                };
                if let Some(op) = self.op_mut(token)
                    && let OpKind::Find { query, seen, more, next, .. } = &mut op.kind
                {
                    seen.clear();
                    *more = 0;
                    *next = 0;
                    for node in nodes {
                        if !crate::find::accepts(node, query) {
                            continue;
                        }
                        let Some(seen_node) = parse_seen(node, env.limits.text) else {
                            continue;
                        };
                        if seen.push(seen_node).is_err() {
                            *more = more.saturating_add(1);
                        }
                    }
                }
                let needs_box = if let Some(Operation { kind: OpKind::Find { query, seen, .. }, .. }) = self.op(token) {
                    query.boxes && !seen.is_empty()
                } else {
                    false
                };
                if needs_box {
                    self.next_find_box(env, token);
                } else {
                    self.finish_find(env, token);
                }
            }
            Step::Box => {
                let rect = parse_rect(result);
                if let Some(op) = self.op_mut(token)
                    && let OpKind::Find { seen, next, .. } = &mut op.kind
                {
                    if let Some(seen) = seen.get_mut(*next) {
                        seen.rect = rect;
                    }
                    *next = next.saturating_add(1);
                }
                let remaining = if let Some(Operation { kind: OpKind::Find { seen, next, .. }, .. }) = self.op(token) {
                    *next < seen.len()
                } else {
                    false
                };
                if remaining {
                    self.next_find_box(env, token);
                } else {
                    self.finish_find(env, token);
                }
            }
            Step::Scroll => {
                let node = self.press_node(token);
                if let Some(node) = node {
                    self.issue_step(
                        env,
                        token,
                        b"Accessibility.getPartialAXTree",
                        &Params::Backend(node),
                        Step::Accessibility,
                    );
                }
            }
            Step::Accessibility => {
                let first = match field_array(result, b"nodes") {
                    Some(mut nodes) => nodes.next(),
                    None => None,
                };
                let disabled = match first {
                    Some(node) => ax_disabled(node),
                    None => false,
                };
                if disabled {
                    self.finish_refused(token, Refusal::Disabled);
                    return;
                }
                let node = self.press_node(token);
                if let Some(node) = node {
                    self.issue_step(env, token, b"DOM.getContentQuads", &Params::Quads(node), Step::Quads);
                }
            }
            Step::Quads => {
                let Some(rect) = parse_rect(result) else {
                    self.finish_refused(token, Refusal::Hidden);
                    return;
                };
                let point = match self.op(token) {
                    Some(Operation { kind: OpKind::Press { point, .. }, .. }) => *point,
                    Some(_) | None => None,
                };
                let Some((x, y)) = crate::find::position(rect, point) else {
                    self.finish_refused(token, Refusal::Hidden);
                    return;
                };
                if let Some(op) = self.op_mut(token)
                    && let OpKind::Press { at, .. } = &mut op.kind
                {
                    *at = Some((x, y));
                }
                self.issue_step(env, token, b"Page.getLayoutMetrics", &Params::Empty, Step::Metrics);
            }
            Step::Metrics => {
                // Quads and mouse events use viewport coordinates; the DOM
                // hit test uses document coordinates after a scroll.
                let offset = viewport_offset(result);
                let at = match self.op(token) {
                    Some(Operation { kind: OpKind::Press { at, .. }, .. }) => *at,
                    Some(_) | None => None,
                };
                if let (Some((x, y)), Some((page_x, page_y))) = (at, offset) {
                    self.issue_step(
                        env,
                        token,
                        b"DOM.getNodeForLocation",
                        &Params::Point { x: x.saturating_add(page_x), y: y.saturating_add(page_y) },
                        Step::Hit,
                    );
                } else {
                    self.finish_refused(token, Refusal::Protocol);
                }
            }
            Step::Hit => {
                let hit = field_u64(result, b"backendNodeId");
                let node = self.press_node(token);
                match (hit, node) {
                    (Some(hit), Some(node)) if hit == node => self.mouse(env, token, Step::MouseMove),
                    (Some(hit), Some(node)) => {
                        if let Some(op) = self.op_mut(token)
                            && let OpKind::Press { hit: slot, .. } = &mut op.kind
                        {
                            *slot = Some(hit);
                        }
                        self.issue_step(
                            env,
                            token,
                            b"DOM.describeNode",
                            &Params::Describe { backend: node, depth: -1 },
                            Step::Describe,
                        );
                    }
                    _ => self.finish_refused(token, Refusal::Covered),
                }
            }
            Step::Describe => {
                let hit = self.press_hit(token);
                if let Some(hit) = hit
                    && subtree_has(result.get(b"node"), hit)
                {
                    self.mouse(env, token, Step::MouseMove);
                    return;
                }
                self.finish_refused(token, Refusal::Covered);
            }
            Step::MouseMove => self.mouse(env, token, Step::MouseDown),
            Step::MouseDown => self.mouse(env, token, Step::MouseUp),
            Step::MouseUp | Step::Insert | Step::KeyUp => self.finish_done(token),
            Step::TypeNode { node } => {
                let name = match result.get(b"node") {
                    Some(node) => field_text(node, b"nodeName"),
                    None => None,
                };
                let Some(name) = name else {
                    self.finish_refused(token, Refusal::Protocol);
                    return;
                };
                if name.as_ref() == b"INPUT"
                    && let Some(Operation { kind: OpKind::Type { text, .. }, .. }) = self.op_mut(token)
                {
                    // A person's paste strips CR/LF in single-line inputs. Chromium's
                    // insertText would turn them into spaces before the value sanitizer.
                    let mut pasted = Vec::with_capacity(text.len());
                    for byte in text.iter() {
                        if *byte != b'\r' && *byte != b'\n' {
                            pasted.push(*byte);
                        }
                    }
                    *text = pasted.into_boxed_slice();
                }
                self.issue_step(env, token, b"DOM.focus", &Params::Backend(node), Step::Focus);
            }
            Step::Focus => {
                let detail = self.type_detail(token);
                match detail {
                    Some((text, Entry::Compose)) => self.issue_step(
                        env,
                        token,
                        b"Input.imeSetComposition",
                        &Params::Composition(&text),
                        Step::Insert,
                    ),
                    Some((text, Entry::Insert)) => {
                        self.issue_step(env, token, b"Input.insertText", &Params::Text(&text), Step::Insert);
                    }
                    Some((_, Entry::Keys)) => self.issue_character(env, token, 0, true),
                    None => {}
                }
            }
            Step::Character { offset, down } => self.issue_character(env, token, offset, !down),
            Step::KeyDown => {
                let key = self.key_state(token);
                if let Some((key, shift)) = key {
                    self.issue_key(env, token, key, shift, false);
                }
            }
            Step::Look(step) => self.look_reply(env, token, step, result),
            Step::AttributeRoot | Step::AttributeSelector | Step::AttributeValue => {
                self.attribute_reply(env, token, step, result);
            }
            Step::Address => {
                let current = field_i64(result, b"currentIndex");
                let entries = field_array(result, b"entries");
                let (Some(current), Some(entries)) = (current, entries) else {
                    self.finish_refused(token, Refusal::Protocol);
                    return;
                };
                let mut url = None;
                for (index, entry) in entries.enumerate() {
                    if i64::try_from(index).ok() == Some(current) {
                        url = field_text(entry, b"url");
                        break;
                    }
                }
                let Some(url) = url else {
                    self.finish_refused(token, Refusal::Protocol);
                    return;
                };
                if url.len() > usize::try_from(env.limits.text).expect("text limit fits usize") {
                    self.finish_refused(token, Refusal::Limit);
                } else if self.remove_op(token).is_some() {
                    self.events.push_back(Event::Address { op: token, url });
                }
            }
            Step::Snapshot => {
                let text = snapshot(result, env.limits.snapshot, env.limits.text);
                if self.remove_op(token).is_some() {
                    self.events.push_back(Event::Snapshot { op: token, text });
                }
            }
            Step::Screenshot => {
                let png = match field_text(result, b"data") {
                    Some(base64) => {
                        skein_lib::base64::decode(skein_lib::base64::Alphabet::Standard, &base64, env.limits.screenshot)
                            .ok()
                    }
                    None => None,
                };
                if let Some(png) = png {
                    if self.remove_op(token).is_some() {
                        self.events.push_back(Event::Screenshot { op: token, png });
                    }
                } else {
                    self.finish_refused(token, Refusal::Limit);
                }
            }
        }
    }

    fn next_find_box(&mut self, env: &Env<Limits>, token: Token) {
        let node = if let Some(Operation { kind: OpKind::Find { seen, next, .. }, .. }) = self.op(token) {
            match seen.get(*next) {
                Some(seen) => Some(seen.node),
                None => None,
            }
        } else {
            None
        };
        if let Some(node) = node {
            self.issue_step(env, token, b"DOM.getContentQuads", &Params::Quads(node), Step::Box);
        } else {
            self.finish_find(env, token);
        }
    }

    fn finish_find(&mut self, env: &Env<Limits>, token: Token) {
        let Some(mut op) = self.remove_op(token) else {
            return;
        };
        let OpKind::Find { query, expect, seen, more, .. } = op.kind else {
            return;
        };
        match expect {
            None => self.events.push_back(Event::Found { op: token, seen, more }),
            Some((expect, until)) => {
                let count = seen.len().saturating_add(more);
                let met = match expect {
                    Expect::Present => count > 0,
                    Expect::Count(wanted) => count == wanted,
                    Expect::Absent => count == 0,
                };
                if met {
                    self.events.push_back(Event::Met { op: token, seen });
                } else if env.now >= until {
                    self.events.push_back(Event::Missed { op: token, seen, more });
                } else {
                    op.kind = OpKind::Find {
                        query,
                        expect: Some((expect, until)),
                        seen: List::with_capacity(env.limits.matches),
                        more: 0,
                        next: 0,
                    };
                    op.due = Some(env.now.saturating_add(env.limits.poll).min(until));
                    let accepted = self.insert_op(op, env.limits.ops);
                    assert!(accepted, "the same operation's slot is reusable");
                }
            }
        }
    }

    fn mouse(&mut self, env: &Env<Limits>, token: Token, step: Step) {
        let at = if let Some(Operation { kind: OpKind::Press { at, .. }, .. }) = self.op(token) { *at } else { None };
        let Some((x, y)) = at else {
            self.finish_refused(token, Refusal::Protocol);
            return;
        };
        let (kind, pressed) = match step {
            Step::MouseMove => (b"mouseMoved".as_slice(), false),
            Step::MouseDown => (b"mousePressed".as_slice(), true),
            Step::MouseUp => (b"mouseReleased".as_slice(), false),
            Step::History
            | Step::Go
            | Step::Query
            | Step::Box
            | Step::Scroll
            | Step::Accessibility
            | Step::Quads
            | Step::Metrics
            | Step::Hit
            | Step::Describe
            | Step::TypeNode { .. }
            | Step::Focus
            | Step::Insert
            | Step::KeyDown
            | Step::KeyUp
            | Step::Character { .. }
            | Step::Look(_)
            | Step::AttributeRoot
            | Step::AttributeSelector
            | Step::AttributeValue
            | Step::Address
            | Step::Snapshot
            | Step::Screenshot => unreachable!("only mouse steps reach mouse"),
        };
        self.issue_step(env, token, b"Input.dispatchMouseEvent", &Params::Mouse { kind, x, y, pressed }, step);
    }
}

mod attribute;
mod look;

#[cfg(test)]
#[path = "browser/type_tests.rs"]
mod type_tests;

#[cfg(test)]
mod download_tests;

mod download;
