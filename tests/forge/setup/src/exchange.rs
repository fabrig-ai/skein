//! One HTTP exchange, cut into seeded fragments; credentials never enter its trace.
use skein_fake_forge_domain::{self as domain, api};
use skein_fake_forge_protocol::{self as fake, Service};
use skein_forge::forgejo::{
    client as codec,
    document::Document,
    request::{Method, Request},
};
use skein_forge_machine_world::pipe::{Cut, Pipe};
use skein_forge_setup::{
    call::{Auth, Call},
    reconcile::Reply,
};
use skein_http::{Header, client as http};
use skein_lib::{
    Env, Queue, Time, Token, Wall,
    stream::{Down, Read, Up},
};
struct Exchange {
    client: http::Client,
    client_env: Env<http::Limits>,
    above: Queue<http::Event>,
    below: Queue<Down>,
    server: fake::Server,
    events: Queue<fake::Event>,
    down: Queue<Down>,
    pipe: Pipe,
    upload: Vec<u8>,
    body: Vec<u8>,
    status: u16,
    done: bool,
    closed: bool,
    repositories: Vec<fake::Repository>,
    login: Box<[u8]>,
}
pub fn send(
    service: &mut Service,
    env: &Env<fake::Limits>,
    request: &Call,
    auth: &Auth,
    seed: u64,
    repositories: &[fake::Repository],
    login: &[u8],
) -> Reply {
    send_on(service, env, request, auth, seed, repositories, login, None)
}
#[expect(clippy::too_many_arguments, reason = "the exchange carries the scenario and its optional persistent domain")]
pub fn send_on(
    service: &mut Service,
    env: &Env<fake::Limits>,
    request: &Call,
    auth: &Auth,
    seed: u64,
    repositories: &[fake::Repository],
    login: &[u8],
    mut hosted: Option<fake::Hosted<'_>>,
) -> Reply {
    let (request, token) = match (request, auth) {
        (Call::Forgejo(request), Auth::Token(token)) => (request, token),
        pair => panic!("unsupported setup send: {pair:?}"),
    };
    let encoded = codec::encode(request, &env.limits.documents).expect("machine request encodes");
    let limits = http::Limits {
        request: env.limits.http.head,
        head: env.limits.http.head,
        headers: env.limits.http.headers,
        read: env.limits.http.read,
        send: env.limits.http.send,
    };
    let mut exchange = Exchange {
        client: http::Client::new(&limits),
        client_env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
        above: Queue::with_capacity(8),
        below: Queue::with_capacity(8),
        server: fake::Server::new(Token::new(1), &env.limits).expect("server limits"),
        events: Queue::with_capacity(8),
        down: Queue::with_capacity(32),
        pipe: Pipe::new(seed, Cut::Never),
        upload: encoded.body.unwrap_or_default().into_vec(),
        body: Vec::new(),
        status: 0,
        done: false,
        closed: false,
        repositories: repositories.to_vec(),
        login: login.into(),
    };
    let method = match encoded.method {
        Method::Get => http::Method::Get,
        Method::Post => http::Method::Post,
        Method::Put => http::Method::Put,
        Method::Patch => http::Method::Patch,
        Method::Delete => http::Method::Delete,
    };
    let mut authorization = b"token ".to_vec();
    authorization.extend_from_slice(token.expose());
    let call = http::Call {
        method,
        target: encoded.target,
        headers: Box::new([
            Header { name: b"Host".as_slice().into(), value: b"127.0.0.1:3000".as_slice().into() },
            Header { name: b"Authorization".as_slice().into(), value: authorization.into_boxed_slice() },
        ]),
        body: if exchange.upload.is_empty() {
            http::Body::None
        } else {
            http::Body::Length(exchange.upload.len().try_into().expect("body length"))
        },
        close: true,
    };
    exchange.client(http::Request::Call(call));
    if !exchange.upload.is_empty() {
        exchange.client(http::Request::Upload(Down::Demand {
            read: Read::Nothing,
            room: u32::try_from(exchange.upload.len()).expect("bounded body").min(limits.send),
        }));
    }
    fake::start(&mut exchange.server, service, env, &mut exchange.events, &mut exchange.down);
    for _ in 0..100_000 {
        exchange.tick(service, env, hosted.as_mut());
        if exchange.done && exchange.closed {
            return decode(request, exchange.status, &exchange.body, &env.limits);
        }
    }
    panic!(
        "HTTP exchange did not settle: {request:?}, status {}, done {}, closed {}",
        exchange.status, exchange.done, exchange.closed
    )
}
impl Exchange {
    fn client(&mut self, request: http::Request) {
        http::down(&mut self.client, &self.client_env, request, &mut self.above, &mut self.below);
    }
    fn tick(&mut self, service: &mut Service, env: &Env<fake::Limits>, mut hosted: Option<&mut fake::Hosted<'_>>) {
        match hosted.as_mut() {
            Some(hosted) => fake::resume_with_domain(
                &mut self.server,
                service,
                fake::Hosted { domain: hosted.domain, config: hosted.config },
                env,
                &mut self.events,
                &mut self.down,
            ),
            None => fake::resume(&mut self.server, service, env, &mut self.events, &mut self.down),
        }
        while let Some(down) = self.below.pop() {
            self.pipe.client_down(down);
        }
        while let Some(down) = self.down.pop() {
            self.pipe.server_down(down);
        }
        if let Some(up) = self.pipe.server_up() {
            match hosted.as_mut() {
                Some(hosted) => fake::up_with_domain(
                    &mut self.server,
                    service,
                    fake::Hosted { domain: hosted.domain, config: hosted.config },
                    env,
                    up,
                    &mut self.events,
                    &mut self.down,
                ),
                None => fake::up(&mut self.server, service, env, up, &mut self.events, &mut self.down),
            }
        }
        if let Some(up) = self.pipe.client_up() {
            http::up(&mut self.client, &self.client_env, up, &mut self.above, &mut self.below);
        }
        while let Some(event) = self.above.pop() {
            self.event(event);
        }
        while let Some(event) = self.events.pop() {
            if let Some(hosted) = hosted.as_mut() {
                let event = match event {
                    fake::Event::Domain(event) => event,
                    fake::Event::Close => {
                        fake::closed(&mut self.server, service, env, &mut self.events, &mut self.down);
                        continue;
                    }
                    fake::Event::Closed => {
                        self.closed = true;
                        continue;
                    }
                };
                let domain_env = Env { now: env.now, wall: env.wall, limits: *hosted.config };
                let mut answers = Queue::with_capacity(domain::MAX_OUT);
                domain::step(hosted.domain, &domain_env, event, &mut answers);
                domain::fire(hosted.domain, &domain_env, &mut answers);
                while let Some(answer) = answers.pop() {
                    match answer {
                        domain::Request::Reply { .. } => {
                            fake::down(&mut self.server, service, env, answer, &mut self.events, &mut self.down);
                        }
                        domain::Request::Hook { .. } => (),
                    }
                }
                hosted.domain.reclaim();
                continue;
            }
            match event {
                fake::Event::Domain(domain::Event::Call {
                    reply_to,
                    user,
                    repository,
                    op: api::Op::Read(api::Read::ListingPermission),
                }) => {
                    let id =
                        self.repositories.iter().find(|repo| repo.full == repository).expect("scenario repository").id;
                    let grant = service
                        .setup()
                        .find(&self.login)
                        .filter(|account| account.id == user)
                        .and_then(|account| service.setup().collaborator(id, &account.login));
                    let permission = match grant {
                        None => api::Permission::None,
                        Some(skein_forge::forgejo::request::Grant::Read) => api::Permission::Read,
                        Some(skein_forge::forgejo::request::Grant::Write) => api::Permission::Write,
                        Some(skein_forge::forgejo::request::Grant::Admin) => api::Permission::Admin,
                    };
                    fake::down(
                        &mut self.server,
                        service,
                        env,
                        domain::Request::Reply { to: reply_to, result: Ok(api::Answer::Permission(permission)) },
                        &mut self.events,
                        &mut self.down,
                    );
                }
                fake::Event::Domain(domain::Event::Call { .. }) => panic!("unexpected repository domain call"),
                fake::Event::Close => fake::closed(&mut self.server, service, env, &mut self.events, &mut self.down),
                fake::Event::Closed => self.closed = true,
            }
        }
        service.reclaim();
    }
    fn event(&mut self, event: http::Event) {
        match event {
            http::Event::Response(response) => {
                self.status = response.status;
                self.read();
            }
            http::Event::Upload(Up::Room) => {
                let count = self.upload.len().min(usize::try_from(self.client_env.limits.send).expect("send"));
                let bytes: Vec<_> = self.upload.drain(..count).collect();
                self.client(http::Request::Upload(Down::Send(bytes.into_boxed_slice())));
                if self.upload.is_empty() {
                    self.client(http::Request::Upload(Down::Finish));
                } else {
                    self.client(http::Request::Upload(Down::Demand {
                        read: Read::Nothing,
                        room: self.client_env.limits.send,
                    }));
                }
            }
            http::Event::Body(Up::Bytes(bytes)) => {
                self.body.extend_from_slice(&bytes);
                self.read();
            }
            http::Event::Body(Up::End) | http::Event::Closed => (),
            http::Event::Done(_) => {
                self.done = true;
                self.client(http::Request::Close);
            }
            http::Event::Failed(error) => panic!("HTTP failed: {error:?}"),
            http::Event::Body(Up::Room | Up::Failed(_))
            | http::Event::Upload(Up::Bytes(_) | Up::End | Up::Failed(_)) => panic!("invalid HTTP stream event"),
        }
    }
    fn read(&mut self) {
        self.client(http::Request::Body(Down::Demand {
            read: Read::Arrived { max: self.client_env.limits.read },
            room: 0,
        }));
    }
}
fn decode(request: &Request, status: u16, body: &[u8], limits: &fake::Limits) -> Reply {
    if !(200..300).contains(&status) {
        return Reply::Status(status);
    }
    let Some(decoder) = codec::decoder(request, &limits.documents) else {
        if let Request::Raw { .. } = request {
            return Reply::Empty;
        }
        assert!(body.is_empty());
        return Reply::Empty;
    };
    let answer = crate::decode::decode_answer(decoder, body, &limits.documents);
    let document = answer.document.expect("fake answer decodes");
    match document {
        Document::Listed { skipped: 0 } | Document::RepoSearch { ok: true } => {
            Reply::Rows(answer.rows.into_boxed_slice())
        }
        other @ (Document::Hook(_)
        | Document::Token(_)
        | Document::Org(_)
        | Document::CreatedRepository(_)
        | Document::CreatedFile { .. }
        | Document::Team(_)
        | Document::User(_)
        | Document::ApiSettings(_)
        | Document::Repository(_)
        | Document::Search { .. }
        | Document::RepoSearch { ok: false }
        | Document::Listed { .. }
        | Document::Issue(_)
        | Document::Comment(_)
        | Document::Status(_)
        | Document::Review(_)
        | Document::Pull(_)
        | Document::CombinedStatus(_)
        | Document::Branch(_)
        | Document::Protection(_)
        | Document::Permission(_)
        | Document::Compare(_)
        | Document::Tree { .. }
        | Document::Runs { .. }
        | Document::Run(_)
        | Document::Job(_)
        | Document::Error(_)) => Reply::Document(other),
    }
}
