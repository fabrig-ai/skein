//! One request per connection, with its answer decoded as bytes arrive (6.3).
use super::{
    Limits, limits,
    settings::{self, Forge, Transport},
};
use crate::{
    call::{Auth, Call},
    reconcile::Reply,
};
use alloc::boxed::Box;
use skein_forge::{
    credential::Credential,
    forgejo::{
        client as documents,
        decode::Step,
        document::{Document, Row},
        request,
    },
    github as dialect,
};
use skein_http::client as http;
use skein_json::tokenizer;
use skein_lib::{
    Duration, Env, List, Queue, Secret, Token, Wall, bytes,
    stream::{Down, Read, Up},
};

pub(crate) struct Connection {
    pub(super) run: Token,
    pub(crate) socket: Option<Token>,
    pub(super) closed: bool,
    pub(super) closing: Option<bool>,
    close_sent: bool,
    pub(crate) result: Option<Reply>,
    answered: bool,
    http: http::Client,
    tls: Option<skein_tls::client::Client>,
    tls_up: Queue<skein_tls::client::Event>,
    tls_down: Queue<Down>,
    http_up: Queue<http::Event>,
    http_down: Queue<Down>,
    json: tokenizer::Tokenizer,
    json_up: Queue<tokenizer::Event>,
    json_down: Queue<Down>,
    json_next: Option<tokenizer::Request>,
    speech: Speech,
    pub(crate) call: Option<http::Call>,
    pub(crate) upload: Option<Box<[u8]>>,
    uploaded: usize,
}
#[expect(clippy::large_enum_variant, reason = "a connection reserves one concrete decoder for either dialect")]
enum Speech {
    Forgejo {
        request: request::Request,
        decoder: Option<documents::Decoder>,
        document: Option<Document>,
        rows: List<Row>,
    },
    GitHub {
        kind: Option<dialect::document::Kind>,
        decoder: Option<dialect::decode::Decoder>,
        document: Option<dialect::document::Document>,
        rows: List<dialect::document::Row>,
    },
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Refused {
    Unexpected,
    Unsigned,
}
impl Connection {
    pub(crate) fn new(
        run: Token,
        forge: &Forge,
        agent: &[u8],
        call: &Call,
        token: &Secret,
        l: &Limits,
    ) -> Option<Self> {
        let capacity = row_bound(call)?;
        let request = match call {
            Call::Forgejo(request) => request,
            Call::GitHub(_) => return None,
        };
        let encoded = documents::encode(request, &limits::documents(l)).ok()?;
        let mut headers = List::with_capacity(4);
        headers.push(field(b"Host", &settings::host(forge)?)).expect("four headers");
        let auth = skein_forge::forgejo::credential::authorization(&Credential::Token(token.clone()))?;
        headers.push(field(b"Authorization", auth.expose())).expect("four headers");
        headers.push(field(b"User-Agent", agent)).expect("four headers");
        let body = match &encoded.body {
            Some(body) => {
                headers.push(field(b"Content-Type", b"application/json")).expect("four headers");
                http::Body::Length(u64::try_from(body.len()).expect("bounded body"))
            }
            None => http::Body::None,
        };
        let method = match encoded.method {
            request::Method::Get => skein_http::Method::Get,
            request::Method::Post => skein_http::Method::Post,
            request::Method::Put => skein_http::Method::Put,
            request::Method::Patch => skein_http::Method::Patch,
            request::Method::Delete => skein_http::Method::Delete,
        };
        let tls = tls(forge, l);
        Some(Self {
            run,
            socket: None,
            closed: false,
            closing: None,
            close_sent: false,
            result: None,
            answered: false,
            http: http::Client::new(&limits::http_transport(l, tls.is_some())),
            tls,
            tls_up: Queue::uncapped(),
            tls_down: Queue::uncapped(),
            http_up: Queue::uncapped(),
            http_down: Queue::uncapped(),
            json: tokenizer::Tokenizer::new(&limits::documents(l).json),
            json_up: Queue::uncapped(),
            json_down: Queue::uncapped(),
            json_next: None,
            speech: Speech::Forgejo {
                request: request.clone(),
                decoder: None,
                document: None,
                rows: List::with_capacity(capacity),
            },
            call: Some(http::Call { method, target: encoded.target, headers: headers.into_boxed(), body, close: true }),
            upload: encoded.body,
            uploaded: 0,
        })
    }
    pub(crate) fn github(
        run: Token,
        forge: &Forge,
        agent: &[u8],
        call: &Call,
        auth: &Auth,
        wall: Wall,
        l: &Limits,
    ) -> Result<Self, Refused> {
        let capacity = row_bound(call).ok_or(Refused::Unexpected)?;
        let request = match call {
            Call::GitHub(request) => request,
            Call::Forgejo(_) => return Err(Refused::Unexpected),
        };
        let kind = kind_of(request)?;
        let encoded = dialect::request::encode(request, &limits::github(l)).or(Err(Refused::Unexpected))?;
        let authorization = authorization(auth, encoded.auth, wall)?;
        let mut headers = List::with_capacity(6);
        let host = settings::host(forge).ok_or(Refused::Unexpected)?;
        headers.push(field(b"Host", &host)).expect("six headers");
        headers.push(field(b"User-Agent", agent)).expect("six headers");
        if let Some(authorization) = authorization {
            headers.push(field(b"Authorization", authorization.expose())).expect("six headers");
        }
        for (name, value) in dialect::request::fields(&encoded) {
            headers.push(field(name, value)).expect("six headers");
        }
        let body = match &encoded.body {
            Some(body) => {
                headers.push(field(b"Content-Type", b"application/json")).expect("six headers");
                http::Body::Length(u64::try_from(body.len()).expect("bounded body"))
            }
            None => http::Body::None,
        };
        let method = match encoded.method {
            dialect::request::Method::Get => skein_http::Method::Get,
            dialect::request::Method::Post => skein_http::Method::Post,
            dialect::request::Method::Put => skein_http::Method::Put,
            dialect::request::Method::Patch => skein_http::Method::Patch,
            dialect::request::Method::Delete => skein_http::Method::Delete,
        };
        let tls = tls(forge, l);
        Ok(Self {
            run,
            socket: None,
            closed: false,
            closing: None,
            close_sent: false,
            result: None,
            answered: false,
            http: http::Client::new(&limits::http_transport(l, tls.is_some())),
            tls,
            tls_up: Queue::uncapped(),
            tls_down: Queue::uncapped(),
            http_up: Queue::uncapped(),
            http_down: Queue::uncapped(),
            json: tokenizer::Tokenizer::new(&limits::github(l).json),
            json_up: Queue::uncapped(),
            json_down: Queue::uncapped(),
            json_next: None,
            speech: Speech::GitHub { kind, decoder: None, document: None, rows: List::with_capacity(capacity) },
            call: Some(http::Call { method, target: encoded.target, headers: headers.into_boxed(), body, close: true }),
            upload: encoded.body,
            uploaded: 0,
        })
    }
    fn json_limits(&self, l: &Limits) -> tokenizer::Limits {
        match &self.speech {
            Speech::Forgejo { .. } => limits::documents(l).json,
            Speech::GitHub { .. } => limits::github(l).json,
        }
    }
    pub(crate) fn connect(&mut self, env: &Env<Limits>) {
        if self.closing.is_some() {
            return;
        }
        match &mut self.tls {
            Some(tls) => {
                let e = Env { now: env.now, wall: env.wall, limits: env.limits.tls };
                skein_tls::client::down(
                    tls,
                    &e,
                    skein_tls::client::Request::Handshake,
                    &mut self.tls_up,
                    &mut self.tls_down,
                );
            }
            None => self.start(env),
        }
    }
    fn start(&mut self, env: &Env<Limits>) {
        if let Some(call) = self.call.take() {
            self.http_down(env, http::Request::Call(call));
            if self.upload.is_some() {
                self.http_down(
                    env,
                    http::Request::Upload(Down::Demand {
                        read: Read::Nothing,
                        room: limits::http_transport(&env.limits, self.tls.is_some()).send,
                    }),
                );
            }
        }
    }
    pub(crate) fn stream(&mut self, env: &Env<Limits>, event: Up) {
        if self.closing.is_some() {
            return;
        }
        match &mut self.tls {
            Some(tls) => {
                let e = Env { now: env.now, wall: env.wall, limits: env.limits.tls };
                skein_tls::client::up(tls, &e, event, &mut self.tls_up, &mut self.tls_down);
            }
            None => self.http_stream(env, event),
        }
    }
    fn http_stream(&mut self, env: &Env<Limits>, event: Up) {
        let h = Env { now: env.now, wall: env.wall, limits: limits::http_transport(&env.limits, self.tls.is_some()) };
        http::up(&mut self.http, &h, event, &mut self.http_up, &mut self.http_down);
    }
    fn http_down(&mut self, env: &Env<Limits>, request: http::Request) {
        let h = Env { now: env.now, wall: env.wall, limits: limits::http_transport(&env.limits, self.tls.is_some()) };
        http::down(&mut self.http, &h, request, &mut self.http_up, &mut self.http_down);
    }
    fn json_down(&mut self, env: &Env<Limits>, request: tokenizer::Request) {
        let j = Env { now: env.now, wall: env.wall, limits: self.json_limits(&env.limits) };
        tokenizer::down(&mut self.json, &j, request, &mut self.json_up, &mut self.json_down);
    }
    pub(super) fn fail(&mut self) {
        if !self.answered {
            self.finish(Reply::Down);
        }
    }
    fn finish(&mut self, reply: Reply) {
        self.result = Some(reply);
        self.answered = true;
        self.closing = Some(false);
        self.upload = None;
        self.call = None;
    }
    pub(super) fn abort(&mut self) {
        if self.closing == Some(false) {
            self.close_sent = false;
        }
        self.answered = true;
        self.closing = Some(true);
        self.result = None;
        self.upload = None;
        self.call = None;
    }
    /// Routes one owned movement, with room reserved for one io request.
    pub(crate) fn pump(&mut self, env: &Env<Limits>, io: &mut Queue<skein_io::Request>) -> bool {
        if let Some(abort) = self.closing {
            if !self.closed
                && !self.close_sent
                && let Some(socket) = self.socket
            {
                self.close_sent = true;
                io.push(if abort {
                    skein_io::Request::Abort { entity: socket }
                } else {
                    skein_io::Request::Close { entity: socket }
                });
                return true;
            }
            return false;
        }
        if let Some(down) = self.tls_down.pop() {
            if !self.closed
                && let Some(socket) = self.socket
            {
                io.push(skein_io::Request::Stream { stream: socket, down });
            }
            return true;
        }
        if let Some(event) = self.tls_up.pop() {
            match event {
                skein_tls::client::Event::Ready(_) => self.start(env),
                skein_tls::client::Event::Stream(event) => self.http_stream(env, event),
                skein_tls::client::Event::Failed(_) => self.fail(),
                skein_tls::client::Event::Closed => (),
            }
            return true;
        }
        if let Some(down) = self.http_down.pop() {
            match &mut self.tls {
                Some(tls) => {
                    let e = Env { now: env.now, wall: env.wall, limits: env.limits.tls };
                    skein_tls::client::down(
                        tls,
                        &e,
                        skein_tls::client::Request::Stream(down),
                        &mut self.tls_up,
                        &mut self.tls_down,
                    );
                }
                None => {
                    if !self.closed
                        && let Some(socket) = self.socket
                    {
                        io.push(skein_io::Request::Stream { stream: socket, down });
                    }
                }
            }
            return true;
        }
        if let Some(down) = self.json_down.pop() {
            self.http_down(env, http::Request::Body(down));
            return true;
        }
        if let Some(next) = self.json_next.take() {
            self.json_down(env, next);
            return true;
        }
        if let Some(event) = self.json_up.pop() {
            self.json_event(env, &event);
            return true;
        }
        if let Some(event) = self.http_up.pop() {
            self.http_event(env, event);
            return true;
        }
        if self.closed && !self.answered {
            self.fail();
            return true;
        }
        false
    }
    pub(super) fn ready(&self) -> bool {
        if self.closing.is_some() {
            return !self.closed && !self.close_sent && self.socket.is_some();
        }
        !self.tls_down.is_empty()
            || !self.tls_up.is_empty()
            || !self.http_down.is_empty()
            || !self.http_up.is_empty()
            || !self.json_down.is_empty()
            || !self.json_up.is_empty()
            || self.json_next.is_some()
            || self.closed
    }
    fn http_event(&mut self, env: &Env<Limits>, event: http::Event) {
        match event {
            http::Event::Response(response) => {
                if (200..300).contains(&response.status) {
                    match &mut self.speech {
                        Speech::Forgejo { request, decoder, .. } => {
                            *decoder = documents::decoder(request, &limits::documents(&env.limits));
                            match decoder {
                                Some(decoder) => self.json_next = Some(documents::first(decoder)),
                                None => self.finish(Reply::Empty),
                            }
                        }
                        Speech::GitHub { kind, decoder, .. } => match kind {
                            Some(kind) => {
                                *decoder = Some(dialect::decode::decoder(*kind, &limits::github(&env.limits)));
                                self.json_next = Some(tokenizer::Request::Next);
                            }
                            None => self.finish(Reply::Empty),
                        },
                    }
                } else {
                    self.finish(Reply::Status(response.status));
                }
            }
            http::Event::Body(event) => {
                let j = Env { now: env.now, wall: env.wall, limits: self.json_limits(&env.limits) };
                tokenizer::up(&mut self.json, &j, event, &mut self.json_up, &mut self.json_down);
            }
            http::Event::Upload(event) => match event {
                Up::Room => self.upload(env),
                Up::Failed(_) | Up::Bytes(_) | Up::End => self.fail(),
            },
            http::Event::Done(_) => {
                let reply = match &mut self.speech {
                    Speech::Forgejo { document, rows, .. } => match document.take() {
                        Some(Document::Listed { skipped: 0 } | Document::RepoSearch { ok: true }) => {
                            let rows = core::mem::replace(rows, List::with_capacity(0));
                            Reply::Rows(rows.into_boxed())
                        }
                        Some(document) => Reply::Document(document),
                        None => Reply::Down,
                    },
                    Speech::GitHub { document, rows, .. } => match document.take() {
                        Some(dialect::document::Document::Rows) => {
                            let rows = core::mem::replace(rows, List::with_capacity(0));
                            Reply::GithubRows(rows.into_boxed())
                        }
                        Some(document) => Reply::GithubDocument(document),
                        None => Reply::Down,
                    },
                };
                self.finish(reply);
            }
            http::Event::Failed(_) => self.fail(),
            http::Event::Closed => (),
        }
    }
    fn json_event(&mut self, env: &Env<Limits>, event: &tokenizer::Event) {
        match &mut self.speech {
            Speech::Forgejo { decoder, document, rows, .. } => {
                let Some(decoder) = decoder else {
                    return;
                };
                match documents::step(decoder, &limits::documents(&env.limits), event) {
                    Step::Demand(next) => self.json_next = Some(next),
                    Step::Row(row, next) => {
                        if rows.push(row).is_err() {
                            self.fail();
                        } else {
                            self.json_next = Some(next);
                        }
                    }
                    Step::Done(Document::ApiSettings(mut settings)) => {
                        // Every setup listing uses the effective cap for sending and finishing.
                        settings.max_response_items = settings.max_response_items.min(env.limits.inner);
                        *document = Some(Document::ApiSettings(settings));
                    }
                    Step::Done(done) => *document = Some(done),
                    Step::Malformed(_) => self.fail(),
                }
            }
            Speech::GitHub { decoder, document, rows, .. } => {
                let Some(decoder) = decoder else {
                    return;
                };
                let (next, step) = decoder.step(event.clone());
                match step {
                    dialect::decode::Step::Demand => self.json_next = Some(next),
                    dialect::decode::Step::Row(row) => {
                        if rows.push(row).is_err() {
                            self.fail();
                        } else {
                            self.json_next = Some(next);
                        }
                    }
                    dialect::decode::Step::Done(done) => *document = Some(done),
                    dialect::decode::Step::Malformed(_) => self.fail(),
                }
            }
        }
    }
    fn upload(&mut self, env: &Env<Limits>) {
        let Some(body) = &self.upload else {
            return;
        };
        let end = self
            .uploaded
            .saturating_add(
                usize::try_from(limits::http_transport(&env.limits, self.tls.is_some()).send).expect("u32 fits"),
            )
            .min(body.len());
        let piece = bytes::copy_of(body.get(self.uploaded..end).expect("bounded upload"));
        self.uploaded = end;
        let done = end == body.len();
        self.http_down(env, http::Request::Upload(Down::Send(piece)));
        if done {
            self.upload = None;
            self.http_down(env, http::Request::Upload(Down::Finish));
        } else {
            self.http_down(
                env,
                http::Request::Upload(Down::Demand {
                    read: Read::Nothing,
                    room: limits::http_transport(&env.limits, self.tls.is_some()).send,
                }),
            );
        }
    }
}
/// Rows in one setup reply, from its requested page or the pinned installation page.
/// Unpaged listings and requests outside setup have no admitted row bound.
pub(crate) fn row_bound(call: &Call) -> Option<u32> {
    match call {
        Call::Forgejo(request) => forgejo_row_bound(request),
        Call::GitHub(request) => github_row_bound(request),
    }
}
fn forgejo_row_bound(request: &request::Request) -> Option<u32> {
    use request::Request;
    match request {
        Request::Tokens { page, .. }
        | Request::RepoHooks { page, .. }
        | Request::RepoSearch { page }
        | Request::UserRepos { page } => page.limit,
        Request::AddCollaborator { .. }
        | Request::CreateRepoHook { .. }
        | Request::EditRepoHook { .. }
        | Request::DeleteRepoHook { .. }
        | Request::Org { .. }
        | Request::CreateOrg { .. }
        | Request::CreateFile { .. }
        | Request::CreateRepository { .. }
        | Request::Repository { .. }
        | Request::Branch { .. }
        | Request::Raw { .. }
        | Request::User
        | Request::ApiSettings
        | Request::RepositoryById { .. }
        | Request::Permission { .. }
        | Request::CreateToken { .. }
        | Request::DeleteToken { .. }
        | Request::UserOf { .. }
        | Request::CreateUser { .. } => Some(0),
        Request::Contents { .. }
        | Request::OrgHooks { .. }
        | Request::CreateOrgHook { .. }
        | Request::EditOrgHook { .. }
        | Request::DeleteOrgHook { .. }
        | Request::Teams { .. }
        | Request::CreateTeam { .. }
        | Request::EditTeam { .. }
        | Request::TeamMembers { .. }
        | Request::AddMember { .. }
        | Request::UserSearch { .. }
        | Request::Issues { .. }
        | Request::Issue { .. }
        | Request::Comments { .. }
        | Request::Pull { .. }
        | Request::PullFor { .. }
        | Request::CombinedStatus { .. }
        | Request::Reviews { .. }
        | Request::ReviewComments { .. }
        | Request::Branches { .. }
        | Request::Protection { .. }
        | Request::Protections { .. }
        | Request::Collaborators { .. }
        | Request::Pulls { .. }
        | Request::PullFiles { .. }
        | Request::Compare { .. }
        | Request::Tree { .. }
        | Request::WorkflowRuns { .. }
        | Request::Dispatch { .. }
        | Request::Rerun { .. }
        | Request::Runs { .. }
        | Request::Run { .. }
        | Request::Jobs { .. }
        | Request::Job { .. }
        | Request::JobLog { .. }
        | Request::CreateIssue { .. }
        | Request::OpenPull { .. }
        | Request::PostComment { .. }
        | Request::PostReview { .. }
        | Request::EditIssue { .. }
        | Request::RequestReviewers { .. }
        | Request::RemoveReviewers { .. }
        | Request::Merge { .. }
        | Request::Update { .. }
        | Request::PostStatus { .. }
        | Request::CreateBranch { .. }
        | Request::DeleteBranch { .. } => None,
    }
}
fn github_row_bound(request: &dialect::request::Request) -> Option<u32> {
    use dialect::request::Request;
    match request {
        Request::Installations { .. } => Some(dialect::pin::PAGE_CAP),
        Request::Reachable { per_page, .. } => Some(*per_page),
        Request::Installation { .. }
        | Request::Mint { .. }
        | Request::Viewer
        | Request::App
        | Request::HookConfig
        | Request::SetHook { .. }
        | Request::Uninstall { .. }
        | Request::Conversion { .. }
        | Request::BotUser { .. } => Some(0),
        Request::LogPage { .. }
        | Request::LogDelivery { .. }
        | Request::Merge { .. }
        | Request::Enqueue { .. }
        | Request::PullGraphql { .. }
        | Request::Repository { .. }
        | Request::Locate { .. }
        | Request::Issues { .. }
        | Request::Issue { .. }
        | Request::Comments { .. }
        | Request::Pull { .. }
        | Request::PullsFor { .. }
        | Request::Reviews { .. }
        | Request::ReviewComments { .. }
        | Request::CombinedStatus { .. }
        | Request::CheckRuns { .. }
        | Request::Job { .. }
        | Request::Followed { .. }
        | Request::JobLog { .. }
        | Request::Branch { .. }
        | Request::Branches { .. }
        | Request::PullFiles { .. }
        | Request::Compare { .. }
        | Request::Raw { .. }
        | Request::RulesForBranch { .. }
        | Request::Ruleset { .. }
        | Request::ClassicProtection { .. }
        | Request::Collaborators { .. }
        | Request::Permission { .. }
        | Request::User { .. }
        | Request::Reopen { .. }
        | Request::Edit { .. }
        | Request::Status { .. }
        | Request::CreateBranch { .. }
        | Request::Reviewers { .. }
        | Request::Update { .. }
        | Request::Review { .. }
        | Request::Post { .. }
        | Request::CreateIssue { .. }
        | Request::OpenPull { .. }
        | Request::Close { .. }
        | Request::DeleteBranch { .. } => None,
    }
}

fn tls(forge: &Forge, l: &Limits) -> Option<skein_tls::client::Client> {
    match &forge.transport {
        Transport::Plaintext { .. } => None,
        Transport::Tls { server_name, trust } => {
            Some(skein_tls::client::Client::new(trust, server_name.clone(), &l.tls))
        }
    }
}
fn field(name: &[u8], value: &[u8]) -> skein_http::Header {
    skein_http::Header { name: bytes::copy_of(name), value: bytes::copy_of(value) }
}
impl core::fmt::Debug for Connection {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Connection")
            .field("run", &self.run)
            .field("closing", &self.closing)
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}

/// The setup document a request is answered with, or no document for uninstall.
pub(crate) fn kind_of(request: &dialect::request::Request) -> Result<Option<dialect::document::Kind>, Refused> {
    use dialect::{document::Kind, request::Request};
    Ok(Some(match request {
        Request::Installations { .. } => Kind::InstallationRows,
        Request::Installation { .. } => Kind::Installation,
        Request::Mint { .. } => Kind::Minted,
        Request::Reachable { .. } => Kind::ReachableRows,
        Request::Viewer => Kind::Viewer,
        Request::App => Kind::App,
        Request::HookConfig | Request::SetHook { .. } => Kind::HookConfig,
        Request::Conversion { .. } => Kind::Conversion,
        Request::BotUser { .. } => Kind::BotUser,
        Request::Uninstall { .. } => return Ok(None),
        Request::PullGraphql { .. }
        | Request::Repository { .. }
        | Request::Locate { .. }
        | Request::Issues { .. }
        | Request::Issue { .. }
        | Request::Comments { .. }
        | Request::Pull { .. }
        | Request::PullsFor { .. }
        | Request::Reviews { .. }
        | Request::ReviewComments { .. }
        | Request::CombinedStatus { .. }
        | Request::CheckRuns { .. }
        | Request::Job { .. }
        | Request::JobLog { .. }
        | Request::Followed { .. }
        | Request::Branch { .. }
        | Request::Branches { .. }
        | Request::PullFiles { .. }
        | Request::Compare { .. }
        | Request::Raw { .. }
        | Request::RulesForBranch { .. }
        | Request::Ruleset { .. }
        | Request::ClassicProtection { .. }
        | Request::Collaborators { .. }
        | Request::Permission { .. }
        | Request::User { .. }
        | Request::Reopen { .. }
        | Request::Edit { .. }
        | Request::Status { .. }
        | Request::CreateBranch { .. }
        | Request::Update { .. }
        | Request::Review { .. }
        | Request::Post { .. }
        | Request::CreateIssue { .. }
        | Request::OpenPull { .. }
        | Request::Close { .. }
        | Request::DeleteBranch { .. }
        | Request::Reviewers { .. }
        | Request::Merge { .. }
        | Request::LogPage { .. }
        | Request::LogDelivery { .. }
        | Request::Enqueue { .. } => return Err(Refused::Unexpected),
    }))
}
fn authorization(auth: &Auth, encoded: dialect::request::Auth, wall: Wall) -> Result<Option<Secret>, Refused> {
    use dialect::request::Auth as Selected;
    match auth {
        Auth::Token(_) => match encoded {
            Selected::App | Selected::Installation | Selected::None => Err(Refused::Unexpected),
        },
        Auth::App { id, key } => match encoded {
            Selected::App => {
                let issued = Wall::from_nanos(wall.as_nanos().saturating_sub(Duration::from_secs(60).as_nanos()));
                let expires = Wall::from_nanos(issued.as_nanos().saturating_add(dialect::jwt::LIFE.as_nanos()));
                let input = dialect::jwt::signing_input(*id, issued, expires).ok_or(Refused::Unsigned)?;
                let signature = skein_tls::sign_rs256(key.expose(), &input).ok_or(Refused::Unsigned)?;
                let jwt = dialect::jwt::token(&input, &signature).ok_or(Refused::Unsigned)?;
                let bearer = dialect::credential::bearer(&jwt).ok_or(Refused::Unsigned)?;
                Ok(Some(bearer))
            }
            Selected::Installation | Selected::None => Err(Refused::Unexpected),
        },
        Auth::Bearer(token) => match encoded {
            Selected::Installation => {
                let bearer = dialect::credential::bearer(token).ok_or(Refused::Unexpected)?;
                Ok(Some(bearer))
            }
            Selected::App | Selected::None => Err(Refused::Unexpected),
        },
        Auth::None => match encoded {
            Selected::None => Ok(None),
            Selected::App | Selected::Installation => Err(Refused::Unexpected),
        },
    }
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
