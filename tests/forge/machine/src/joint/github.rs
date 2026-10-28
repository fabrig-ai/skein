//! GitHub HTTP peers sharing the joint's domain and cutting pipes.
use super::{Joint, StartCounts};
use crate::pipe::{Cut, Pipe, RequestCounts};
use skein_fake_forge_domain as fake;
use skein_fake_forge_github::service::{Outcome, Request, Service};
use skein_fake_forge_protocol::{Route, Tally};
use skein_forge::github::request::Method;
use skein_http::server as http;
use skein_lib::{
    Env, Queue, ReplyTo, Token,
    stream::{Down, Read, Up},
};
use std::collections::{BTreeMap, VecDeque};

struct Peer {
    pipe: Pipe,
    server: http::Server,
    above: Queue<http::Event>,
    below: Queue<Down>,
    head: Option<http::Call>,
    body: Vec<u8>,
    reply: Vec<u8>,
    offset: usize,
    heads: VecDeque<bool>,
    line_indices: Vec<usize>,
}
impl Peer {
    fn action(&mut self, env: &Env<http::Limits>, request: http::Request) {
        http::down(&mut self.server, env, request, &mut self.above, &mut self.below);
    }
}

pub(super) struct Github {
    service: Service,
    app: skein_fake_forge_github::app::App,
    verified: skein_fake_forge_github::app::Verified,
    peers: BTreeMap<Token, Peer>,
    pub tally: Tally,
}
impl Github {
    pub fn new(seed: u64) -> Self {
        Self {
            service: crate::github::Fake::new(seed).into_service(),
            app: crate::github::fixture_app(),
            verified: skein_fake_forge_github::app::Verified::empty(),
            peers: BTreeMap::new(),
            tally: Tally::new(1),
        }
    }
    pub fn busy(&self) -> bool {
        self.peers.values().any(|peer| peer.pipe.has_work() || !peer.above.is_empty() || !peer.below.is_empty())
    }
    pub fn owners(&self) -> impl Iterator<Item = Token> + '_ {
        self.peers.keys().copied()
    }
    fn env(world: &Joint) -> Env<http::Limits> {
        Env { now: world.env.now, wall: world.env.wall, limits: world.protocol_env.limits.http }
    }
    pub fn tick(&mut self, world: &mut Joint) {
        let env = Self::env(world);
        let owners: Vec<_> = self.owners().collect();
        for owner in owners {
            let mut peer = self.peers.remove(&owner).expect("live peer");
            while let Some(down) = peer.below.pop() {
                peer.pipe.server_down(down);
            }
            if let Some(up) = peer.pipe.server_up() {
                http::up(&mut peer.server, &env, up, &mut peer.above, &mut peer.below);
            }
            self.events(world, &mut peer, &env);
            let up = peer.pipe.client_up();
            self.peers.insert(owner, peer);
            if let Some(up) = up {
                world.up(skein_io::Event::Stream { owner, up });
            }
        }
    }
    fn events(&mut self, world: &mut Joint, peer: &mut Peer, env: &Env<http::Limits>) {
        while let Some(event) = peer.above.pop() {
            match event {
                http::Event::Call(call) => {
                    if peer.heads.pop_front().expect("head follows first byte") {
                        world.starts.served += 1;
                    }
                    let user = call
                        .header(b"Authorization")
                        .and_then(|header| header.strip_prefix(b"Bearer "))
                        .and_then(|bearer| {
                            (self.service.installations().token(bearer, env.wall).is_ok()
                                || self.verified.check(bearer, &self.app, env.wall).is_ok())
                            .then_some(9)
                        });
                    self.tally.count(if user.is_some() { route(&call) } else { None }, user);
                    peer.head = Some(call);
                    peer.body.clear();
                    peer.action(
                        env,
                        http::Request::Body(Down::Demand { read: Read::Arrived { max: env.limits.read }, room: 0 }),
                    );
                }
                http::Event::Body(Up::Bytes(bytes)) => {
                    peer.body.extend_from_slice(&bytes);
                    peer.action(
                        env,
                        http::Request::Body(Down::Demand { read: Read::Arrived { max: env.limits.read }, room: 0 }),
                    );
                }
                http::Event::Body(Up::End) => self.serve(world, peer, env),
                http::Event::Reply(Up::Room) => {
                    let end = (peer.offset + usize::try_from(env.limits.send).expect("send")).min(peer.reply.len());
                    let bytes = Box::from(&peer.reply[peer.offset..end]);
                    peer.offset = end;
                    peer.action(env, http::Request::Reply(Down::Send(bytes)));
                    if end == peer.reply.len() {
                        peer.action(env, http::Request::Reply(Down::Finish));
                    } else {
                        peer.action(
                            env,
                            http::Request::Reply(Down::Demand { read: Read::Nothing, room: env.limits.send }),
                        );
                    }
                }
                http::Event::Done(http::Reuse::Keep) => peer.action(env, http::Request::Next),
                http::Event::Done(http::Reuse::Close)
                | http::Event::Ended
                | http::Event::Failed(_)
                | http::Event::Closed
                | http::Event::Body(Up::Failed(_))
                | http::Event::Reply(Up::Failed(_)) => {}
                http::Event::Body(Up::Room) | http::Event::Reply(Up::Bytes(_) | Up::End) | http::Event::Refused(_) => {
                    panic!("HTTP adapter contract")
                }
            }
        }
    }
    fn serve(&mut self, world: &mut Joint, peer: &mut Peer, env: &Env<http::Limits>) {
        let call = peer.head.take().expect("request head");
        let method = method(call.method);
        let request = Request {
            method,
            target: &call.target,
            authorization: call.header(b"Authorization"),
            version: call.header(b"X-GitHub-Api-Version"),
            agent: call.header(b"User-Agent").is_some(),
            body: &peer.body,
            validator: call.header(b"If-None-Match"),
        };
        let mut outcome = self.service.request(&request, env.wall, ReplyTo::new(Token::new(77)));
        let response = loop {
            outcome = match outcome {
                Outcome::Reply(response) => break response,
                Outcome::Admit(admission) => {
                    self.service.admit(admission, &mut world.domain, &world.fake_env, ReplyTo::new(Token::new(77)))
                }
                Outcome::Call(pending, event) => {
                    let mut out = Queue::uncapped();
                    fake::step(&mut world.domain, &world.fake_env, event, &mut out);
                    // More fires than the fixture's queue and CI can have due at once.
                    for _ in 0..64 {
                        if !out.is_empty() {
                            break;
                        }
                        fake::fire(&mut world.domain, &world.fake_env, &mut out);
                    }
                    let Some(fake::Request::Reply { result, .. }) = out.pop() else {
                        panic!("zero-latency domain reply")
                    };
                    assert!(out.is_empty());
                    self.service.finish(pending, result, ReplyTo::new(Token::new(77)), &world.domain)
                }
            };
        };
        let mut headers = vec![skein_http::Header {
            name: Box::from(b"Content-Type".as_slice()),
            value: Box::from(response.content_type.bytes()),
        }];
        if let Some(link) = response.link {
            headers.push(skein_http::Header { name: Box::from(b"Link".as_slice()), value: link });
        }
        if let Some(location) = response.location {
            headers.push(skein_http::Header { name: Box::from(b"Location".as_slice()), value: location });
        }
        peer.reply = response.body.into_vec();
        peer.offset = 0;
        peer.action(
            env,
            http::Request::Respond(http::Response {
                status: response.status,
                headers: headers.into_boxed_slice(),
                body: http::Body::Length(u64::try_from(peer.reply.len()).expect("reply length")),
                close: false,
            }),
        );
        if !peer.reply.is_empty() {
            peer.action(env, http::Request::Reply(Down::Demand { read: Read::Nothing, room: env.limits.send }));
        }
    }
    pub fn io(&mut self, world: &mut Joint, request: skein_io::Request) {
        match request {
            skein_io::Request::Connect { owner, .. } => {
                let cut =
                    if world.rng.chance(world.cut_chance) { Cut::At(world.rng.between(0, 2048)) } else { Cut::Never };
                let env = Self::env(world);
                let mut peer = Peer {
                    pipe: Pipe::new(world.rng.next_u64(), cut),
                    server: http::Server::new(&env.limits),
                    above: Queue::with_capacity(64),
                    below: Queue::with_capacity(64),
                    head: None,
                    body: Vec::new(),
                    reply: Vec::new(),
                    offset: 0,
                    heads: VecDeque::new(),
                    line_indices: Vec::new(),
                };
                peer.action(&env, http::Request::Next);
                assert!(self.peers.insert(owner, peer).is_none(), "fresh connection");
                world.up(skein_io::Event::Connecting { owner, socket: owner });
                world.up(skein_io::Event::Connected { owner });
            }
            skein_io::Request::Stream { stream, down } => {
                if let Some(peer) = self.peers.get_mut(&stream) {
                    let before = peer.pipe.request_counts().written;
                    peer.pipe.client_down(down);
                    let written = peer.pipe.request_counts().written - before;
                    let startup = world.forges.iter().any(|forge| {
                        matches!(
                            world.component.start(*forge),
                            Some(
                                skein_forge_connection::start::Start::Listing { .. }
                                    | skein_forge_connection::start::Start::Minting { .. }
                                    | skein_forge_connection::start::Start::Identity { .. }
                                    | skein_forge_connection::start::Start::Waiting { .. }
                            )
                        )
                    });
                    for _ in 0..written {
                        peer.heads.push_back(startup);
                        world.starts.written += u64::from(startup);
                    }
                    // Earlier lines are complete; only the last observed line can gain bytes.
                    let mut first = peer.line_indices.len();
                    if peer.line_indices.last().is_some_and(|index| !world.lines[*index].ends_with(b"\r\n")) {
                        first -= 1;
                    }
                    for (at, line) in peer.pipe.request_lines().iter().enumerate().skip(first) {
                        if let Some(index) = peer.line_indices.get(at) {
                            let saved = &mut world.lines[*index];
                            saved.extend_from_slice(&line[saved.len()..]);
                        } else {
                            peer.line_indices.push(world.lines.len());
                            world.lines.push(line.clone());
                        }
                    }
                }
            }
            skein_io::Request::Abort { entity } | skein_io::Request::Close { entity } => self.end(world, entity),
            other @ (skein_io::Request::Listen { .. }
            | skein_io::Request::Bind { .. }
            | skein_io::Request::Reject { .. }
            | skein_io::Request::Output { .. }
            | skein_io::Request::Spawn { .. }
            | skein_io::Request::Signal { .. }
            | skein_io::Request::Usage { .. }
            | skein_io::Request::Window { .. }) => panic!("unexpected GitHub io {other:?}"),
        }
    }
    pub fn end(&mut self, world: &mut Joint, owner: Token) {
        if let Some(mut peer) = self.peers.remove(&owner) {
            peer.pipe.freeze();
            let counts = peer.pipe.request_counts();
            world.closed_counts.written += counts.written;
            world.closed_counts.cut_in_head += counts.cut_in_head;
            if counts.cut_in_head > 0 {
                assert_eq!(counts.cut_in_head, 1, "one head in flight per pipe");
                if peer.heads.pop_front().expect("cut follows first byte") {
                    world.starts.cut_in_head += 1;
                }
            }
            world.up(skein_io::Event::Closed { owner });
        }
    }
    pub fn start_counts(&self, counts: &mut StartCounts) {
        for peer in self.peers.values() {
            if peer.heads.front() == Some(&true) {
                counts.cut_in_head += peer.pipe.request_counts().cut_in_head;
            }
        }
    }
    pub fn request_counts(&self, counts: &mut RequestCounts) {
        for peer in self.peers.values() {
            let requests = peer.pipe.request_counts();
            counts.written += requests.written;
            counts.cut_in_head += requests.cut_in_head;
        }
    }
}

