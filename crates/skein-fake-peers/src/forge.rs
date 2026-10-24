//! A loopback Forgejo or GitHub peer over one owner's shared forge (fake-forge.md, 5).
use std::{cell::RefCell, net::SocketAddr, rc::Rc};

use self::layer as protocol;
use skein_fake_forge_domain::{self as domain, api};
use skein_fake_forge_protocol as forgejo;
use skein_io::{self as io, kernel};
use skein_lib::stream::{Down, Up};
use skein_lib::{Env, Queue, ReplyTo, Time, Token, Wall};
use skein_shell::Host;

use crate::face::{Face, LISTENER};
use crate::transport::{self, Wire};
use crate::{Error, Limits, Transport};

const BLOB_LISTENER: Token = Token::new(u64::MAX - 1);

mod delivery;
mod github_delivery;
mod layer;
mod observe;
mod shared;

/// The owner's forge, borrowed by the peer and another fake for separate passes.
#[derive(Debug)]
pub struct Forge {
    pub domain: domain::Domain,
    pub config: domain::Config,
    pub now: Time,
    /// Replies not yet consumed by their owner, and hooks awaiting routing.
    pub pending: Queue<domain::Request>,
    pub hooks: Option<forgejo::delivery::Endpoint>,
    serial: u64,
}

/// Frozen outside records, retaining values rather than reply rights.
#[expect(
    clippy::large_enum_variant,
    reason = "bounded observation slots retain inline domain replies; shared::bound counts each full slot"
)]
#[derive(Debug, PartialEq, Eq)]
pub enum Observation {
    Connection {
        connection: Token,
        open: bool,
    },
    /// Every case-insensitive Authorization field has an empty value.
    Head {
        connection: Token,
        call: skein_http::server::Call,
    },
    Call {
        connection: Option<Token>,
        user: u64,
        repository: Box<[u8]>,
        op: api::Op,
        result: Result<api::Answer, api::Error>,
    },
    Delivery {
        post: Option<forgejo::delivery::Post>,
        outcome: Delivery,
    },
}

/// The kinds of outside records a peer keeps (fake-forge.md, section 5).
#[expect(clippy::struct_excessive_bools, reason = "each outside record kind is kept independently (fake-forge.md, 5)")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kept {
    pub connections: bool,
    pub heads: bool,
    pub calls: bool,
    pub deliveries: bool,
}
impl Kept {
    pub const EVERY: Self = Self { connections: true, heads: true, calls: true, deliveries: true };
    pub const NONE: Self = Self { connections: false, heads: false, calls: false, deliveries: false };
}

/// One best-effort write terminal, independent of the recipient's verdict.
#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    Written,
    NoAddress,
    NoRoom,
    Refused,
    Reset,
}

struct Connection {
    owner: Token,
    wire: Wire,
    server: protocol::Server,
    above: Queue<protocol::Event>,
    below: Queue<Down>,
    plain: Queue<Up>,
    closing: bool,
    head_seen: bool,
}
struct Call {
    token: Token,
    local: Token,
    connection: Token,
    user: u64,
    snapshot: Option<(Box<[u8]>, api::Op)>,
}

/// Hosted admission, HTTP routing and settlement over the owner's shared forge.
pub struct Peer {
    face: Face,
    transport: Transport,
    listen_address: SocketAddr,
    fresh: bool,
    blob_listener: Option<Token>,
    blob_address: Option<SocketAddr>,
    forge: Rc<RefCell<Forge>>,
    service: protocol::Service,
    protocol: protocol::Limits,
    protocol_bound: u64,
    connections: Vec<Connection>,
    calls: Vec<Call>,
    deliveries: Vec<delivery::Outbound>,
    endpoint: Option<bool>,
    github_deliveries: Vec<github_delivery::Outbound>,
    clock: (Time, Wall),
    next: u64,
    observations: Vec<Observation>,
    observed_bytes: u64,
    kept: Kept,
    bound: u64,
}

impl Peer {
    /// Adopts a held listener before the first drive, so the face's queued
    /// Listen claims its address. On refusal the caller keeps the descriptor.
    pub fn adopt_listener(&mut self, fd: kernel::Fd, addr: SocketAddr) -> Result<(), kernel::Fd> {
        self.face.io.adopt_listener(fd, addr).map(|_| ())
    }

