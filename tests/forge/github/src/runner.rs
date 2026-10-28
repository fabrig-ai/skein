//! One request path for both targets, with write-ahead mutations and controlled cleanup.
use crate::{
    Result,
    fake::{self, World},
    journal::{Journal, Made},
    live::{Connector, Fixtures},
    mask::Mask,
    wire::{Request, Response},
};
use skein_forge::{
    api,
    github::{self, request::Method},
    marker,
};
use skein_forge_conformance::{
    facts::{Fact, Outcome},
    json,
    write::{self, SecretKind, Secrets},
};
use skein_lib::{Duration, Wall};
use std::{fmt::Write as _, fs, path::Path};
const WORD: &[u8] = b"conformance";
// Keep the App JWT's ten-minute life and renew at half life before an App request.
const JWT_LIFE: Duration = Duration::from_secs(600);
const JWT_HALF_LIFE: Duration = Duration::from_secs(300);

#[derive(Debug)]
pub struct Report {
    pub facts: Vec<Fact>,
    pub requests: usize,
    pub leftover_mints: Vec<String>,
}
#[expect(clippy::large_enum_variant, reason = "the runner owns one target for its entire lifetime")]
pub(crate) enum Target {
    Fake(World),
    Live(Connector),
}
pub(crate) struct Token {
    installation: u64,
    journal: usize,
    value: String,
}
pub(crate) struct Runner<'a> {
    #[cfg(test)]
    pub replay: Vec<(&'static str, Request)>,
    #[cfg(test)]
    pub phase: &'static str,
    #[cfg(test)]
    pub plan_refusal: Option<(&'static str, &'static str, usize)>,
    #[cfg(test)]
    pub enqueue_refusal: Option<&'static str>,
    #[cfg(test)]
    pub response_override: Option<(&'static str, &'static str, Vec<u8>)>,
    pub wire: Option<String>,
    pub target: Target,
    pub fixtures: Fixtures,
    pub secrets: Secrets,
    pub mask: Mask,
    pub journal: Journal,
    pub out: &'a Path,
    pub case: &'static str,
    pub requests: usize,
    pub case_limit: usize,
    pub cleanup: bool,
    pub jwt: String,
    jwt_signed_at: Option<Wall>,
    pub tokens: Vec<Token>,
    pub facts: Vec<Fact>,
    pub provenance: Vec<String>,
    pub serial: usize,
    pub queue_installation: Option<usize>,
    tables: github::plans::tables::Tables,
    repositories: std::collections::BTreeMap<String, u64>,
    cleanup_tokens: std::collections::BTreeMap<u64, String>,
    null_pulls: std::collections::BTreeSet<u64>,
    request_bytes: Vec<u8>,
    response_bytes: Vec<u8>,
    pub leftover_mints: Vec<String>,
}
impl<'a> Runner<'a> {
    pub fn new(target: Target, fixtures: Fixtures, out: &'a Path) -> Result<Self> {
        let secrets = fixtures.secrets();
        let journal = Journal::create(out, matches!(&target, Target::Live(_)), &secrets)?;
        if let Some(source) = &fixtures.source {
            let text = source.to_str().ok_or("fixture path is not text")?;
            write::check("fixture path", text.as_bytes(), b"", &secrets)?;
            fs::write(out.join("fixture-path.txt"), text)?;
        }
        Ok(Self::with_journal(target, fixtures, out, journal))
    }
    pub fn resume(target: Target, fixtures: Fixtures, out: &'a Path) -> Result<Self> {
        let (journal, live) = Journal::load(out)?;
        if live != matches!(&target, Target::Live(_)) {
            return Err("cleanup target differs from journal".into());
        }
        Ok(Self::with_journal(target, fixtures, out, journal))
    }
    fn with_journal(target: Target, fixtures: Fixtures, out: &'a Path, journal: Journal) -> Self {
        let secrets = fixtures.secrets();
        Self {
            #[cfg(test)]
            replay: vec![],
            #[cfg(test)]
            phase: "cases",
            #[cfg(test)]
            plan_refusal: None,
            #[cfg(test)]
            enqueue_refusal: None,
            #[cfg(test)]
            response_override: None,
            wire: None,
            target,
            fixtures,
            secrets,
            mask: Mask::new(0),
            journal,
            out,
            case: "bootstrap",
            requests: 0,
            case_limit: 320,
            cleanup: false,
            jwt: String::new(),
            jwt_signed_at: None,
            tokens: vec![],
            facts: vec![],
            provenance: vec![],
            serial: 0,
            queue_installation: None,
            tables: github::plans::tables::Tables::new(16, 512),
            repositories: std::collections::BTreeMap::new(),
            cleanup_tokens: std::collections::BTreeMap::new(),
            null_pulls: std::collections::BTreeSet::new(),
            request_bytes: vec![],
            response_bytes: vec![],
            leftover_mints: vec![],
        }
    }
    pub fn created_pull_null(&self, number: u64) -> bool {
        self.null_pulls.contains(&number)
    }
    pub fn fake(&self) -> bool {
        matches!(self.target, Target::Fake(_))
    }
    pub fn now(&self) -> Wall {
        match &self.target {
            Target::Fake(w) => w.now(),
            Target::Live(_) => skein_shell::Clock::new().now().wall,
        }
    }
    pub fn wait(&mut self, seconds: u64) {
        match &mut self.target {
            Target::Fake(w) => w.wait(seconds),
            Target::Live(_) => std::thread::sleep(std::time::Duration::from_secs(seconds)),
        }
    }
    pub fn wait_ci(&mut self) {
        self.wait(55);
    }
    pub fn ask(&mut self, request: &Request) -> Result<Response> {
        let restore: usize = self
            .journal
            .pending
            .iter()
            .map(|(_, made)| match made {
                Made::Mint { .. } | Made::Branch { .. } | Made::Access { .. } => 1,
                Made::Pull { .. } | Made::Issue { .. } => 3,
                Made::Rules { .. } => 2,
            })
            .sum();
        if self.requests >= 400
            || !self.cleanup && (self.requests >= self.case_limit || self.requests + restore + 8 >= 400)
        {
            return Err("conformance request budget exhausted".into());
        }
        self.requests += 1;
        #[cfg(test)]
        {
            let mut replay = Request::new(request.method, &request.target, None, &request.body);
            replay.blob = request.blob;
            self.replay.push((self.phase, replay));
        }
        #[cfg(test)]
        if self.enqueue_refusal == Some(self.case) && request.target == "/graphql" {
            let document = json::Json::parse(&request.body)?;
            if document.root().get("query").str().map(str::as_bytes) == Some(github::document::ENQUEUE) {
                self.enqueue_refusal = None;
                self.flush()?;
                return Ok(Response {
                    status: 200,
                    head: b"HTTP/1.1 200 OK\r\nContent-Type: application/json".to_vec(),
                    body: br#"{"data":{"enqueuePullRequest":null},"errors":[{"type":"UNPROCESSABLE","message":"invented queue admission refusal"}]}"#.to_vec(),
                });
            }
        }
        let (wire, response) = match self.exchange(request) {
            Ok(exchange) => exchange,
            Err(error) => {
                if let Some(failed) = error.downcast_ref::<crate::wire::ReadError>() {
                    self.record_failed(failed)?;
                    if !self.cleanup {
                        self.wire = Some(error.to_string());
                    }
                }
                return Err(error);
            }
        };
        if response.close_delimited() {
            self.provenance.push(format!("# {} close-delimited response {}", self.case, response.status));
        }
        self.record_created(request, &response)?;
        let credentials = request.target.ends_with("/access_tokens") || request.target.ends_with("/conversions");
        if credentials && response.status == 201 {
            let j = response.json()?;
            for (key, kind) in [
                ("token", SecretKind::MintedToken),
                ("pem", SecretKind::AppKey),
                ("webhook_secret", SecretKind::HookSecret),
            ] {
                if let Some(value) = j.root().get(key).str() {
                    crate::live::register(&mut self.secrets, kind, value);
                }
            }
            if let Some(token) = j.root().get("token").str() {
                let (id, installation) = self
                    .journal
                    .pending
                    .iter()
                    .rev()
                    .find_map(|(id, m)| match m {
                        Made::Mint { installation, .. } => Some((*id, *installation)),
                        Made::Access { .. }
                        | Made::Branch { .. }
                        | Made::Pull { .. }
                        | Made::Issue { .. }
                        | Made::Rules { .. } => None,
                    })
                    .ok_or("mint has no write-ahead entry")?;
                self.tokens.push(Token { installation, journal: id, value: token.into() });
            }
        }
        let at = wire.windows(4).position(|b| b == b"\r\n\r\n").ok_or("request head absent")?;
        let head = self.mask.headers(&wire[..at]);
        let body = self.mask.body(&wire[at + 4..], false);
        let masked_request = crate::wire::framed(&head, &body);
        let head = self.mask.headers(&response.head);
        let body = self.mask.body(&response.body, credentials);
        let masked_response = crate::wire::framed(&head, &body);
        write::check(self.case, &masked_request, &masked_response, &self.secrets)?;
        if !self.request_bytes.is_empty() {
            self.request_bytes.extend_from_slice(b"\r\n");
            self.response_bytes.extend_from_slice(b"\r\n");
        }
        self.request_bytes.extend_from_slice(&masked_request);
        self.response_bytes.extend_from_slice(&masked_response);
        Ok(response)
    }
    fn exchange(&mut self, request: &Request) -> Result<(Vec<u8>, Response)> {
        #[cfg(test)]
        if self.response_override.as_ref().map(|(case, target, _)| (*case, *target))
            == Some((self.case, request.target.as_str()))
        {
            let (_, _, received) = self.response_override.take().ok_or("response override absent")?;
            let wire = request.wire("127.0.0.1")?;
            let response = crate::wire::read(&wire, &received, true)?.ok_or("injected answer incomplete")?;
            return Ok((wire, response));
        }
        match &mut self.target {
            Target::Fake(w) => w.exchange(request),
            Target::Live(c) => c.exchange(request),
        }
    }
    fn record_failed(&mut self, failed: &crate::wire::ReadError) -> Result<()> {
        let end = failed.request.windows(4).position(|p| p == b"\r\n\r\n").ok_or("request head absent")?;
        let request = self.failure_head(&failed.request[..end]);
        let response = self.failure_head(&failed.head);
        write::check(self.case, &request, &response, &self.secrets)?;
        if !self.request_bytes.is_empty() {
            self.request_bytes.extend_from_slice(b"\r\n");
            self.response_bytes.extend_from_slice(b"\r\n");
        }
        self.request_bytes.extend_from_slice(&request);
        self.response_bytes.extend_from_slice(&response);
        Ok(())
    }
    fn failure_head(&mut self, head: &[u8]) -> Vec<u8> {
        let mut filtered = Vec::new();
        for line in head.split(|byte| *byte == b'\n') {
            let key = line.split(|byte| *byte == b':').next().unwrap_or_default();
            if [b"authorization".as_slice(), b"proxy-authorization", b"cookie", b"set-cookie"]
                .iter()
                .any(|name| key.eq_ignore_ascii_case(name))
            {
                continue;
            }
            filtered.extend_from_slice(line);
            filtered.push(b'\n');
        }
        self.mask.headers(&filtered)
    }
    fn record_created(&mut self, request: &Request, response: &Response) -> Result<()> {
        if request.method == Method::Post
            && (response.status == 201 || (400..500).contains(&response.status))
            && (request.target.ends_with("/issues") || request.target.ends_with("/pulls"))
        {
            let input = json::Json::parse(&request.body)?;
            let id = self.journal.pending.iter().rev().find_map(|(id, made)| match made {
                Made::Issue { repo, title }
                    if request.target == format!("/repos/{repo}/issues") && input.root().get("title").is(title) =>
                {
                    Some(*id)
                }
                Made::Pull { repo, head, base }
                    if request.target == format!("/repos/{repo}/pulls")
                        && input.root().get("head").is(head)
                        && input.root().get("base").is(base) =>
                {
                    Some(*id)
                }
                Made::Branch { .. }
                | Made::Pull { .. }
                | Made::Issue { .. }
                | Made::Rules { .. }
                | Made::Access { .. }
                | Made::Mint { .. } => None,
            });
            if response.status == 201 {
                let id = id.ok_or("created object has no write-ahead entry")?;
                let number = response.json()?.root().get("number").u64().ok_or("created number absent")?;
                self.journal.created(id, number, &self.secrets)?;
                if request.target.ends_with("/pulls")
                    && response.json()?.root().get("mergeable").kind() == Some(skein_json::Kind::Null)
                {
                    self.null_pulls.insert(number);
                }
            } else if let Some(id) = id
                && !self.journal.numbers.contains_key(&id)
            {
                self.journal.cancel(id, &self.secrets)?;
            }
        }
        Ok(())
    }
    pub fn raw(&mut self, method: Method, target: impl Into<String>, token: &str, body: &[u8]) -> Result<Response> {
        self.ask(&Request::new(method, target, (!token.is_empty()).then_some(token), body))
    }
    pub fn app(&mut self, method: Method, target: impl Into<String>, body: &[u8]) -> Result<Response> {
        let renew = match self.jwt_signed_at {
            Some(at) => self.now().as_nanos().saturating_sub(at.as_nanos()) >= JWT_HALF_LIFE.as_nanos(),
            None => true,
        };
        if renew {
            self.sign()?;
        }
        let token = self.jwt.clone();
        self.raw(method, target, &token, body)
    }
    pub fn token(&self, index: usize) -> Result<String> {
        let installation = self.fixtures.installations.get(index).ok_or("installation absent")?.id;
        self.tokens
            .iter()
            .find(|t| t.installation == installation)
            .map(|t| t.value.clone())
            .ok_or("installation token absent".into())
    }
    pub fn person(&self, index: usize) -> Result<String> {
        let owner = &self.fixtures.installations.get(index).ok_or("installation absent")?.owner;
        self.fixtures.people.get(owner).cloned().ok_or("person token absent".into())
    }
    pub fn repo(&self, index: usize) -> Result<String> {
        self.fixtures
            .installations
            .get(index)
            .and_then(|i| i.repositories.first())
            .cloned()
            .ok_or("probe repository absent".into())
    }
    pub fn before(&mut self, made: Made) -> Result<usize> {
        self.journal.before(made, &self.secrets)
    }
    pub fn mint(&mut self, index: usize, contents: bool) -> Result<String> {
        let installation = self.fixtures.installations.get(index).ok_or("installation absent")?.id;
        let at = self.now().as_nanos() / 1_000_000_000;
        self.before(Made::Mint { installation, at })?;
        let response = self.app(
            Method::Post,
            format!("/app/installations/{installation}/access_tokens"),
            if contents { br#"{"permissions":{"contents":"write"}}"# } else { b"" },
        )?;
        if response.status != 201 {
            return Err("mint was refused".into());
        }
        if contents {
            let json = response.json()?;
            let permissions = json.root().get("permissions");
            if !permissions.get("contents").is("write") || !permissions.get("metadata").is("read") {
                return Err("contents mint permissions differ from contents write and metadata read".into());
            }
            self.provenance.push(
                "# token-narrowed-creates-not-edits mint permissions object recorded in response transcript".into(),
            );
        }
        Ok(response.json()?.root().get("token").str().ok_or("mint token absent")?.into())
    }
    pub fn sign(&mut self) -> Result<()> {
        let now = self.now();
        let input =
            github::jwt::signing_input(self.fixtures.app, now, Wall::from_nanos(now.as_nanos() + JWT_LIFE.as_nanos()))
                .ok_or("JWT window invalid")?;
        let signature = skein_tls::sign_rs256(&self.fixtures.key, &input).ok_or("JWT signing failed")?;
        let token = github::jwt::token(&input, &signature).ok_or("JWT encoding failed")?;
        self.jwt = String::from_utf8(token.expose().to_vec())?;
        crate::live::register(&mut self.secrets, SecretKind::Jwt, &self.jwt);
        self.jwt_signed_at = Some(now);
        Ok(())
    }
    pub fn start(&mut self) -> Result<()> {
        if !marker::valid_word(WORD) {
            return Err("conformance marker word invalid".into());
        }
        self.sign()?;
        self.provenance = vec![
            format!("# target {}", if self.fake() { "fake" } else { "github.com" }),
            format!("# api_version {}", self.fixtures.version),
            format!("# app {}", self.fixtures.slug),
        ];
        for index in 0..self.fixtures.installations.len() {
            let person = self.person(index)?;
            let repo = self.repo(index)?;
            let r = self.raw(Method::Get, format!("/repos/{repo}"), &person, b"")?;
            if r.status != 200 {
                return Err("probe repository discovery refused".into());
            }
            if self.queue_installation.is_none() && r.json()?.root().get("owner").get("type").is("Organization") {
                self.queue_installation = Some(index);
            }
            if let Some(expires) = r.header("github-authentication-token-expiration") {
                let rfc = expires.replace(' ', "T");
                let date = format!("{}Z", rfc.get(..19).ok_or("expiration header invalid")?);
                let expiry = skein_forge::forgejo::time::read(date.as_bytes()).ok_or("expiration header invalid")?;
                if expiry.as_nanos() <= self.now().as_nanos() + 30 * 86400 * 1_000_000_000 {
                    self.provenance.push(format!(
                        "# person-{}.token expires within 30 days",
                        self.fixtures.installations[index].owner
                    ));
                }
            }
        }
        if self.queue_installation.is_none() {
            return Err("no organization installation for the merge-queue cases".into());
        }
        if self.fake() {
            self.bootstrap()?;
        }
        for index in 0..self.fixtures.installations.len() {
            self.mint(index, false)?;
        }
        self.flush()?;
        Ok(())
    }
    fn bootstrap(&mut self) -> Result<()> {
        #[cfg(test)]
        {
            self.phase = "setup-lifecycle";
        }
        let manifest=br#"{"name":"example-app-conformance","url":"https://example-app.invalid/","redirect_url":"http://127.0.0.1/callback","setup_url":"http://127.0.0.1/setup","public":true,"hook_attributes":{"url":"https://example-app.invalid/","active":false},"default_permissions":{"contents":"write","issues":"write","pull_requests":"write","checks":"read","statuses":"write","administration":"write","actions":"read","metadata":"read"},"default_events":["push","pull_request","issues","issue_comment","pull_request_review","check_run","merge_group"]}"#;
        let form = format!("manifest={}", percent(manifest));
        let r = self.raw(Method::Post, "/settings/apps/new?state=example-state", "", form.as_bytes())?;
        let at = r.body.windows(6).position(|b| b == b"?code=").ok_or("bootstrap code absent")? + 6;
        let end = r.body[at..].iter().position(|b| *b == b'&').ok_or("bootstrap code end absent")? + at;
        let code = std::str::from_utf8(&r.body[at..end])?.to_owned();
        let r = self.raw(Method::Post, format!("/app-manifests/{code}/conversions"), "", b"")?;
        if r.status != 201 {
            return Err("bootstrap conversion refused".into());
        }
        self.fixtures.webhook = r.json()?.root().get("webhook_secret").str().ok_or("bootstrap webhook absent")?.into();
        for (account, query) in [("acme", "&repository_ids=2"), ("globex", "&repository_ids=4")] {
            let r = self.raw(
                Method::Get,
                format!("/apps/{}/installations/new?state=install-state&account={account}{query}", self.fixtures.slug),
                "",
                b"",
            )?;
            if r.status != 200 && r.status != 302 {
                return Err("bootstrap installation refused".into());
            }
        }
        #[cfg(test)]
        {
            self.phase = "cases";
        }
        Ok(())
    }
    pub fn flush(&mut self) -> Result<()> {
        if !self.request_bytes.is_empty() || !self.out.join("transcripts").join(self.case).join("request").exists() {
            write::case(self.out, self.case, &self.request_bytes, &self.response_bytes, &self.secrets)?;
            self.request_bytes.clear();
            self.response_bytes.clear();
        }
        Ok(())
    }
    pub fn next(&mut self, name: &'static str) -> Result<()> {
        self.wire = None;
        self.flush()?;
        self.case = name;
        Ok(())
    }
    pub fn fact(&mut self, name: &'static str, holds: bool, why: &str) {
        self.facts.push(Fact { name, outcome: if holds { Outcome::Holds } else { Outcome::Fails(why.into()) } });
    }
    pub fn name(&mut self, kind: &str) -> String {
        self.serial += 1;
        format!("conformance-{kind}-{}", self.serial)
    }
    pub fn branch(
        &mut self,
        index: usize,
        parent: &str,
        message: &str,
        files: Vec<skein_fake_forge_domain::api::File>,
    ) -> Result<(String, String)> {
        let repo = self.repo(index)?;
        let branch = self.name("branch");
        self.before(Made::Branch { repo: repo.clone(), name: branch.clone() })?;
        let sha = match &mut self.target {
            Target::Fake(w) => w.commit(&repo, parent, &branch, message, files)?,
            Target::Live(_) => {
                let person = self.person(index)?;
                let parent_doc =
                    self.raw(Method::Get, format!("/repos/{repo}/git/commits/{parent}"), &person, b"")?.json()?;
                let parent_tree = parent_doc.root().get("tree").get("sha").str().ok_or("parent tree absent")?;
                let entries = json::write(|out| {
                    out.object_start();
                    out.key(b"base_tree");
                    out.string(parent_tree.as_bytes());
                    out.key(b"tree");
                    out.array_start();
                    for file in &files {
                        out.object_start();
                        out.key(b"path");
                        out.string(&file.path);
                        out.key(b"mode");
                        out.string(b"100644");
                        out.key(b"type");
                        out.string(b"blob");
                        out.key(b"content");
                        out.string(&file.content);
                        out.object_end();
                    }
                    out.array_end();
                    out.object_end();
                })?;
                let tree = self
                    .raw(Method::Post, format!("/repos/{repo}/git/trees"), &person, &entries)?
                    .json()?
                    .root()
                    .get("sha")
                    .str()
                    .ok_or("tree sha absent")?
                    .to_owned();
                let body = json::object(&[("message", &message), ("parents", &[parent]), ("tree", &tree)])?;
                let r = self.raw(Method::Post, format!("/repos/{repo}/git/commits"), &person, &body)?;
                let sha = r.json()?.root().get("sha").str().ok_or("commit sha absent")?.to_owned();
                let body = json::object(&[("ref", &format!("refs/heads/{branch}")), ("sha", &sha)])?;
                let r = self.raw(Method::Post, format!("/repos/{repo}/git/refs"), &person, &body)?;
                if r.status != 201 {
                    return Err("branch creation refused".into());
                }
                sha
            }
        };
        Ok((branch, sha))
    }
    pub fn base(&mut self, index: usize) -> Result<String> {
        let answer = self
            .plan(index, api::Op::Read(api::Read::Branch { branch: fake::boxed(b"main") }))?
            .map_err(|e| format!("default branch plan failed: {e:?}"))?;
        let api::Answer::Commit(commit) = answer else {
            return Err("default branch plan answer invalid".into());
        };
        Ok(String::from_utf8(github::id::write_commit(&commit).ok_or("invalid branch oid")?.to_vec())?)
    }
    pub fn pull(&mut self, index: usize, head: &str, base: &str, person: bool) -> Result<u64> {
        let repo = self.repo(index)?;
        self.before(Made::Pull { repo, head: head.into(), base: base.into() })?;
        let answer = self
            .plan_as(
                index,
                api::Op::Write(api::Write::OpenPull {
                    title: fake::boxed(b"conformance"),
                    body: fake::boxed(b"conformance"),
                    head: fake::boxed(head.as_bytes()),
                    base: fake::boxed(base.as_bytes()),
                }),
                person,
            )?
            .map_err(|e| format!("pull plan failed: {e:?}"))?;
        let api::Answer::Created(number) = answer else {
            return Err("pull plan answer invalid".into());
        };
        Ok(number)
    }
    pub fn issue(&mut self, index: usize) -> Result<u64> {
        let repo = self.repo(index)?;
        let title = self.name("issue");
        self.before(Made::Issue { repo: repo.clone(), title: title.clone() })?;
        let marked = marker::encode(WORD, title.as_bytes(), title.as_bytes(), fake::bounds().body_bytes)
            .ok_or("issue marker exceeds body bound")?;
        let body = json::object(&[("body", &std::str::from_utf8(&marked)?), ("title", &title)])?;
        let token = self.token(index)?;
        let answer = self.raw(Method::Post, format!("/repos/{repo}/issues"), &token, &body)?;
        if answer.status != 201 {
            return Err("issue fixture POST refused".into());
        }
        answer.json()?.root().get("number").u64().ok_or("issue number absent".into())
    }
    pub fn copy_branch(&mut self, index: usize, parent: &str) -> Result<String> {
        let repo = self.repo(index)?;
        let name = self.name("base");
        self.before(Made::Branch { repo, name: name.clone() })?;
        self.plan(
            index,
            api::Op::Write(api::Write::CreateBranch {
                branch: fake::boxed(name.as_bytes()),
                commit: github::id::commit(parent.as_bytes()).ok_or("invalid base oid")?,
            }),
        )?
        .map_err(|e| format!("base branch plan failed: {e:?}"))?;
        Ok(name)
    }
    pub fn advance(
        &mut self,
        index: usize,
        parent: &str,
        branch: &str,
        message: &str,
        files: Vec<skein_fake_forge_domain::api::File>,
    ) -> Result<String> {
        let repo = self.repo(index)?;
        if let Target::Fake(w) = &mut self.target {
            return w.commit(&repo, parent, branch, message, files);
        }
        let (_, sha) = self.branch(index, parent, message, files)?;
        let token = self.person(index)?;
        let body = json::object(&[("sha", &sha)])?;
        let r = self.raw(
            Method::Patch,
            format!("/repos/{repo}/git/refs/heads/{}", percent(branch.as_bytes())),
            &token,
            &body,
        )?;
        if r.status != 200 {
            return Err("head advance refused".into());
        }
        Ok(sha)
    }
    pub fn plan(&mut self, index: usize, op: api::Op) -> Result<std::result::Result<api::Answer, api::Error>> {
        self.plan_as(index, op, false)
    }
    #[expect(
        clippy::too_many_lines,
        reason = "the dialect plan driver keeps request, document and redirect handling together"
    )]
    pub fn plan_as(
        &mut self,
        index: usize,
        op: api::Op,
        person: bool,
    ) -> Result<std::result::Result<api::Answer, api::Error>> {
        use github::{
            document::Kind,
            plans::{
                Body, Heard,
                plan::{Bounds, Plan, Step},
            },
        };
        #[cfg(test)]
        if let Some((case, kind, skip)) = &mut self.plan_refusal {
            let operation = match &op {
                api::Op::Write(api::Write::Review { .. }) => "approval",
                api::Op::Write(api::Write::Merge { .. }) => "enqueue",
                api::Op::Read(api::Read::Checks { .. }) => "checks",
                api::Op::Read(api::Read::Branches { .. }) => "branches",
                api::Op::Read(api::Read::Branch { branch }) if branch.starts_with(b"gh-readonly-queue/") => "group",
                api::Op::Read(api::Read::Pull { .. }) => "pull",
                api::Op::Read(_) | api::Op::Write(_) => "other",
            };
            if *case == self.case && *kind == operation {
                if *skip == 0 {
                    self.plan_refusal = None;
                    return Ok(Err(api::Error::Forbidden));
                }
                *skip -= 1;
            }
        }
        let repo = self.repo(index)?;
        let token = if person { self.person(index)? } else { self.token(index)? };
        let located = if let Some(id) = self.repositories.get(&repo) {
            *id
        } else {
            let id = self
                .raw(Method::Get, format!("/repos/{repo}"), &token, b"")?
                .json()?
                .root()
                .get("id")
                .u64()
                .ok_or("repository id absent")?;
            self.repositories.insert(repo, id);
            id
        };
        let mut plan = Plan::new(api::Repository { forge: 1, repository: u32::try_from(located)? }, op);
        let bounds = Bounds { inner: 512, page_size: 100, rows: 512, answer_bytes: 1_048_576, body_bytes: 524_288 };
        let mut step =
            plan.begin(bounds, WORD, &mut self.tables).map_err(|e| format!("dialect plan refused to begin: {e:?}"))?;
        for _ in 0..64 {
            let events = match step {
                Step::Done(result) => return Ok(result),
                Step::Send(exchange) => {
                    let encoded = github::request::encode(&exchange.request, &fake::bounds())
                        .map_err(|e| format!("dialect request exceeds bounds: {e:?}"))?;
                    let mut request = Request::new(
                        encoded.method,
                        String::from_utf8(encoded.target.into_vec())?,
                        Some(&token),
                        encoded.body.as_deref().unwrap_or_default(),
                    );
                    if let Some(v) = encoded.validator {
                        request.headers.push(("If-None-Match".into(), String::from_utf8(v.as_bytes().to_vec())?));
                    }
                    let r = self.ask(&request)?;
                    let mut head = github::header::Head::new();
                    for line in r.head.split(|b| *b == b'\n').skip(1) {
                        if let Some(at) = line.iter().position(|b| *b == b':') {
                            github::header::field(
                                &mut head,
                                &line[..at],
                                line[at + 1..].strip_suffix(b"\r").unwrap_or(&line[at + 1..]).trim_ascii(),
                                &fake::bounds(),
                            );
                        }
                    }
                    let mut events = vec![Heard::Head { status: r.status, head }];
                    if r.status == 204 || r.status == 304 {
                        events.push(Heard::Done(Body::Empty));
                    } else if r.status != 302 {
                        let kind = if r.status < 300 {
                            github::decode::kind(&exchange.request).ok_or("dialect response kind absent")?
                        } else {
                            Kind::Error
                        };
                        let (rows, doc) = decode(&r.body, kind)?;
                        events.extend(rows.into_iter().map(Heard::Row));
                        events.push(Heard::Done(Body::Document(doc)));
                    }
                    events
                }
                Step::Redirect { redirect, retention } => {
                    let host = std::str::from_utf8(&redirect.origin.host)?;
                    let target = std::str::from_utf8(&redirect.target)?;
                    let mut request = Request::new(
                        Method::Get,
                        if self.fake() { target.into() } else { format!("https://{host}{target}") },
                        None,
                        b"",
                    );
                    request.blob = true;
                    let r = self.ask(&request)?;
                    let github::plans::plan::Retention::Tail { bytes } = retention else {
                        return Err("unexpected log retention".into());
                    };
                    let start = r.body.len().saturating_sub(usize::try_from(bytes)?);
                    vec![
                        Heard::Head { status: r.status, head: github::header::Head::new() },
                        Heard::Done(Body::Raw { bytes: fake::boxed(&r.body[start..]), more: start != 0 }),
                    ]
                }
            };
            let mut next = None;
            for event in events {
                if let Some(s) = plan
                    .heard(event, bounds, WORD, &mut self.tables)
                    .map_err(|e| format!("dialect plan rejected response: {e:?}"))?
                {
                    next = Some(s);
                    break;
                }
            }
            step = next.ok_or("dialect plan did not advance")?;
        }
        Err("dialect plan progress bound".into())
    }
    pub fn remove_rules(&mut self) -> Result<()> {
        for (id, made) in self.journal.pending.clone() {
            if matches!(made, Made::Rules { .. }) {
                self.undo(id, &made)?;
            }
        }
        Ok(())
    }
    pub fn finish(&mut self) -> Result<()> {
        self.cleanup = true;
        let mut error = self.flush().err();
        self.request_bytes.clear();
        self.response_bytes.clear();
        self.case = "cleanup";
        self.cleanup_tokens.clear();
        for (id, made) in self.journal.pending.clone().into_iter().rev() {
            let result = self.undo(id, &made);
            if let Err(e) = result
                && error.is_none()
            {
                error = Some(e);
            }
        }
        // Revocation is independent of repository restoration and transcript failures.
        for token in std::mem::take(&mut self.tokens) {
            if !self.journal.pending.iter().any(|(id, _)| *id == token.journal) {
                continue;
            }
            let result = self.raw(Method::Delete, "/installation/token", &token.value, b"");
            match result {
                Ok(r) if r.status == 204 || r.status == 401 => {
                    if let Err(e) = self.journal.done(token.journal, &self.secrets)
                        && error.is_none()
                    {
                        error = Some(e);
                    }
                }
                Ok(_) => {
                    self.tokens.push(token);
                    if error.is_none() {
                        error = Some("minted token revocation refused".into());
                    }
                }
                Err(e) => {
                    self.tokens.push(token);
                    if error.is_none() {
                        error = Some(e);
                    }
                }
            }
        }
        self.cleanup_tokens.clear();
        self.flush()?;
        if let Some(e) = error {
            return Err(e);
        }
        Ok(())
    }
    fn cleanup_token(&mut self, repo: &str) -> Result<String> {
        let index = self
            .fixtures
            .installations
            .iter()
            .position(|installation| installation.repositories.iter().any(|name| name == repo))
            .ok_or("cleanup installation absent")?;
        let installation = self.fixtures.installations[index].id;
        if let Some(token) = self.cleanup_tokens.get(&installation) {
            return Ok(token.clone());
        }
        self.sign()?;
        let token = self.mint(index, false)?;
        self.cleanup_tokens.insert(installation, token.clone());
        Ok(token)
    }
    #[expect(clippy::too_many_lines, reason = "each durable intention has its restoration beside the others")]
    fn undo(&mut self, id: usize, made: &Made) -> Result<()> {
        match made {
            Made::Mint { installation, at } => {
                if self.tokens.iter().any(|t| t.journal == id) {
                    return Ok(());
                }
                let report =
                    format!("minted token for installation {installation}: not revoked, expires by {}", at + 3600);
                cleanup_report(self.out, std::slice::from_ref(&report), &self.secrets)?;
                self.leftover_mints.push(report);
            }
            Made::Branch { repo, name } => {
                let owner = repo.split_once('/').ok_or("invalid cleanup repository")?.0;
                let token = self.fixtures.people.get(owner).ok_or("cleanup owner absent")?.clone();
                let r = self.raw(
                    Method::Delete,
                    format!("/repos/{repo}/git/refs/heads/{}", percent(name.as_bytes()).replace("%2F", "/")),
                    &token,
                    b"",
                )?;
                if r.status != 204 && !r.is(422, "Reference does not exist") {
                    return Err("branch cleanup refused".into());
                }
            }
            Made::Pull { repo, .. } | Made::Issue { repo, .. } => {
                let token = self.cleanup_token(repo)?;
                if let Some(number) = self.journal.numbers.get(&id).copied() {
                    self.close_number(repo, number, &token)?;
                } else {
                    let mut page = 1;
                    let mut found = false;
                    loop {
                        let target = format!(
                            "/repos/{repo}/issues?state=all&sort=updated&direction=asc&per_page=100&page={page}"
                        );
                        let response = self.raw(Method::Get, target, &token, b"")?;
                        if response.status != 200 {
                            return Err("cleanup lookup refused".into());
                        }
                        let more = response.header("link").is_some_and(|v| v.contains("rel=\"next\""));
                        for row in response.json()?.root().items() {
                            let belongs = match made {
                                Made::Issue { title, .. } => row.get("title").is(title),
                                Made::Pull { head, base, .. } if row.get("pull_request").kind().is_some() => {
                                    let number = row.get("number").u64().ok_or("cleanup pull number absent")?;
                                    let pull =
                                        self.raw(Method::Get, format!("/repos/{repo}/pulls/{number}"), &token, b"")?;
                                    if pull.status != 200 {
                                        return Err("cleanup pull read refused".into());
                                    }
                                    let doc = pull.json()?;
                                    doc.root().get("head").get("ref").is(head)
                                        && doc.root().get("base").get("ref").is(base)
                                }
                                Made::Branch { .. }
                                | Made::Pull { .. }
                                | Made::Rules { .. }
                                | Made::Access { .. }
                                | Made::Mint { .. } => false,
                            };
                            if belongs {
                                let number = row.get("number").u64().ok_or("cleanup number absent")?;
                                self.close_number(repo, number, &token)?;
                                found = true;
                            }
                        }
                        if !more {
                            break;
                        }
                        page += 1;
                    }
                    if !found {
                        return Err("cleanup object not found; intention retained".into());
                    }
                }
            }
            Made::Rules { repo, name } => {
                let owner = repo.split_once('/').ok_or("invalid cleanup repository")?.0;
                let token = self.fixtures.people.get(owner).ok_or("cleanup owner absent")?.clone();
                let ids = if self.fake() {
                    vec![12_345_678]
                } else {
                    let r = self.raw(Method::Get, format!("/repos/{repo}/rulesets?per_page=100"), &token, b"")?;
                    if r.status != 200 {
                        return Err("ruleset cleanup lookup refused".into());
                    }
                    r.json()?
                        .root()
                        .items()
                        .iter()
                        .filter(|row| row.get("name").is(name))
                        .filter_map(|row| row.get("id").u64())
                        .collect()
                };
                for rule in ids {
                    let r = self.raw(Method::Delete, format!("/repos/{repo}/rulesets/{rule}"), &token, b"")?;
                    if r.status != 204 && r.status != 404 {
                        return Err("ruleset cleanup refused".into());
                    }
                }
            }
            Made::Access { installation, repository, owner, add } => {
                let token = self.fixtures.people.get(owner).ok_or("cleanup person absent")?.clone();
                let r = self.raw(
                    if *add { Method::Put } else { Method::Delete },
                    format!("/user/installations/{installation}/repositories/{repository}"),
                    &token,
                    b"",
                )?;
                if r.status != 204 {
                    return Err("access restoration refused".into());
                }
            }
        }
        self.journal.done(id, &self.secrets)
    }
    fn close_number(&mut self, repo: &str, number: u64, token: &str) -> Result<()> {
        let target = format!("/repos/{repo}/issues/{number}");
        let first = self.raw(Method::Get, &target, token, b"")?;
        if first.status != 200 {
            return Err("cleanup object read refused".into());
        }
        if !first.json()?.root().get("state").is("closed") {
            let response = self.raw(Method::Patch, &target, token, br#"{"state":"closed"}"#)?;
            if response.status != 200 {
                return Err("cleanup close refused".into());
            }
            let read = self.raw(Method::Get, &target, token, b"")?;
            if read.status != 200 || !read.json()?.root().get("state").is("closed") {
                return Err("cleanup object is not closed; intention retained".into());
            }
        }
        Ok(())
    }
    pub fn complete(&mut self, run: Result<()>) -> Result<()> {
        let cleanup = self.finish();
        let setup = if run.is_ok() && cleanup.is_ok() { crate::cases::setup_lifecycle(self) } else { Ok(()) };
        let report = self.report();
        run?;
        cleanup?;
        setup?;
        report
    }
    pub fn report(&self) -> Result<()> {
        let mut text = self.provenance.join("\n");
        text.push('\n');
        for leftover in &self.leftover_mints {
            text.push_str("# ");
            text.push_str(leftover);
            text.push('\n');
        }
        for fact in &self.facts {
            text.push_str(&fact.line());
        }
        write::check("facts", text.as_bytes(), b"", &self.secrets)?;
        fs::write(self.out.join("facts.txt"), text)?;
        Ok(())
    }
}
pub(crate) fn percent(bytes: &[u8]) -> String {
    let mut result = String::new();
    for byte in bytes {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(byte) {
            result.push(char::from(*byte));
        } else {
            write!(result, "%{byte:02X}").expect("String accepts formatting");
        }
    }
    result
}
fn decode(
    bytes: &[u8],
    kind: github::document::Kind,
) -> Result<(Vec<github::document::Row>, github::document::Document)> {
    use skein_json::tokenizer;
    use skein_lib::{
        Env, Intake, Queue, Time,
        stream::{Down, Up},
    };
    let bounds = fake::bounds();
    let mut decoder = github::decode::decoder(kind, &bounds);
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: bounds.json };
    let mut tokenizer = tokenizer::Tokenizer::new(&bounds.json);
    let mut above = Queue::with_capacity(1);
    let mut below = Queue::with_capacity(1);
    let mut intake = Intake::with_capacity(bounds.writer.length);
    intake.append(bytes).map_err(|e| format!("dialect intake bound: {e:?}"))?;
    let mut rows = vec![];
    tokenizer::down(&mut tokenizer, &env, tokenizer::Request::Next, &mut above, &mut below);
    for _ in 0..bytes.len() * 8 + 64 {
        if let Some(event) = above.pop() {
            let (request, step) = decoder.step(event);
            match step {
                github::decode::Step::Demand => {}
                github::decode::Step::Row(row) => rows.push(row),
                github::decode::Step::Done(document) => return Ok((rows, document)),
                github::decode::Step::Malformed(_) => return Err("dialect document malformed".into()),
            }
            tokenizer::down(&mut tokenizer, &env, request, &mut above, &mut below);
        } else {
            let Down::Demand { read, .. } = below.pop().ok_or("JSON demand absent")? else {
                return Err("JSON demand invalid".into());
            };
            let up = intake.meet(read).map_or(Up::End, Up::Bytes);
            tokenizer::up(&mut tokenizer, &env, up, &mut above, &mut below);
        }
    }
    Err("dialect decoding progress bound".into())
}
/// Runs only the in-process hosted fake.
/// # Errors
/// Returns a fixture, request, transcript, leak or cleanup failure.
pub fn run_fake(out: &Path) -> Result<Report> {
    let world = World::new()?;
    let fixtures = World::fixtures()?;
    execute(Runner::new(Target::Fake(world), fixtures, out)?)
}
/// The fixture owner's opt-in entry point. Never called by a suite.
/// # Errors
/// Refuses insecure fixtures, disallowed mutations, leaks and budget excess.
pub fn run_live(dir: &Path, out: &Path) -> Result<Report> {
    let fixtures = Fixtures::load(dir)?;
    let connector = Connector::new(&fixtures)?;
    execute(Runner::new(Target::Live(connector), fixtures, out)?)
}
fn execute(mut runner: Runner<'_>) -> Result<Report> {
    let run = runner.start().and_then(|()| crate::cases::run(&mut runner)).and_then(|()| {
        let found: std::collections::BTreeSet<_> = runner.facts.iter().map(|f| f.name).collect();
        if found.len() != runner.facts.len() || found != crate::cases::NAMES.iter().copied().collect() {
            return Err("conformance case names differ from the contract".into());
        }
        Ok(())
    });
    let result = runner.complete(run);
    let shutdown = match &mut runner.target {
        Target::Fake(w) => w.shutdown(),
        Target::Live(_) => Ok(()),
    };
    result?;
    shutdown?;
    Ok(Report { facts: runner.facts, requests: runner.requests, leftover_mints: runner.leftover_mints })
}
/// Finishes recorded repository intentions; unavailable minted values are reported by expiry.
/// # Errors
/// Refuses a malformed journal or missing/insecure live fixtures and cleanup failures.
pub fn clean(out: &Path, live: Option<&Path>) -> Result<()> {
    let (journal, is_live) = Journal::load(out)?;
    if !is_live {
        // No hosted state survives a fake process; only unavailable mint reports remain.
        let fixtures = World::fixtures()?;
        let secrets = fixtures.secrets();
        let mut journal = journal;
        let mut leftovers = vec![];
        for (id, made) in journal.pending.clone() {
            if let Made::Mint { installation, at } = made {
                let report =
                    format!("minted token for installation {installation}: not revoked, expires by {}", at + 3600);
                cleanup_report(out, std::slice::from_ref(&report), &secrets)?;
                leftovers.push(report);
            }
            journal.done(id, &secrets)?;
        }
        return cleanup_report(out, &leftovers, &secrets);
    }
    let fixtures = if let Some(dir) = live {
        Fixtures::load(dir)?
    } else {
        let saved = fs::read_to_string(out.join("fixture-path.txt"))?;
        Fixtures::load(Path::new(&saved))?
    };
    let target = Target::Live(Connector::new(&fixtures)?);
    let mut r = Runner::resume(target, fixtures, out)?;
    let result = r.finish();
    cleanup_report(out, &r.leftover_mints, &r.secrets)?;
    result?;
    match &mut r.target {
        Target::Fake(w) => w.shutdown(),
        Target::Live(_) => Ok(()),
    }
}

fn cleanup_report(out: &Path, leftovers: &[String], secrets: &Secrets) -> Result<()> {
    let path = out.join("facts.txt");
    let mut text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "# resumed cleanup\n".into(),
        Err(e) => return Err(e.into()),
    };
    for leftover in leftovers {
        let line = format!("# {leftover}");
        if !text.lines().any(|existing| existing == line) {
            writeln!(text, "{line}")?;
        }
    }
    write::check("facts", text.as_bytes(), b"", secrets)?;
    fs::write(&path, text)?;
    fs::File::open(path)?.sync_all()?;
    fs::File::open(out)?.sync_all()?;
    Ok(())
}
