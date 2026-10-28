//! Shared pending reply rights and scenario tables (fake-forge.md, 4).
use crate::{Config, Limits, Tally};
use alloc::boxed::Box;
use skein_fake_forge_domain as domain;
use skein_lib::{Id, List, ReplyTo, Rng, Slab, Token};
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Error {
    Limits,
}
#[derive(Debug)]
pub(crate) struct Call {
    pub server: Token,
    pub active: bool,
}
#[derive(Debug)]
pub struct Service {
    pub(crate) config: Config,
    pub(crate) calls: Slab<Call>,
    pub(crate) rng: Rng,
    pub(crate) tally: Tally,
    pub(crate) setup: crate::setup::Store,
}
impl Service {
    pub fn new(mut config: Config, limits: &Limits, seed: u64) -> Result<Self, Error> {
        if let Some(endpoint) = &config.hooks
            && (endpoint.host.is_empty() || endpoint.secret.is_empty())
        {
            return Err(Error::Limits);
        }
        if !valid(limits, &config)
            || config.max_response_items == 0
            || config.default_paging_num == 0
            || config.max_response_items > limits.documents.inner
            || config.faults.reset > 1000
            || config.faults.drop > 1000
            || config.faults.server_error > 1000
        {
            return Err(Error::Limits);
        }
        check_base(&config.base, limits)?;
        if config.users.len() > limits.documents.inner
            || config.repositories.len() > limits.documents.inner
            || config.tokens.len() > limits.documents.inner
        {
            return Err(Error::Limits);
        }
        for (index, (token, user)) in config.tokens.iter().enumerate() {
            if token.is_empty() || token.len() > usize::try_from(limits.http.head).expect("u32 fits") || *user == 0 {
                return Err(Error::Limits);
            }
            for (prior, _) in config.tokens.iter().take(index) {
                if prior == token {
                    return Err(Error::Limits);
                }
            }
        }
        for (_, login) in &config.users {
            if login.len() > usize::try_from(limits.documents.string_bytes).expect("u32 fits") {
                return Err(Error::Limits);
            }
        }
        for (index, repo) in config.repositories.iter().enumerate() {
            if repo.id == 0 || repo.full.len() > usize::try_from(limits.documents.target_bytes).expect("u32 fits") {
                return Err(Error::Limits);
            }
            if let Some(branch) = &repo.protected
                && branch.len() > usize::try_from(limits.documents.string_bytes).expect("u32 fits")
            {
                return Err(Error::Limits);
            }
            for prior in config.repositories.iter().take(index) {
                if prior.id == repo.id || prior.full == repo.full {
                    return Err(Error::Limits);
                }
            }
        }
        let mut repositories = List::with_capacity(limits.documents.inner);
        for repository in config.repositories.into_boxed() {
            repositories.push(repository).expect("checked repository count");
        }
        config.repositories = repositories;
        let tally = Tally::new(config.tokens.len().saturating_add(limits.documents.inner));
        let setup = crate::setup::Store::new(limits);
        Ok(Self { config, calls: Slab::with_capacity(limits.calls), rng: Rng::new(seed), tally, setup })
    }
    /// A host supplies its bound local origin before it accepts requests.
    pub fn set_base(&mut self, base: Box<[u8]>, limits: &Limits) -> Result<(), Error> {
        check_base(&base, limits)?;
        self.config.base = base;
        Ok(())
    }
    /// Returns the previous endpoint on admission; a refusal leaves the current endpoint unchanged.
    pub fn set_hooks(
        &mut self,
        hooks: Option<crate::delivery::Endpoint>,
    ) -> Result<Option<crate::delivery::Endpoint>, Error> {
        if let Some(endpoint) = &hooks
            && (endpoint.host.is_empty() || endpoint.secret.is_empty())
        {
            return Err(Error::Limits);
        }
        Ok(core::mem::replace(&mut self.config.hooks, hooks))
    }
    #[expect(clippy::result_large_err, reason = "an unmatched owned reply returns unchanged with its reply right")]
    pub fn target(&mut self, request: domain::Request) -> Result<(Token, domain::Request), domain::Request> {
        match request {
            domain::Request::Reply { to, result } => {
                let token = to.into_token();
                let reply = domain::Request::Reply { to: ReplyTo::new(token), result };
                match self.calls.get(Id::<Call>::from_token(token)) {
                    Some(call) if call.active => Ok((call.server, reply)),
                    Some(_) | None => Err(reply),
                }
            }
            domain::Request::Hook { .. } => Err(request),
        }
    }
    #[must_use]
    pub const fn tally(&self) -> &Tally {
        &self.tally
    }
    pub fn reclaim(&mut self) {
        self.calls.reclaim();
    }
    pub(crate) fn user(&self, authorization: Option<&[u8]>) -> Option<u64> {
        let token = authorization?.strip_prefix(b"token ")?;
        for (known, user) in &self.config.tokens {
            if known.expose() == token {
                return Some(*user);
            }
        }
        self.setup.token_user(token)
    }
    pub(crate) fn git_user(&self, authorization: Option<&[u8]>, write: bool) -> Option<(u64, bool)> {
        let value = authorization?.strip_prefix(b"Basic ")?;
        let max = u32::try_from(value.len()).ok()?;
        let decoded = skein_lib::base64::decode(skein_lib::base64::Alphabet::Standard, value, max).ok()?;
        let colon = skein_lib::bytes::find(&decoded, b":")?;
        let login = decoded.get(..colon)?;
        let token = decoded.get(colon.checked_add(1)?..)?;
        if login.is_empty() || token.is_empty() {
            return None;
        }
        for (known, user) in &self.config.tokens {
            if known.expose() == token {
                for (id, name) in &self.config.users {
                    if id == user && name.as_ref() == login {
                        return Some((*user, true));
                    }
                }
                return None;
            }
        }
        let user = self.setup.token_user(token)?;
        if self.setup.user(user)?.login.as_ref() != login {
            return None;
        }
        Some((user, self.setup.accesses_repository(token, write)))
    }
    pub(crate) fn reads_user(&self, authorization: Option<&[u8]>) -> bool {
        self.permits(authorization, b"read:user", b"write:user", false)
    }
    pub(crate) fn accesses_repository(&self, authorization: Option<&[u8]>, write: bool) -> bool {
        self.permits(authorization, b"read:repository", b"write:repository", write)
    }
    pub(crate) fn accesses_admin(&self, authorization: Option<&[u8]>, write: bool) -> bool {
        self.permits(authorization, b"read:admin", b"write:admin", write)
    }
    pub(crate) fn accesses_organization(&self, authorization: Option<&[u8]>, write: bool) -> bool {
        self.permits(authorization, b"read:organization", b"write:organization", write)
    }
    fn permits(&self, authorization: Option<&[u8]>, read: &[u8], write: &[u8], writing: bool) -> bool {
        let Some(value) = authorization else {
            return false;
        };
        let Some(token) = value.strip_prefix(b"token ") else {
            return false;
        };
        // Config tokens carry no scopes; worlds use them with unrestricted access.
        for (known, _) in &self.config.tokens {
            if known.expose() == token {
                return true;
            }
        }
        self.setup.permits(token, read, write, writing)
    }
    /// Scenario state for setup, without changing the repository domain.
    pub fn setup_mut(&mut self) -> &mut crate::setup::Store {
        &mut self.setup
    }
    #[must_use]
    pub fn setup(&self) -> &crate::setup::Store {
        &self.setup
    }
}
fn check_base(base: &[u8], limits: &Limits) -> Result<(), Error> {
    if base.len() > usize::try_from(limits.documents.target_bytes).expect("u32 fits") {
        return Err(Error::Limits);
    }
    let authority = match base.strip_prefix(b"http://") {
        Some(authority) => authority,
        None => base.strip_prefix(b"https://").ok_or(Error::Limits)?,
    };
    if authority.is_empty() {
        return Err(Error::Limits);
    }
    for byte in authority {
        if *byte == b'/' || !(0x21..=0x7e).contains(byte) {
            return Err(Error::Limits);
        }
    }
    Ok(())
}
/// Whether these limits are representable: every edge bound they size is finite
/// and every capacity they derive fits (programming-model.md, 6.3).
/// HTTP and document limits are usable, tokenizer reads fit HTTP, git decoding
/// is finite, and the object walk's four-parent frontier fits u32.
#[must_use]
pub fn valid(limits: &Limits, config: &Config) -> bool {
    limits.calls > 0
        && limits.connections > 0
        && limits.raw_bytes > 0
        && skein_http::server::valid(&limits.http)
        && skein_json::tokenizer::largest_demand(&limits.documents.json) <= limits.http.read
        && skein_forge::forgejo::limits::check(&limits.documents).is_ok()
        && skein_json::tokenizer::worst_case(&limits.documents.json).is_some()
        && crate::git::decoder_worst_case(limits).is_some()
        && config.max_response_items.checked_mul(4).is_some()
}
