//! Hosted fake LLM process (fake-llm.md, sections 2 and 3). It keeps the
//! script domain and byte routing, socket bindings and outside observations;
//! it never sees client state or application effects. `new` listens,
//! `Host::iterate` drives progress, and `shutdown` drains actual settlement.

use std::mem::size_of;
use std::net::SocketAddr;

use skein_fake_llm_domain::{self as domain, api};
use skein_fake_llm_protocol::provider;
use skein_io::{self as io, kernel};
use skein_lib::stream::{Down, Up};
use skein_lib::{Env, Queue, Time, Token, Wall};
use skein_llm::Credential;
use skein_shell::Host;

use crate::face::{self, Face, LISTENER};
use crate::transport::{self, Wire};
use crate::{Error, Limits, Transport};

/// An independent peer observation emitted to the world's referee.
#[derive(Debug, PartialEq, Eq)]
pub enum Observation {
    /// The peer accepted this connection from a client.
    Accepted { connection: Token },
    /// HTTP fields without credentials received before a query on this connection.
    Head { connection: Token, headers: Box<[skein_http::Header]> },
    /// The route, scheme and accepted index presented by this request, without bytes.
    Credential { connection: Token, route: skein_llm::Provider, scheme: Option<skein_llm::Scheme>, index: Option<u32> },
    /// The byte peer decoded this complete call before the script domain received it.
    Query { connection: Token, query: api::Query },
    /// The real script domain emitted this terminal, even after the connection closed.
    Answered { connection: Option<Token>, result: Result<(), api::Error> },
    /// io completed this connection's close.
    Closed { connection: Token },
}

struct Connection {
    owner: Token,
    wire: Wire,
    server: provider::Server,
    above: Queue<provider::Event>,
    below: Queue<Down>,
    plain: Queue<Up>,
    closing: bool,
}

/// A world's fake LLM process, configured with its real script domain and wire peer.
pub struct Peer {
    face: Face,
    transport: Transport,
    service: provider::Service,
    credentials: Vec<Credential>,
    protocol_limits: provider::Limits,
    domain: domain::Domain,
    domain_limits: domain::Config,
    replies: Queue<domain::Request>,
    releases: Queue<Box<[u8]>>,
    connections: Vec<Connection>,
    next: u64,
    observations: Vec<Observation>,
    observed_bytes: u64,
    recording: bool,
    ignore_half_close: bool,
}

impl Peer {
    /// Adopts a held listener before the first drive, so the face's queued
    /// Listen claims its address. On refusal the caller keeps the descriptor.
    pub fn adopt_listener(&mut self, fd: kernel::Fd, addr: SocketAddr) -> Result<(), kernel::Fd> {
        self.face.io.adopt_listener(fd, addr).map(|_| ())
    }

    /// Starts loopback admission with caller-supplied seeded script and fault configuration.
    #[expect(clippy::too_many_arguments, reason = "independent machine configurations stay explicit")]
    pub fn new(
        address: SocketAddr,
        transport: Transport,
        limits: Limits,
        config: provider::Config,
        credential: Credential,
        protocol_limits: provider::Limits,
        domain: domain::Domain,
        domain_limits: domain::Config,
    ) -> Result<Self, Error> {
        provider::scripts(&config, core::slice::from_ref(&credential), domain.scripts()).map_err(|_| Error::Limits)?;
        transport::validate(&limits, transport)?;
        if !valid(&limits, &protocol_limits) {
            return Err(Error::Limits);
        }
        if credential.value.len().checked_add(credential.account_id.len()).ok_or(Error::Limits)?
            > usize::try_from(protocol_limits.http.head).expect("u32 fits")
        {
            return Err(Error::Limits);
        }
        let mut credentials = Vec::with_capacity(usize::try_from(provider::MAX_CREDENTIALS).expect("u32 fits"));
        credentials.push(credential);
        Ok(Self {
            face: Face::new(address, limits)?,
            transport,
            service: provider::Service::new(config, &protocol_limits).map_err(|_| Error::Limits)?,
            credentials,
            protocol_limits,
            domain,
            domain_limits,
            replies: Queue::with_capacity(limits.queue),
            releases: Queue::with_capacity(domain_limits.calls),
            connections: Vec::with_capacity(usize::try_from(limits.connections).expect("u32 fits")),
            next: 1,
            observations: Vec::with_capacity(usize::try_from(limits.observations).expect("u32 fits")),
            observed_bytes: 0,
            recording: true,
            ignore_half_close: false,
        })
    }

