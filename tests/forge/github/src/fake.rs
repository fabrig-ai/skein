//! Hosted GitHub over the replaying kernel; no machine socket is opened.
use crate::{
    Result,
    live::{Fixtures, Installation},
    wire::{Request, Response},
};
use skein_fake_forge_domain::{self as domain, api as a};
use skein_fake_forge_github::{
    self as fake,
    service::{Account, AppFacts, Row},
};
use skein_fake_peers::forge::{Forge, Peer};
use skein_io::kernel::{Complete, Done, Family, Op, Submit};
use skein_lib::{Duration, Env, List, Queue, ReplyTo, Secret, Time, Token, Wall, bytes};
use skein_shell::Host;
use skein_sim::{Pid, Sim};
use std::{cell::RefCell, rc::Rc};

pub(crate) const BOT: u64 = 9;
pub(crate) const PERSON: u64 = 31;
pub(crate) fn boxed(b: &[u8]) -> Box<[u8]> {
    bytes::copy_of(b)
}
pub(crate) fn bounds() -> skein_forge::github::limits::Limits {
    skein_forge::github::limits::Limits {
        json: skein_json::tokenizer::Limits { depth: 32, string: 524_288, number: 32, chunk: 4096, length: 1_048_576 },
        writer: skein_json::writer::Limits { depth: 32, length: 1_048_576 },
        string_bytes: 8192,
        body_bytes: 524_288,
        inner: 512,
        target_bytes: 4096,
    }
}
fn limits() -> fake::Limits {
    fake::Limits {
        deliveries: fake::delivery::Limits { entries: 256, payload_bytes: 1_048_576 },
        http: skein_http::server::Limits {
            head: 8192,
            headers: 32,
            body: 524_288,
            read: 4096,
            response: 1_048_576,
            send: 4096,
        },
        github: bounds(),
        installations: fake::installations::Limits { installations: 8, tokens: 16, listed: 8 },
        pages: fake::pages::Limits { states: 8, manifest_bytes: 8192 },
        page: 100,
        body: 524_288,
        response: 1_048_576,
        blob_records: 32,
        git_records: 16,
    }
}
fn config() -> domain::Config {
    domain::Config {
        limits: domain::Limits {
            repositories: 3,
            users: 4,
            bypassing_users: 4,
            labels: 4,
            items: 128,
            comments: 32,
            dependencies: 2,
            reviews: 8,
            branches: 64,
            commits: 128,
            files: 512,
            statuses: 128,
            contexts: 4,
            pages: 1,
            name_bytes: 256,
            title_bytes: 256,
            body_bytes: 8192,
            content_bytes: 1024,
            page_size: 100,
            calls: 8,
            hooks: 128,
            observations: 1024,
        },
        latency_min: Duration::from_millis(1),
        latency_max: Duration::from_millis(1),
        settle_min: Duration::from_secs(2),
        settle_max: Duration::from_secs(2),
        late: 0,
        late_min: Duration::ZERO,
        late_max: Duration::ZERO,
        unavailable: 0,
        timeouts: 0,
        landing: 0,
        land_min: Duration::ZERO,
        land_max: Duration::ZERO,
        rate_limit: 0,
        rate_window: Duration::from_secs(3600),
        ci: 33,
        hook_min: Duration::ZERO,
        hook_max: Duration::ZERO,
        hooks_late: 0,
        hooks_lost: 0,
        resolution: Duration::from_secs(1),
        skew: domain::Skew::None,
        status_updates: false,
        edit_updates: false,
    }
}
pub(crate) struct World {
    pub peer: Peer,
    pub forge: Rc<RefCell<Forge>>,
    sim: Sim,
    peer_pid: Pid,
    client_pid: Pid,
    serial: u64,
    #[cfg(test)]
    pub last_wire: Vec<u8>,
    #[cfg(test)]
    pub refuse_issue_patch_for: Option<String>,
    #[cfg(test)]
    pub null_next_merged_pull_commit: bool,
}
impl World {
    pub fn new() -> Result<Self> {
        Self::with_organization(true)
    }
    #[expect(clippy::too_many_lines, reason = "one declared fixture binds domain, service and peer limits")]
    pub(crate) fn with_organization(organization: bool) -> Result<Self> {
        let config = config();
        let mut d = domain::Domain::new(&config, 23);
        for (id, name) in [(2, b"acme/widgets".as_slice()), (4, b"globex/gadgets"), (6, b"acme/spare")] {
            domain::repository(
                &mut d,
                &config,
                a::Setup {
                    id,
                    name: boxed(name),
                    default: boxed(b"main"),
                    tree: Box::new([]),
                    labels: Box::new([]),
                    checks: a::Checks {
                        contexts: Box::new([boxed(b"ci")]),
                        latency_min: Duration::from_secs(45),
                        latency_max: Duration::from_secs(45),
                        silent: 0,
                        passes: 1000,
                        reruns: 0,
                        cue: Some(a::Cue::Message { fail: boxed(b"FAIL") }),
                    },
                    protection: None,
                    hooked: true,
                },
            );
            for user in [BOT, PERSON, 32] {
                domain::grant(&mut d, name, user, a::Permission::Admin);
            }
        }
        let forge = Rc::new(RefCell::new(Forge::new(d, config)));
        let mut repositories = List::with_capacity(3);
        for (id, owner, owner_id, name) in [
            (2, b"acme".as_slice(), 22, b"widgets".as_slice()),
            (4, b"globex", 24, b"gadgets"),
            (6, b"acme", 22, b"spare"),
        ] {
            repositories
                .push(Row {
                    ci_setup: skein_fake_forge_github::service::CiSetup::DEFAULT,
                    id,
                    owner: boxed(owner),
                    owner_id,
                    organization: organization && owner == b"globex",
                    name: boxed(name),
                    private: true,
                })
                .expect("fixture rows");
        }
        let mut accounts = List::with_capacity(4);
        for (id, login, organization) in [
            (22, b"acme".as_slice(), false),
            (24, b"globex", organization),
            (PERSON, b"alice", false),
            (32, b"bob", false),
        ] {
            accounts.push(Account { id, login: boxed(login), organization }).expect("fixture accounts");
        }
        let mut tokens = List::with_capacity(2);
        for (token, user, expiry) in [
            (
                "invented-acme-person-token",
                PERSON,
                Some(skein_forge::forgejo::time::read(b"2026-01-11T00:00:00Z").expect("fixture expiry")),
            ),
            ("invented-globex-person-token", 32, None),
        ] {
            tokens.push((Secret::new(boxed(token.as_bytes())), user, expiry)).expect("fixture tokens");
        }
        let cfg = fake::Config {
            rate_quota: 5000,
            rate_window: Duration::from_secs(3600),
            token_life: skein_fake_forge_github::installations::TOKEN_LIFE,
            deliveries: fake::delivery::Config::standard(),
            base: boxed(b"http://127.0.0.1:1"),
            blob: boxed(b"http://logs.example.test"),
            third_origin: None,
            blob_redirect: None,
            app: fake::app::App {
                id: 7,
                slug: boxed(b"example-app-conformance"),
                owner: PERSON,
                permissions: fake::app::wanted(),
                events: Box::new([]),
                key: fake::keys::KEYS[0],
                webhook: Secret::new(boxed(b"invented-webhook-secret")),
            },
            app_facts: AppFacts::Complete,
            shown: fake::service::Shown::Ruleset,
            ruleset_id: 12_345_678,
            pull_requests_only: false,
            required_signatures: false,
            tokens,
            faults: fake::Faults { reset: 0, drop: 0, server_error: 0 },
            bot: BOT,
            hook: boxed(b"http://127.0.0.1:1/hooks"),
            repositories,
            accounts,
            person: boxed(b"alice"),
        };
        let peer_limits = skein_fake_peers::Limits {
            io: skein_io::Limits {
                sockets: 10,
                refusals: 1,
                intake: 32768,
                receive: 4096,
                output: 1_048_576,
                sends: 2,
                accepts: 1,
                backlog: 4,
                close_timeout: Duration::from_secs(1),
                names: 0,
                retry: Duration::from_millis(1),
            },
            connections: 4,
            queue: 512,
            plaintext: 1_048_576,
            ciphertext: 1_048_576,
            observations: 8192,
            observation_bytes: 16_777_216,
        };
        let addr = (std::net::Ipv4Addr::LOCALHOST, 0).into();
        let peer = Peer::github(addr, addr, peer_limits, cfg, limits(), Rc::clone(&forge), 23)
            .map_err(|_| "fake fixture limits refused")?;
        let mut sim = Sim::new(23, skein_sim::Config::calm());
        // Item timestamps use the domain clock; keep them above the observed since cutoff.
        sim.advance_to(Time::from_nanos(31_536_000_000_000_000));
        let peer_pid = sim.spawn_process();
        let client_pid = sim.spawn_process();
        let mut w = Self {
            peer,
            forge,
            sim,
            peer_pid,
            client_pid,
            serial: 0,
            #[cfg(test)]
            last_wire: vec![],
            #[cfg(test)]
            refuse_issue_patch_for: None,
            #[cfg(test)]
            null_next_merged_pull_commit: false,
        };
        for _ in 0..1000 {
            if w.peer.address().is_some() && w.peer.blob_address().is_some() {
                return Ok(w);
            }
            w.pass();
        }
        Err("fake listeners did not bind".into())
    }
    fn pass(&mut self) {
        self.sim.reap(self.peer_pid, self.peer.completions());
        self.peer.iterate(self.sim.now(), self.sim.wall());
        self.sim.submit(self.peer_pid, self.peer.submissions());
        if !self.peer.work_pending(self.sim.now())
            && let Some(next) = [self.sim.next_due(), self.peer.next_deadline()].into_iter().flatten().min()
        {
            self.sim.advance_to(next.min(self.sim.now().saturating_add(Duration::from_millis(1))));
        }
    }
    fn client(&mut self, kind: Op) -> Result<Complete> {
        self.serial += 1;
        let mut output = Queue::with_capacity(1);
        let mut input = Queue::with_capacity(1);
        output.push(Submit { op: Token::new(self.serial), kind });
        self.sim.submit(self.client_pid, &mut output);
        for _ in 0..100_000 {
            self.sim.reap(self.client_pid, &mut input);
            if let Some(done) = input.pop() {
                return Ok(done);
            }
            self.pass();
        }
        Err("fake client progress bound".into())
    }
    pub fn now(&self) -> Wall {
        self.sim.wall()
    }
    pub fn wait(&mut self, seconds: u64) {
        self.sim.advance_to(self.sim.now().saturating_add(Duration::from_secs(seconds)));
        for _ in 0..16 {
            self.pass();
        }
    }
    pub fn exchange(&mut self, request: &Request) -> Result<(Vec<u8>, Response)> {
        let wire = request.wire("127.0.0.1")?;
        #[cfg(test)]
        {
            self.last_wire = wire.clone();
            // A scoped issue-PATCH refusal models a restricted person token.
            if request.method == skein_forge::github::request::Method::Patch
                && request.target.rsplit_once("/issues/").is_some_and(|(_, number)| number.parse::<u64>().is_ok())
                && self
                    .refuse_issue_patch_for
                    .as_ref()
                    .is_some_and(|token| request.authorization.as_deref() == Some(format!("Bearer {token}").as_str()))
            {
                return Ok((
                    wire,
                    Response {
                        status: 403,
                        head: b"HTTP/1.1 403 Forbidden\r\nContent-Type: application/json".to_vec(),
                        body: br#"{"message":"Forbidden"}"#.to_vec(),
                    },
                ));
            }
        }
        let socket = self.client(Op::Socket { family: Family::Ipv4 })?;
        let Ok(Done::Fd(fd)) = socket.result else {
            return Err("fake socket refused".into());
        };
        let addr =
            if request.blob { self.peer.blob_address() } else { self.peer.address() }.ok_or("fake listener absent")?;
        self.client(Op::Connect { fd, addr })?.result.map_err(|_| "fake connection refused")?;
        let mut from = 0;
        while usize::try_from(from)? < wire.len() {
            let sent = self.client(Op::Send { fd, bytes: boxed(&wire), from })?;
            let Ok(Done::Count(count)) = sent.result else {
                return Err("fake send refused".into());
            };
            if count == 0 {
                return Err("fake send made no progress".into());
            }
            from += count;
        }
        let mut received = Vec::new();
        for _ in 0..4096 {
            let read = self.client(Op::Recv { fd, buf: Box::new([0; 4096]) })?;
            let Ok(Done::Count(count)) = read.result else {
                return Err("fake receive refused".into());
            };
            if count == 0 {
                break;
            }
            let Op::Recv { buf, .. } = read.kind else {
                return Err("fake receive buffer absent".into());
            };
            received.extend_from_slice(&buf[..usize::try_from(count)?]);
            if received.len() > 4_194_304 {
                self.client(Op::Close { fd })?.result.map_err(|_| "fake close refused")?;
                return Err(crate::wire::failure(&wire, &received, "response exceeds conformance bound").into());
            }
        }
        self.client(Op::Close { fd })?.result.map_err(|_| "fake close refused")?;
        let response = crate::wire::read(&wire, &received, true)?.ok_or("fake answer incomplete")?;
        #[cfg(test)]
        let response = {
            let mut response = response;
            if self.null_next_merged_pull_commit && request.target == "/graphql" {
                let document = response.json()?;
                let pull = document.root().get("data").get("repository").get("pullRequest");
                if pull.get("mergeCommit").get("oid").str().is_some() {
                    let row = skein_forge::github::document::PullGraphql {
                        id: boxed(pull.get("id").str().ok_or("fake pull node id absent")?.as_bytes()),
                        merge_commit: None,
                        queued: pull.get("mergeQueueEntry").get("position").u64().map(u32::try_from).transpose()?,
                        dequeued: pull
                            .get("timelineItems")
                            .get("nodes")
                            .nth(0)
                            .get("reason")
                            .str()
                            .map(|reason| boxed(reason.as_bytes())),
                    };
                    let mut body = skein_lib::Writer::new(response.body.len());
                    skein_forge::github::server::pull_graphql(&row, &mut body)
                        .map_err(|_| "fake null merge commit response exceeds bound")?;
                    let length = body.written();
                    body.put(&vec![0; body.room()]).map_err(|_| "fake response padding exceeds bound")?;
                    response.body = body.finish().into_vec();
                    response.body.truncate(length);
                    self.null_next_merged_pull_commit = false;
                }
            }
            response
        };
        Ok((wire, response))
    }
    pub fn call(&mut self, repo: &str, user: u64, op: a::Op) -> Result<a::Answer> {
        let mut f = self.forge.borrow_mut();
        let token = f.next_reply();
        let env = Env { now: self.sim.now(), wall: self.sim.wall(), limits: f.config };
        let mut output = Queue::with_capacity(domain::MAX_OUT);
        domain::step(
            &mut f.domain,
            &env,
            domain::Event::Call { reply_to: ReplyTo::new(token), user, repository: boxed(repo.as_bytes()), op },
            &mut output,
        );
        while let Some(request) = output.pop() {
            f.pending.push(request);
        }
        drop(f);
        for _ in 0..100_000 {
            self.pass();
            let mut f = self.forge.borrow_mut();
            let mut retained = Vec::new();
            let mut answer = None;
            while let Some(r) = f.pending.pop() {
                match r {
                    domain::Request::Reply { to, result } => {
                        let target = to.into_token();
                        if target == token {
                            answer = Some(result);
                        } else {
                            retained.push(domain::Request::Reply { to: ReplyTo::new(target), result });
                        }
                    }
                    hook @ domain::Request::Hook { .. } => retained.push(hook),
                }
            }
            for r in retained {
                f.pending.push(r);
            }
            if let Some(answer) = answer {
                return answer.map_err(|e| format!("fixture domain call refused: {e:?}").into());
            }
        }
        Err("fixture domain reply absent".into())
    }
    pub fn commit(
        &mut self,
        repo: &str,
        parent: &str,
        branch: &str,
        message: &str,
        files: Vec<a::File>,
    ) -> Result<String> {
        let parent = u64::from_str_radix(parent.get(16..32).ok_or("invalid fixture sha")?, 16)?;
        let mut f = self.forge.borrow_mut();
        let config = f.config;
        let commit =
            domain::commit_tree(&mut f.domain, &config, parent, None, files.into_boxed_slice(), message.as_bytes())
                .map_err(|_| "fixture commit refused")?;
        drop(f);
        self.call(repo, BOT, a::Op::Git(a::Git::Push { branch: boxed(branch.as_bytes()), commit, expected: None }))?;
        let mut bytes = [0; 32];
        bytes[..20].copy_from_slice(&domain::object_bytes(commit));
        let hex = skein_forge::github::id::write_commit(&bytes).ok_or("invalid fixture sha")?;
        Ok(String::from_utf8(hex.to_vec())?)
    }
    pub fn fixtures() -> Result<Fixtures> {
        let key = fake::keys::KEYS[0].to_vec();
        let pem = String::from_utf8(fake::keys::pem(&key).ok_or("fixture PEM")?.into_vec())?;
        Ok(Fixtures {
            app: 8,
            slug: "example-app-conformance".into(),
            version: skein_forge::github::pin::API_VERSION.into(),
            key,
            pem,
            webhook: "invented-webhook-secret".into(),
            people: [
                ("acme".into(), "invented-acme-person-token".into()),
                ("globex".into(), "invented-globex-person-token".into()),
            ]
            .into(),
            installations: vec![
                Installation { id: 1, owner: "acme".into(), repositories: vec!["acme/widgets".into()] },
                Installation { id: 2, owner: "globex".into(), repositories: vec!["globex/gadgets".into()] },
            ],
            spare: "acme/spare".into(),
            source: None,
        })
    }
    pub fn shutdown(&mut self) -> Result<()> {
        self.peer.shutdown();
        for _ in 0..10000 {
            if self.peer.is_empty() {
                for pid in [self.peer_pid, self.client_pid] {
                    self.sim.assert_quiescent(pid);
                    self.sim.assert_no_open_fds(pid);
                }
                return Ok(());
            }
            self.pass();
        }
        Err("fake teardown bound".into())
    }
}