    pub fn new(
        address: SocketAddr,
        limits: Limits,
        mut config: forgejo::Config,
        protocol: forgejo::Limits,
        forge: Rc<RefCell<Forge>>,
        seed: u64,
    ) -> Result<Self, Error> {
        // Port 0 may bind any u16 port; reserve its longest spelling at admission.
        let origin = format!("http://{}", SocketAddr::new(address.ip(), u16::MAX));
        if origin.len() > usize::try_from(protocol.documents.target_bytes).map_err(|_| Error::Limits)? {
            return Err(Error::Limits);
        }
        config.hooks = None;
        let protocol_bound = forgejo::worst_case(&protocol, &config).ok_or(Error::Limits)?;
        let service =
            protocol::Service::Forgejo(forgejo::Service::new(config, &protocol, seed).map_err(|_| Error::Limits)?);
        Self::host(address, limits, protocol::Limits::Forgejo(protocol), service, protocol_bound, forge)
    }
    /// GitHub's API and browser pages share this listener and the owner's forge.
    /// Its second listener serves signed, credential-free job logs (fake-forge.md, 10.5).
    pub fn github(
        address: SocketAddr,
        blob: SocketAddr,
        limits: Limits,
        config: skein_fake_forge_github::Config,
        protocol: skein_fake_forge_github::Limits,
        forge: Rc<RefCell<Forge>>,
        seed: u64,
    ) -> Result<Self, Error> {
        // Port 0 may bind any u16 port; reserve its longest spelling at admission.
        let origin = format!("http://{}", SocketAddr::new(address.ip(), u16::MAX));
        let blob_origin = format!("http://{}", SocketAddr::new(blob.ip(), u16::MAX));
        if !blob.ip().is_loopback()
            || blob_origin.len().checked_add(160).ok_or(Error::Limits)?
                > usize::try_from(protocol.github.target_bytes.min(protocol.response)).map_err(|_| Error::Limits)?
            || origin.len() > usize::try_from(protocol.github.target_bytes).map_err(|_| Error::Limits)?
        {
            return Err(Error::Limits);
        }
        let protocol_bound = skein_fake_forge_github::worst_case(&protocol, &config).ok_or(Error::Limits)?;
        let service = protocol::Service::Github(
            skein_fake_forge_github::Service::new(config, &protocol, seed).map_err(|_| Error::Limits)?,
        );
        let mut peer = Self::host(address, limits, protocol::Limits::Github(protocol), service, protocol_bound, forge)?;
        peer.face.requests.push(io::Request::Listen { owner: BLOB_LISTENER, addr: io::Addr::Ip(blob) });
        Ok(peer)
    }
    fn host(
        address: SocketAddr,
        limits: Limits,
        protocol: protocol::Limits,
        service: protocol::Service,
        protocol_bound: u64,
        forge: Rc<RefCell<Forge>>,
    ) -> Result<Self, Error> {
        let bound = shared::bound(&limits, &protocol, protocol_bound, &forge.borrow(), Transport::Plaintext)
            .ok_or(Error::Limits)?;
        let call_capacity = call_capacity(&protocol, &forge.borrow(), limits.connections).ok_or(Error::Limits)?;
        let (forgejo_capacity, github_capacity) = match protocol {
            protocol::Limits::Forgejo(_) => (limits.connections, 0),
            protocol::Limits::Github(_) => (0, limits.connections),
        };
        Ok(Self {
            face: Face::new(address, limits)?,
            transport: Transport::Plaintext,
            listen_address: address,
            fresh: true,
            blob_listener: None,
            blob_address: None,
            forge,
            service,
            protocol,
            protocol_bound,
            connections: Vec::with_capacity(usize::try_from(limits.connections).expect("u32 fits")),
            calls: Vec::with_capacity(usize::try_from(call_capacity).expect("u32 fits")),
            deliveries: Vec::with_capacity(usize::try_from(forgejo_capacity).expect("u32 fits")),
            endpoint: None,
            github_deliveries: Vec::with_capacity(usize::try_from(github_capacity).expect("u32 fits")),
            clock: (Time::ZERO, Wall::EPOCH),
            next: 1,
            observations: Vec::with_capacity(usize::try_from(limits.observations).expect("u32 fits")),
            observed_bytes: 0,
            kept: Kept::EVERY,
            bound,
        })
    }
    /// Serves Forgejo over TLS on Skein's fixed test chain (`ROOT`, for `skein.test`
    /// and `127.0.0.1`); called on a fresh peer before its first pass. Hook
    /// deliveries stay plaintext to loopback.
    pub fn tls(&mut self) -> Result<(), Error> {
        assert!(self.fresh, "TLS is selected before the peer's first pass");
        let protocol::Limits::Forgejo(protocol) = &self.protocol else {
            return Err(Error::Limits);
        };
        transport::validate(&self.face.limits, Transport::Tls)?;
        let origin = format!("https://{}", SocketAddr::new(self.listen_address.ip(), u16::MAX));
        if origin.len() > usize::try_from(protocol.documents.target_bytes).map_err(|_| Error::Limits)? {
            return Err(Error::Limits);
        }
        let bound =
            shared::bound(&self.face.limits, &self.protocol, self.protocol_bound, &self.forge.borrow(), Transport::Tls)
                .ok_or(Error::Limits)?;
        self.transport = Transport::Tls;
        self.bound = bound;
        Ok(())
    }
    /// Keeps only the named kinds of outside records. Call on a fresh peer before its first pass.
    #[must_use]
    pub fn keeping(mut self, kept: Kept) -> Self {
        self.kept = kept;
        self
    }
    /// Keeps no outside records. Call on a fresh peer before its first pass.
    #[must_use]
    pub fn without_records(mut self) -> Self {
        let slots = u64::from(self.face.limits.observations)
            .checked_mul(u64::try_from(core::mem::size_of::<Observation>()).expect("record size"))
            .expect("validated record slots");
        let records = slots.checked_add(u64::from(self.face.limits.observation_bytes)).expect("validated records");
        self.bound = self.bound.checked_sub(records).expect("bound includes records");
        self.face.limits.observations = 0;
        self.face.limits.observation_bytes = 0;
        self.observations = Vec::new();
        self.observed_bytes = 0;
        self.kept = Kept::NONE;
        self
    }
    #[must_use]
    pub fn address(&self) -> Option<SocketAddr> {
        self.face.address
    }
    #[must_use]
    pub fn blob_address(&self) -> Option<SocketAddr> {
        self.blob_address
    }
    /// Recent blob paths and header presence; credential values are never retained.
    #[must_use]
    pub fn blob_records(&self) -> Option<&Queue<skein_fake_forge_github::logs::Record>> {
        match &self.service {
            protocol::Service::Github(service) => Some(service.blob_records()),
            protocol::Service::Forgejo(_) => None,
        }
    }
    #[must_use]
    pub fn observations(&self) -> &[Observation] {
        &self.observations
    }
    /// Reads a GitHub browser manifest retained by the hosted service.
    #[must_use]
    pub fn github_manifest(&self, state: &[u8]) -> Option<&skein_fake_forge_github::manifest::Manifest> {
        match &self.service {
            protocol::Service::Github(service) => service.manifest(state),
            protocol::Service::Forgejo(_) => None,
        }
    }
    /// A scenario renames an installed account through the service's existing store.
    pub fn rename_github_installation(&mut self, id: u64, login: &[u8]) -> bool {
        match &mut self.service {
            protocol::Service::Github(service) => service.installations_mut().rename(id, login),
            protocol::Service::Forgejo(_) => false,
        }
    }
    /// Lends Forgejo's setup store so an owner can seed accounts and tokens before serving.
    /// Panics for a GitHub face, which has no Forgejo setup store.
    pub fn setup_mut(&mut self) -> &mut forgejo::setup::Store {
        match &mut self.service {
            protocol::Service::Forgejo(service) => service.setup_mut(),
            protocol::Service::Github(_) => panic!("GitHub has no Forgejo setup store"),
        }
    }
    pub fn shutdown(&mut self) {
        self.face.shutdown();
    }