    /// Adds a credential before the first request, returning its accepted index.
    ///
    /// # Errors
    /// Returns `Limits` after serving, for an incompatible list or script, or past its bounds.
    pub fn accept(&mut self, credential: Credential) -> Result<u32, Error> {
        let index = u32::try_from(self.credentials.len()).map_err(|_| Error::Limits)?;
        let mut bytes = credential.value.len().checked_add(credential.account_id.len()).ok_or(Error::Limits)?;
        for accepted in &self.credentials {
            bytes = bytes
                .checked_add(accepted.value.len())
                .ok_or(Error::Limits)?
                .checked_add(accepted.account_id.len())
                .ok_or(Error::Limits)?;
        }
        if index == provider::MAX_CREDENTIALS
            || bytes > usize::try_from(self.protocol_limits.http.head).expect("u32 fits")
        {
            return Err(Error::Limits);
        }
        self.credentials.push(credential);
        if self.service.validate_credentials(&self.credentials, self.domain.scripts()).is_err() {
            self.credentials.pop();
            return Err(Error::Limits);
        }
        Ok(index)
    }

    /// The actual bound address, available after io's Listening event.
    #[must_use]
    pub fn address(&self) -> Option<SocketAddr> {
        self.face.address
    }

    /// Keeps no outside records. Call on a fresh peer before its first pass.
    #[must_use]
    pub fn without_records(mut self) -> Self {
        self.face.limits.observations = 0;
        self.face.limits.observation_bytes = 0;
        self.observations = Vec::new();
        self.observed_bytes = 0;
        self.recording = false;
        self
    }
    /// Frozen outside records retained under the world's observation caps.
    #[must_use]
    pub fn observations(&self) -> &[Observation] {
        &self.observations
    }

    /// Ends a peer whose clients have settled, retaining accepted work’s terminals.
    /// A caller ending it earlier names its scenario about peer hang-up.
    pub fn shutdown(&mut self) {
        self.face.shutdown();
    }

    /// Keep the byte peer live when the client finishes its write side.
    pub fn ignore_half_close(&mut self) {
        self.ignore_half_close = true;
    }

    /// Queues a hold name for the next domain pass, without releasing from the wire.
    pub fn release(&mut self, name: &[u8]) -> Result<(), Error> {
        if name.is_empty() || name.len() > 64 || self.releases.room() == 0 {
            return Err(Error::Limits);
        }
        self.releases.push(name.into());
        Ok(())
    }

    fn reserve_observation(&self, bytes: u64) {
        assert!(
            self.observations.len() < usize::try_from(self.face.limits.observations).expect("u32 fits"),
            "the world's referee observation count fits its configured cap"
        );
        assert!(
            self.observed_bytes
                .checked_add(bytes)
                .is_some_and(|total| total <= u64::from(self.face.limits.observation_bytes)),
            "observation admitted before copying"
        );
    }

    fn observe(&mut self, observation: Observation, bytes: u64) {
        if !self.recording {
            return;
        }
        self.reserve_observation(bytes);
        self.observed_bytes = self.observed_bytes.checked_add(bytes).expect("bounded observation bytes");
        self.observations.push(observation);
    }