fn method(method: skein_http::Method) -> Method {
    match method {
        skein_http::Method::Get => Method::Get,
        skein_http::Method::Post => Method::Post,
        skein_http::Method::Put => Method::Put,
        skein_http::Method::Patch => Method::Patch,
        skein_http::Method::Delete => Method::Delete,
        skein_http::Method::Head | skein_http::Method::Options => panic!("API method"),
    }
}

// Tally's shared route names cover both dialects' repository operations.
// App, delivery-log and GraphQL entrances have no Forgejo route name.
fn route(call: &http::Call) -> Option<Route> {
    use skein_forge::github::request::Request as R;
    let documents = crate::fixtures::fake_limits().documents;
    let limits = skein_forge::github::limits::Limits {
        json: documents.json,
        writer: documents.writer,
        string_bytes: documents.string_bytes,
        body_bytes: documents.body_bytes,
        inner: documents.inner,
        target_bytes: documents.target_bytes,
    };
    Some(match skein_forge::github::server::route(method(call.method), &call.target, &limits)? {
        R::Reachable { .. } => Route::UserRepos,
        R::Viewer => Route::User,
        R::Merge { .. } => Route::Merge,
        R::BotUser { .. } | R::User { .. } => Route::UserOf,
        R::Repository { .. } => Route::RepositoryById,
        R::Locate { .. } => Route::Repository,
        R::Issues { .. } => Route::Issues,
        R::Issue { .. } => Route::Issue,
        R::Comments { .. } => Route::Comments,
        R::Pull { .. } => Route::Pull,
        R::PullsFor { .. } => Route::PullFor,
        R::Reviews { .. } => Route::Reviews,
        R::ReviewComments { .. } => Route::ReviewComments,
        R::CombinedStatus { .. } => Route::CombinedStatus,
        R::CheckRuns { .. } => Route::Runs,
        R::Job { .. } => Route::Job,
        R::JobLog { .. } => Route::JobLog,
        R::Branch { .. } => Route::Branch,
        R::Branches { .. } => Route::Branches,
        R::PullFiles { .. } => Route::PullFiles,
        R::Compare { .. } => Route::Compare,
        R::Raw { .. } => Route::Raw,
        R::RulesForBranch { .. } | R::ClassicProtection { .. } => Route::Protection,
        R::Ruleset { .. } => Route::Protections,
        R::Collaborators { .. } => Route::Collaborators,
        R::Permission { .. } => Route::Permission,
        R::Reopen { .. } | R::Edit { .. } | R::Close { .. } => Route::EditIssue,
        R::Status { .. } => Route::PostStatus,
        R::CreateBranch { .. } => Route::CreateBranch,
        R::Reviewers { add: false, .. } => Route::RemoveReviewers,
        R::Reviewers { .. } => Route::RequestReviewers,
        R::Update { .. } => Route::Update,
        R::Review { .. } => Route::PostReview,
        R::Post { .. } => Route::PostComment,
        R::CreateIssue { .. } => Route::CreateIssue,
        R::OpenPull { .. } => Route::OpenPull,
        R::DeleteBranch { .. } => Route::DeleteBranch,
        R::LogPage { .. }
        | R::LogDelivery { .. }
        | R::Installations { .. }
        | R::Installation { .. }
        | R::Mint { .. }
        | R::Enqueue { .. }
        | R::PullGraphql { .. }
        | R::App
        | R::HookConfig
        | R::SetHook { .. }
        | R::Uninstall { .. }
        | R::Conversion { .. }
        | R::Followed { .. } => return None,
    })
}