    fn github_deadline(&self) -> Option<Time> {
        let visible = match &self.service {
            protocol::Service::Github(service) => service.next_deadline().map(|wall| {
                self.clock.0.saturating_add(skein_lib::Duration::from_nanos(
                    wall.as_nanos().saturating_sub(self.clock.1.as_nanos()),
                ))
            }),
            protocol::Service::Forgejo(_) => None,
        };
        self.github_deliveries.iter().filter_map(github_delivery::deadline).chain(visible).min()
    }

    fn event(&mut self, event: io::Event, now: Time, wall: Wall) {
        let Some(event) = self.delivery_event(event) else { return };
        let Some(event) = self.github_delivery_event(event, now, wall) else { return };
        let env = Env { now, wall, limits: self.protocol };
        match event {
            io::Event::Listening { owner, listener, addr } => {
                if owner == BLOB_LISTENER {
                    self.blob_listener = Some(listener);
                    self.blob_address = Some(addr.socket().expect("loopback socket address"));
                    if let protocol::Service::Github(service) = &mut self.service {
                        let origin = format!("http://{}", self.blob_address.expect("bound blob address"));
                        service.set_blob_origin(origin.into_bytes().into_boxed_slice()).expect("bounded local origin");
                    }
                    return;
                }
                assert_eq!(owner, LISTENER);
                self.face.listener = Some(listener);
                self.face.address = Some(addr.socket().expect("loopback socket address"));
                // The listening event supplies the origin, including a kernel-selected port.
                let base = match self.transport {
                    Transport::Plaintext => format!("http://{}", self.face.address.expect("bound address")),
                    Transport::Tls => format!("https://{}", self.face.address.expect("bound address")),
                };
                match (&mut self.service, &self.protocol) {
                    (protocol::Service::Forgejo(service), protocol::Limits::Forgejo(limits)) => {
                        service.set_base(base.into_bytes().into_boxed_slice(), limits).expect("bounded local origin");
                    }
                    (protocol::Service::Github(service), protocol::Limits::Github(_)) => {
                        service.set_origin(base.into_bytes().into_boxed_slice()).expect("bounded local origin");
                    }
                    (protocol::Service::Forgejo(_), protocol::Limits::Github(_))
                    | (protocol::Service::Github(_), protocol::Limits::Forgejo(_)) => unreachable!("matching dialect"),
                }
            }
            io::Event::Accepted { owner, socket, .. } => {
                assert!(owner == LISTENER || owner == BLOB_LISTENER);
                let blob = owner == BLOB_LISTENER;
                if self.face.stopping
                    || self.connections.len() == usize::try_from(self.face.limits.connections).expect("u32 fits")
                {
                    self.face.requests.push(io::Request::Reject { socket });
                    return;
                }
                let owner = Token::new(self.next);
                self.next =
                    self.next.checked_add(1).filter(|next| *next < u64::MAX).expect("finite connection identities");
                self.face.requests.push(io::Request::Bind { socket, owner });
                let limits = self.face.limits;
                let mut connection = Connection {
                    owner,
                    wire: Wire::new(socket, self.transport, limits),
                    server: if blob {
                        protocol::Server::blob(owner, &self.protocol)
                    } else {
                        protocol::Server::new(owner, &self.protocol)
                    }
                    .expect("validated limits"),
                    above: Queue::with_capacity(limits.queue),
                    below: Queue::with_capacity(limits.queue),
                    plain: Queue::with_capacity(limits.queue),
                    closing: false,
                    head_seen: false,
                };
                protocol::start(
                    &mut connection.server,
                    &mut self.service,
                    &env,
                    &mut connection.above,
                    &mut connection.below,
                );
                self.capture(&mut connection);
                self.connections.push(connection);
                if self.kept.connections {
                    self.observe(Observation::Connection { connection: owner, open: true }, 0);
                }
            }
            io::Event::Stream { owner, up } => {
                if let Some(connection) = self.connections.iter_mut().find(|connection| connection.owner == owner) {
                    connection.wire.up(up, &mut connection.plain, &mut self.face.requests);
                }
            }
            io::Event::Output { owner, up } => {
                if let Some(connection) = self.connections.iter_mut().find(|connection| connection.owner == owner) {
                    connection.wire.output(up, &mut connection.plain, &mut self.face.requests);
                }
            }
            io::Event::Closed { owner } => self.connection_closed(owner, &env),
            io::Event::Failed { owner, .. } => {
                assert!(owner != LISTENER && owner != BLOB_LISTENER, "the loopback listeners bind");
                if let Some(connection) = self.connections.iter_mut().find(|connection| connection.owner == owner) {
                    connection.wire.close(&mut self.face.requests);
                }
            }
            io::Event::Connecting { .. }
            | io::Event::Connected { .. }
            | io::Event::Spawned { .. }
            | io::Event::Exited { .. }
            | io::Event::Usage { .. }
            | io::Event::Resized
            | io::Event::Window { .. }
            | io::Event::Shutdown { .. } => panic!("the forge listens and serves byte streams"),
        }
    }