    fn event(&mut self, event: io::Event, now: Time, wall: Wall) {
        match event {
            io::Event::Listening { owner, listener, addr } => {
                assert_eq!(owner, LISTENER, "one listener");
                self.face.listener = Some(listener);
                let addr = addr.socket().expect("fake peers listen on socket addresses");
                self.face.address = Some(addr);
            }
            io::Event::Accepted { owner, socket, .. } => {
                assert_eq!(owner, LISTENER, "one listener accepts");
                if self.face.stopping
                    || self.connections.len() == usize::try_from(self.face.limits.connections).expect("u32 fits")
                {
                    self.face.requests.push(io::Request::Reject { socket });
                    return;
                }
                let owner = Token::new(self.next);
                self.next = self
                    .next
                    .checked_add(1)
                    .filter(|next| *next < u64::MAX)
                    .expect("finite world connection identities");
                self.face.requests.push(io::Request::Bind { socket, owner });
                let limits = self.face.limits;
                let mut connection = Connection {
                    owner,
                    wire: Wire::new(socket, self.transport, limits),
                    server: provider::Server::new(owner, &self.protocol_limits).expect("validated peer limits"),
                    above: Queue::with_capacity(limits.queue),
                    below: Queue::with_capacity(limits.queue),
                    plain: Queue::with_capacity(limits.queue),
                    closing: false,
                };
                provider::start(
                    &mut connection.server,
                    &mut self.service,
                    &self.credentials,
                    &Env { now, wall, limits: self.protocol_limits },
                    &mut connection.above,
                    &mut connection.below,
                );
                self.connections.push(connection);
                self.observe(Observation::Accepted { connection: owner }, 0);
            }
            io::Event::Output { owner, up } => {
                if let Some(connection) = self.connections.iter_mut().find(|connection| connection.owner == owner) {
                    connection.wire.output(up, &mut connection.plain, &mut self.face.requests);
                }
            }
            io::Event::Stream { owner, up } => {
                if let Some(connection) = self.connections.iter_mut().find(|connection| connection.owner == owner) {
                    connection.wire.up(up, &mut connection.plain, &mut self.face.requests);
                }
            }
            io::Event::Closed { owner } => {
                if owner == LISTENER {
                    return;
                }
                if let Some(index) = self.connections.iter().position(|connection| connection.owner == owner) {
                    let mut connection = self.connections.remove(index);
                    provider::closed(
                        &mut connection.server,
                        &mut self.service,
                        &Env { now, wall, limits: self.protocol_limits },
                        &mut connection.above,
                        &mut connection.below,
                    );
                    self.observe(Observation::Closed { connection: owner }, 0);
                }
            }
            io::Event::Failed { owner, .. } => {
                assert_ne!(owner, LISTENER, "the fake listener must bind");
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
            | io::Event::Shutdown { .. } => {
                panic!("the fake peer only listens and serves classic byte streams");
            }
        }
    }

    #[expect(clippy::too_many_lines, reason = "bounded peer passes with explicit queue reservations")]
    fn progress(&mut self, now: Time, wall: Wall) {
        let env = Env { now, wall, limits: self.protocol_limits };
        let domain_env = Env { now, wall, limits: self.domain_limits };
        for index in 0..self.connections.len() {
            if self.face.requests.room() < 32 {
                break;
            }
            let connection = &mut self.connections[index];
            if self.face.stopping {
                provider::close(
                    &mut connection.server,
                    &mut self.service,
                    &env,
                    &mut connection.above,
                    &mut connection.below,
                );
                connection.closing = true;
            }
            if connection.above.room() >= provider::MAX_UP && connection.below.room() >= provider::MAX_DOWN {
                if let Some(up) = connection.plain.pop() {
                    if self.ignore_half_close && up == Up::End {
                        continue;
                    }
                    provider::up(
                        &mut connection.server,
                        &mut self.service,
                        &self.credentials,
                        &env,
                        up,
                        &mut connection.above,
                        &mut connection.below,
                    );
                } else if connection.server.has_work() {
                    provider::resume(
                        &mut connection.server,
                        &mut self.service,
                        &self.credentials,
                        &env,
                        &mut connection.above,
                        &mut connection.below,
                    );
                }
            }
            if connection.plain.room() >= 3 {
                if let Some(down) = connection.below.pop() {
                    connection.wire.down(down, &mut connection.plain, &mut self.face.requests);
                }
                connection.wire.pump(&mut connection.plain, &mut self.face.requests);
            }
            if self.replies.room() >= domain::MAX_OUT
                && let Some(event) = connection.above.pop()
            {
                match event {
                    provider::Event::Head { headers, presented } => {
                        let connection = connection.owner;
                        if self.recording {
                            let bytes = provider::head_bytes(&headers).expect("bounded actual head");
                            self.reserve_observation(bytes);
                            self.observe(Observation::Head { connection, headers }, bytes);
                            self.observe(
                                Observation::Credential {
                                    connection,
                                    route: presented.route,
                                    scheme: presented.scheme,
                                    index: presented.index,
                                },
                                0,
                            );
                        }
                    }
                    provider::Event::Domain(domain::Event::Call { reply_to, call, query }) => {
                        let connection = connection.owner;
                        if self.recording {
                            let bytes = query_bytes(&query).expect("bounded decoded query");
                            // Check before cloning any owned query bytes.
                            self.reserve_observation(bytes);
                            self.observe(Observation::Query { connection, query: query.clone() }, bytes);
                        }
                        domain::step(
                            &mut self.domain,
                            &domain_env,
                            domain::Event::Call { reply_to, call, query },
                            &mut self.replies,
                        );
                    }
                    provider::Event::Domain(domain::Event::Release { .. }) => {
                        unreachable!("the wire never releases a hold")
                    }
                    provider::Event::Close => self.connections[index].closing = true,
                    provider::Event::Closed => {}
                }
            }
        }
        for connection in &mut self.connections {
            if connection.closing && connection.below.is_empty() && self.face.requests.room() > 0 {
                connection.wire.close(&mut self.face.requests);
            }
        }
        while self.replies.room() >= domain::MAX_OUT {
            let Some(name) = self.releases.pop() else { break };
            domain::step(&mut self.domain, &domain_env, domain::Event::Release { name }, &mut self.replies);
        }
        if self.domain.is_due(now) && self.replies.room() >= domain::MAX_OUT {
            domain::fire(&mut self.domain, &domain_env, &mut self.replies);
        }
        if let Some(reply) = self.replies.pop() {
            let (owner, reply) = match self.service.target(reply) {
                Ok((owner, reply)) => (Some(owner), reply),
                Err(reply) => (None, reply),
            };
            match &reply {
                domain::Request::Reply { result, .. } => self.observe(
                    Observation::Answered {
                        connection: owner,
                        result: result.as_ref().map(|_| ()).map_err(|failure| failure.error),
                    },
                    0,
                ),
                domain::Request::Piece { .. } | domain::Request::Hold { .. } => {}
            }
            if let Some(owner) = owner {
                let connection = self
                    .connections
                    .iter_mut()
                    .find(|connection| connection.owner == owner)
                    .expect("active service route has a socket");
                assert!(
                    connection.above.room() >= provider::MAX_UP && connection.below.room() >= provider::MAX_DOWN,
                    "owner reserved the down entrance"
                );
                provider::down(
                    &mut connection.server,
                    &mut self.service,
                    &self.credentials,
                    &env,
                    reply,
                    &mut connection.above,
                    &mut connection.below,
                );
            }
        }
    }
}

impl Host for Peer {
    fn iterate(&mut self, now: Time, wall: Wall) {
        self.face.up(now, wall);
        for _ in 0..self.face.limits.queue {
            if self.face.requests.room() < 32 {
                break;
            }
            let Some(event) = self.face.events.pop() else { break };
            self.event(event, now, wall);
        }
        self.face.close_listener();
        self.progress(now, wall);
        self.face.down(now, wall);
        self.service.reclaim();
        self.domain.reclaim();
    }
    fn completions(&mut self) -> &mut Queue<kernel::Complete> {
        &mut self.face.completions
    }
    fn submissions(&mut self) -> &mut Queue<kernel::Submit> {
        &mut self.face.submissions
    }
    fn work_pending(&self, now: Time) -> bool {
        self.face.work_pending()
            || self.domain.is_due(now)
            || !self.replies.is_empty()
            || !self.releases.is_empty()
            || self.connections.iter().any(|connection| {
                connection.server.has_work()
                    || !connection.above.is_empty()
                    || !connection.below.is_empty()
                    || !connection.plain.is_empty()
                    || connection.wire.work_pending()
            })
    }
    fn next_deadline(&self) -> Option<Time> {
        [self.face.io.next_deadline(), self.domain.next_deadline()].into_iter().flatten().min()
    }

