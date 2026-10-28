//! Loopback webhook listener and its bounded connections (forge-connection.md, 5, 7 and 8).
use crate::{Event, Limits};
use skein_forge::api;
use skein_io::Addr;
use skein_lib::{Env, Id, Map, Queue, Slab, Time, Token};
pub(crate) mod connection;
use connection::{Closing, Hooked};
/// The listener; each forge's webhook secret is in its access.
#[derive(Clone, Debug)]
pub struct Webhooks {
    pub listener: Addr,
}
/// A delivery hint, naming its webhook or log-read connection for refusal.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Hinted {
    pub connection: Token,
    pub hint: api::Hint,
}
const OWNER: Token = Token::new(1_u64 << 59_u32);
#[derive(Debug)]
pub(crate) struct Hooks {
    listener: Option<Token>,
    waiting: Option<(Token, Time)>,
    connections: Map<Token, Id<Hooked>>,
    slab: Slab<Hooked>,
    next: u64,
    live: bool,
    closing: bool,
    close_sent: bool,
    wake: Option<Time>,
}
impl Hooks {
    pub fn new(l: &Limits) -> Self {
        Self {
            listener: None,
            waiting: None,
            connections: Map::with_capacity(l.hooks),
            slab: Slab::with_capacity(l.hooks),
            next: OWNER.raw().saturating_add(1),
            live: false,
            closing: false,
            close_sent: false,
            wake: None,
        }
    }
    pub fn begin(&mut self, settings: &Webhooks, io: &mut Queue<skein_io::Request>) {
        self.live = true;
        io.push(skein_io::Request::Listen { owner: OWNER, addr: settings.listener });
    }
    pub fn owns(&self, owner: Token) -> bool {
        owner == OWNER || self.connections.contains_key(&owner)
    }
    pub fn up(&mut self, env: &Env<Limits>, event: skein_io::Event, up: &mut Queue<Event>) {
        match event {
            skein_io::Event::Listening { listener, addr, .. } => {
                self.listener = Some(listener);
                if !self.closing {
                    up.push(Event::Listening { addr });
                }
            }
            skein_io::Event::Accepted { socket, .. } => self.waiting = Some((socket, env.now)),
            skein_io::Event::Stream { owner, up } => {
                if let Some(id) = self.connections.get(&owner) {
                    self.slab.get_mut(*id).expect("indexed hook").stream(env, up);
                }
            }
            skein_io::Event::Failed { owner, .. } => {
                if owner == OWNER {
                    if !self.closing {
                        up.push(Event::ListenerLost);
                    }
                    self.closing = true;
                } else if let Some(id) = self.connections.get(&owner) {
                    self.slab.get_mut(*id).expect("indexed hook").closing = Closing::Pending;
                }
            }
            skein_io::Event::Closed { owner } => {
                if owner == OWNER {
                    self.listener = None;
                    self.live = false;
                } else if let Some(id) = self.connections.remove(&owner) {
                    self.slab.retire(id);
                }
            }
            skein_io::Event::Connecting { .. }
            | skein_io::Event::Connected { .. }
            | skein_io::Event::Output { .. }
            | skein_io::Event::Spawned { .. }
            | skein_io::Event::Exited { .. }
            | skein_io::Event::Usage { .. }
            | skein_io::Event::Resized
            | skein_io::Event::Window { .. }
            | skein_io::Event::Shutdown { .. } => {}
        }
    }
    pub fn refused(&mut self, hinted: Hinted) {
        if let Some(id) = self.connections.get(&hinted.connection) {
            self.slab.get_mut(*id).expect("indexed hook").refused(hinted.hint);
        }
    }
    pub fn offer(&mut self) {
        for (_, id) in &self.connections {
            self.slab.get_mut(*id).expect("indexed hook").offer();
        }
    }
    pub fn close(&mut self) {
        self.closing = true;
    }
    pub fn closed(&self) -> bool {
        !self.live && self.connections.is_empty() && self.waiting.is_none()
    }
    pub fn deadline(&self) -> Option<Time> {
        let mut earliest = self.wake;
        for (_, id) in &self.connections {
            let c = self.slab.get(*id).expect("indexed hook");
            if c.closing != Closing::Sent && (earliest.is_none() || Some(c.deadline) < earliest) {
                earliest = Some(c.deadline);
            }
        }
        earliest
    }
    pub fn drive(
        &mut self,
        env: &Env<Limits>,
        settings: &crate::settings::Settings,
        up: &mut Queue<Event>,
        io: &mut Queue<skein_io::Request>,
    ) {
        self.wake = None;
        if self.closing {
            if let Some(listener) = self.listener
                && !self.close_sent
            {
                io.push(skein_io::Request::Close { entity: listener });
                self.close_sent = true;
            }
            if let Some((socket, _)) = self.waiting.take() {
                io.push(skein_io::Request::Reject { socket });
            }
        } else if self.connections.len() < self.connections.capacity()
            && let Some((socket, accepted)) = self.waiting.take()
        {
            let owner = Token::new(self.next);
            self.next = self.next.checked_add(1).expect("hook token space");
            assert!(self.next < 1_u64 << 60_u32, "component owner prefix");
            self.slab.reclaim();
            let mut c = Hooked::new(socket, env);
            c.deadline = accepted.saturating_add(env.limits.hook_deadline);
            let id = self.slab.insert(c).expect("connection room");
            let _previous = self.connections.insert(owner, id).expect("connection index room");
            io.push(skein_io::Request::Bind { socket, owner });
        }
        for (owner, id) in &self.connections {
            let c = self.slab.get_mut(*id).expect("indexed hook");
            if c.closing != Closing::Sent && (self.closing || c.deadline <= env.now) {
                c.closing = Closing::Sent;
                io.push(skein_io::Request::Abort { entity: c.socket });
            }
            if c.closing != Closing::Sent {
                c.pump(*owner, env, settings, up, io);
                if c.pending() {
                    self.wake = Some(env.now);
                }
            }
        }
    }
}

/// Fixed connection slots and their owner index, priced with the listener.
pub(crate) fn storage(connections: u32) -> Option<u64> {
    Slab::<Hooked>::worst_case(connections)?.checked_add(Map::<Token, Id<Hooked>>::worst_case(connections)?)
}