    fn connection_closed(&mut self, owner: Token, env: &Env<protocol::Limits>) {
        if let Some(index) = self.connections.iter().position(|connection| connection.owner == owner) {
            let mut connection = self.connections.remove(index);
            protocol::closed(
                &mut connection.server,
                &mut self.service,
                env,
                &mut connection.above,
                &mut connection.below,
            );
            self.capture(&mut connection);
            if self.kept.connections {
                self.observe(Observation::Connection { connection: owner, open: false }, 0);
            }
        }
    }

    fn dispatch(&mut self, forge: &mut Forge, env: &Env<domain::Config>, connection: Token, event: domain::Event) {
        let domain::Event::Call { reply_to, user, repository, op } = event;
        let local = reply_to.into_token();
        let token = forge.next_reply();
        assert!(self.calls.len() < self.calls.capacity(), "admitted call snapshots fit");
        self.calls.push(Call {
            token,
            local,
            connection,
            user,
            snapshot: self.kept.calls.then(|| (repository.clone(), op.clone())),
        });
        let Forge { domain, pending, .. } = forge;
        let event = domain::Event::Call { reply_to: ReplyTo::new(token), user, repository, op };
        if self.service.is_github() {
            domain::step_admitted(domain, env, event, pending);
        } else {
            domain::step(domain, env, event, pending);
        }
    }