    fn next_policy_deadline(&self) -> Option<Time> {
        self.domain.next_deadline()
    }

    fn is_empty(&self) -> bool {
        self.face.is_empty()
            && self.connections.is_empty()
            && self.replies.is_empty()
            && self.releases.is_empty()
            && self.domain.calls() == 0
            && self.service.calls() == 0
    }
    fn operations(&self) -> u32 {
        io::operations(&self.face.limits.io).expect("validated io operations")
    }
}

/// Whether these limits are representable: every edge bound they size is finite
/// and every capacity they derive fits (programming-model.md, 6.3).
/// The provider's HTTP read and room demands fit the transport buffers.
#[must_use]
pub fn valid(limits: &Limits, peer: &provider::Limits) -> bool {
    face::valid(limits)
        && provider::valid(peer)
        && skein_http::server::largest_read(&peer.http) <= limits.plaintext
        && limits.ciphertext.checked_sub(1024).is_some_and(|room| skein_http::server::largest_room(&peer.http) <= room)
}

fn query_bytes(query: &api::Query) -> Option<u64> {
    let mut bytes = u64::try_from(query.model.len().checked_add(query.system.len())?).ok()?;
    bytes = bytes
        .checked_add(u64::try_from(query.messages.len().checked_mul(size_of::<api::Message>())?).ok()?)?
        .checked_add(u64::try_from(query.tools.len().checked_mul(size_of::<api::ToolSpec>())?).ok()?)?;
    match &query.choice {
        api::ToolChoice::Auto | api::ToolChoice::None => {}
        api::ToolChoice::Only(names) => {
            bytes = bytes.checked_add(u64::try_from(names.len().checked_mul(size_of::<Box<[u8]>>())?).ok()?)?;
            for name in names {
                bytes = bytes.checked_add(u64::try_from(name.len()).ok()?)?;
            }
        }
    }
    if let api::Caching::Marks(marks) = &query.caching {
        bytes = bytes.checked_add(u64::try_from(marks.len().checked_mul(size_of::<api::Mark>())?).ok()?)?;
    }
    for tool in &query.tools {
        bytes = bytes.checked_add(
            u64::try_from(tool.name.len().checked_add(tool.description.len())?.checked_add(tool.parameters.len())?)
                .ok()?,
        )?;
    }
    for message in &query.messages {
        bytes = bytes.checked_add(u64::try_from(message.parts.len().checked_mul(size_of::<api::Part>())?).ok()?)?;
        for part in &message.parts {
            let size = match part {
                api::Part::Text { text } | api::Part::Opaque { bytes: text } => text.len(),
                api::Part::ToolCall { id, name, arguments } => {
                    id.len().checked_add(name.len())?.checked_add(arguments.len())?
                }
                api::Part::ToolOutput { id, output, .. } => id.len().checked_add(output.len())?,
            };
            bytes = bytes.checked_add(u64::try_from(size).ok()?)?;
        }
    }
    Some(bytes)
}