    fn take_endpoint(&mut self, forge: &mut Forge) {
        if self.service.is_github() {
            return;
        }
        // Endpoint ownership lives in the service only for this pass; refusal leaves it empty.
        self.endpoint = forge.hooks.as_ref().map(|endpoint| !endpoint.host.is_empty() && !endpoint.secret.is_empty());
        let endpoint = if self.endpoint == Some(false) { None } else { forge.hooks.take() };
        let previous = self.service.set_hooks(endpoint);
        assert!(previous.is_err() || previous.expect("admitted endpoint").is_none());
    }

    fn serve_event(
        &mut self,
        forge: &mut Forge,
        domain_env: &Env<domain::Config>,
        connection: &mut Connection,
        event: protocol::Event,
    ) {
        let env = Env { now: domain_env.now, wall: domain_env.wall, limits: self.protocol };
        match event {
            protocol::Event::Domain(event) => {
                self.dispatch(forge, domain_env, connection.owner, event);
            }
            protocol::Event::Rate { key, limit, window } => {
                let result = domain::rate(&mut forge.domain, domain_env, key, limit, window);
                protocol::admitted(
                    &mut connection.server,
                    &mut self.service,
                    &env,
                    result,
                    &mut connection.above,
                    &mut connection.below,
                );
                self.capture(connection);
            }
            protocol::Event::Close => connection.closing = true,
            protocol::Event::Closed => {}
        }
    }
    fn progress(&mut self, now: Time, wall: Wall) {
        self.clock = (now, wall);
        self.github_progress(now, wall);
        let env = Env { now, wall, limits: self.protocol };
        // Moving the Rc, rather than borrowing through self, keeps the one pass borrow explicit.
        let shared = Rc::clone(&self.forge);
        let mut forge = shared.borrow_mut();
        forge.now = now;
        self.take_endpoint(&mut forge);
        let domain_env = Env { now, wall, limits: forge.config };
        for index in 0..self.connections.len() {
            if self.face.requests.room() < 32 {
                break;
            }
            // Taking the connection makes head observation and call admission one owner pass.
            let mut connection = self.connections.remove(index);
            if connection.above.room() >= protocol::MAX_UP && connection.below.room() >= protocol::MAX_DOWN {
                if let Some(up) = connection.plain.pop() {
                    protocol::up_with_domain(
                        &mut connection.server,
                        &mut self.service,
                        forgejo::Hosted { domain: &mut forge.domain, config: &domain_env.limits },
                        &env,
                        up,
                        &mut connection.above,
                        &mut connection.below,
                    );
                    self.capture(&mut connection);
                } else if connection.server.has_work() {
                    protocol::resume_with_domain(
                        &mut connection.server,
                        &mut self.service,
                        forgejo::Hosted { domain: &mut forge.domain, config: &domain_env.limits },
                        &env,
                        &mut connection.above,
                        &mut connection.below,
                    );
                    self.capture(&mut connection);
                }
            }
            if forge.pending.room() >= domain::MAX_OUT
                && let Some(event) = connection.above.pop()
            {
                self.serve_event(&mut forge, &domain_env, &mut connection, event);
            }
            if connection.plain.room() >= 3 {
                if let Some(down) = connection.below.pop() {
                    connection.wire.down(down, &mut connection.plain, &mut self.face.requests);
                }
                connection.wire.pump(&mut connection.plain, &mut self.face.requests);
            }
            // Admission ends first; an accepted request reaches its byte terminal before close.
            let active = self.calls.iter().any(|call| call.connection == connection.owner);
            if self.face.stopping
                && connection.head_seen
                && connection.server.is_settled()
                && !active
                && connection.above.is_empty()
                && connection.below.is_empty()
                && connection.plain.is_empty()
                && !connection.server.has_work()
            {
                protocol::close(
                    &mut connection.server,
                    &mut self.service,
                    &env,
                    &mut connection.above,
                    &mut connection.below,
                );
                self.capture(&mut connection);
            }
            if connection.closing && connection.below.is_empty() {
                connection.wire.close(&mut self.face.requests);
            }
            self.connections.insert(index, connection);
        }
        self.finish_pass(&mut forge, &domain_env, &env);
    }
    fn finish_pass(&mut self, forge: &mut Forge, domain_env: &Env<domain::Config>, env: &Env<protocol::Limits>) {
        if forge.pending.room() >= domain::MAX_OUT {
            let Forge { domain, pending, .. } = &mut *forge;
            domain::fire(domain, domain_env, pending);
        }
        self.route(forge, env);
        self.github_progress(env.now, env.wall);
        forge.domain.reclaim();
        self.service.reclaim();
        if self.service.is_github() {
            return;
        }
        let endpoint = self.service.set_hooks(None).expect("clearing an endpoint is valid");
        if self.endpoint != Some(false) {
            forge.hooks = endpoint;
        }
    }
}

impl Host for Peer {
    fn iterate(&mut self, now: Time, wall: Wall) {
        self.fresh = false;
        let bound =
            shared::bound(&self.face.limits, &self.protocol, self.protocol_bound, &self.forge.borrow(), self.transport)
                .expect("owner updates preserve usable bounds");
        // An owner reducing a limit does not release already allocated capacities.
        self.bound = self.bound.max(bound);
        self.forge.borrow_mut().now = now;
        self.face.up(now, wall);
        for _ in 0..self.face.limits.queue {
            if self.face.requests.room() < 32 {
                break;
            }
            let Some(event) = self.face.events.pop() else { break };
            self.event(event, now, wall);
        }
        self.face.close_listener();
        if self.face.stopping
            && self.face.requests.room() > 0
            && let Some(listener) = self.blob_listener.take()
        {
            self.face.requests.push(io::Request::Close { entity: listener });
        }
        self.progress(now, wall);
        self.face.down(now, wall);
    }
    fn completions(&mut self) -> &mut Queue<kernel::Complete> {
        &mut self.face.completions
    }
    fn submissions(&mut self) -> &mut Queue<kernel::Submit> {
        &mut self.face.submissions
    }
    fn work_pending(&self, now: Time) -> bool {
        self.face.work_pending()
            || (self.face.stopping && self.blob_listener.is_some())
            || self.github_deliveries.iter().any(github_delivery::work)
            || self.forge.borrow().domain.is_due(now)
            || self.connections.iter().any(|connection| {
                connection.server.has_work()
                    || !connection.above.is_empty()
                    || !connection.below.is_empty()
                    || !connection.plain.is_empty()
                    || connection.wire.work_pending()
            })
    }
    fn next_deadline(&self) -> Option<Time> {
        [self.face.io.next_deadline(), self.forge.borrow().domain.next_deadline(), self.github_deadline()]
            .into_iter()
            .flatten()
            .min()
    }
    fn next_policy_deadline(&self) -> Option<Time> {
        [self.forge.borrow().domain.next_deadline(), self.github_deadline()].into_iter().flatten().min()
    }
    fn is_empty(&self) -> bool {
        self.face.is_empty()
            && self.blob_listener.is_none()
            && self.connections.is_empty()
            && self.calls.is_empty()
            && self.deliveries.is_empty()
            && self.github_deliveries.is_empty()
            && self.forge.borrow().domain.calls() == 0
    }
    fn worst_case(&self) -> u64 {
        self.bound
    }
    fn operations(&self) -> u32 {
        io::operations(&self.face.limits.io).expect("validated io operations")
    }
}

fn call_capacity(protocol: &protocol::Limits, forge: &Forge, connections: u32) -> Option<u32> {
    protocol.calls(connections).checked_add(forge.config.limits.calls)
}
/// Checked envelope for machines, pending payloads, connections, snapshots and frozen records.
#[must_use]
pub fn worst_case(limits: &Limits, protocol: &forgejo::Limits, config: &forgejo::Config, forge: &Forge) -> Option<u64> {
    shared::bound(
        limits,
        &protocol::Limits::Forgejo(*protocol),
        forgejo::worst_case(protocol, config)?,
        forge,
        Transport::Plaintext,
    )
}
