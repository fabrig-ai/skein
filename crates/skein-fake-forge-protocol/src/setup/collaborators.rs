use super::{Store, absent, empty, full};
use crate::{Config, Error, Limits, response::Reply};
use alloc::boxed::Box;
use skein_forge::forgejo::request::{Grant, Repo};
#[derive(Debug)]
pub(super) struct Collaborator {
    repository: u32,
    login: Box<[u8]>,
    permission: Grant,
}
/// Resolves only scenario repositories, keeping one source of repository identity.
pub(super) fn repository(config: &Config, at: &Repo) -> Option<u32> {
    for repo in &config.repositories {
        if let Some(tail) = repo.full.strip_prefix(at.owner.as_ref())
            && tail.strip_prefix(b"/") == Some(at.name.as_ref())
        {
            return Some(repo.id);
        }
    }
    None
}
impl Store {
    /// Setup accounts are collaborators only through an explicit collaborator grant.
    /// Scenario-only users retain the domain's configured collaborator table.
    pub(crate) fn lists_collaborator(&self, config: &Config, at: &Repo, user: u64) -> bool {
        let Some(account) = self.user(user) else { return true };
        let Some(repository) = repository(config, at) else { return false };
        self.collaborator(repository, &account.login).is_some()
    }

    /// Inspects a setup grant by scenario repository id without consulting domain permissions.
    #[must_use]
    pub fn collaborator(&self, repository: u32, login: &[u8]) -> Option<Grant> {
        for row in &self.collaborators {
            if row.repository == repository && row.login.as_ref() == login {
                return Some(row.permission);
            }
        }
        None
    }
    pub(super) fn add_collaborator(
        &mut self,
        at: &Repo,
        login: &[u8],
        permission: Grant,
        config: &Config,
        limits: &Limits,
    ) -> Reply {
        let Some(repository) = repository(config, at) else {
            return absent(limits);
        };
        let mut known = self.find(login).is_some();
        for (_, name) in &config.users {
            if name.as_ref() == login {
                known = true;
            }
        }
        if !known {
            return absent(limits);
        }
        match self.grant(repository, login, permission) {
            Ok(()) => empty(),
            Err(Error::Limits) => full(limits),
        }
    }
    fn collaborator_index(&self, repository: u32, login: &[u8]) -> Option<u32> {
        for index in 0..self.collaborators.len() {
            let row = self.collaborators.get(index).expect("bounded collaborator index");
            if row.repository == repository && row.login.as_ref() == login {
                return Some(index);
            }
        }
        None
    }
    /// Removes a scenario collaborator grant without an HTTP exchange.
    pub fn revoke_collaborator(&mut self, repository: u32, login: &[u8]) {
        if let Some(index) = self.collaborator_index(repository, login) {
            let last = self.collaborators.len().checked_sub(1).expect("found collaborator");
            if index != last {
                let row = core::mem::replace(
                    self.collaborators.get_mut(last).expect("last collaborator"),
                    Collaborator { repository: 0, login: Box::new([]), permission: Grant::Read },
                );
                *self.collaborators.get_mut(index).expect("found collaborator") = row;
            }
            self.collaborators.truncate(last);
        }
    }
    /// Sets or replaces a bounded scenario grant without an HTTP exchange.
    pub fn grant(&mut self, repository: u32, login: &[u8], permission: Grant) -> Result<(), Error> {
        if login.len() > self.string_bytes {
            return Err(Error::Limits);
        }
        if let Some(index) = self.collaborator_index(repository, login) {
            self.collaborators.get_mut(index).expect("found collaborator").permission = permission;
            return Ok(());
        }
        if self.collaborators.room() == 0 {
            return Err(Error::Limits);
        }
        if self.collaborators.push(Collaborator { repository, login: login.into(), permission }).is_err() {
            return Err(Error::Limits);
        }
        Ok(())
    }
}

/// Setup `Read`, `Write` and `Admin` grant domain `Read`, `Write` and `Admin`, respectively.
const fn domain_permission(grant: Grant) -> skein_fake_forge_domain::api::Permission {
    use skein_fake_forge_domain::api::Permission;
    match grant {
        Grant::Read => Permission::Read,
        Grant::Write => Permission::Write,
        Grant::Admin => Permission::Admin,
    }
}

impl Store {
    pub(crate) fn answer_with_domain(
        &mut self,
        request: &skein_forge::forgejo::request::Request,
        user: u64,
        config: &mut Config,
        env: &skein_lib::Env<Limits>,
        domain: Option<&mut crate::Hosted<'_>>,
    ) -> Option<Reply> {
        use skein_forge::forgejo::request::Request;
        let limits = &env.limits;
        let Some(hosted) = domain else { return self.answer(request, user, config, limits) };
        if let Some(reply) = self.creation(request, user, config, env, hosted) {
            return Some(reply);
        }
        if let Request::RepositoryById { id } = request {
            let caller = self.user(user)?;
            return Some(self.repository_by_id(*id, &caller, config, limits, Some(hosted)));
        }
        let Request::AddCollaborator { at, login, permission } = request else {
            return self.answer(request, user, config, limits);
        };
        let admin = match self.user(user) {
            Some(caller) => caller.is_admin,
            None => false,
        };
        if !admin {
            return self.answer(request, user, config, limits);
        }
        let Some(id) = repository(config, at) else { return self.answer(request, user, config, limits) };
        let mut repository = None;
        for known in &config.repositories {
            if known.id == id {
                repository = Some(known);
            }
        }
        let repository = repository.expect("resolved scenario repository");
        let Some(account) = self.collaborator_user(config, login) else {
            return self.answer(request, user, config, limits);
        };
        let room = hosted.domain.can_grant(&repository.full, account);
        if room == Ok(false) {
            return Some(full(limits));
        }
        let reply = self.answer(request, user, config, limits)?;
        if reply.status == 204 && room == Ok(true) {
            skein_fake_forge_domain::grant(hosted.domain, &repository.full, account, domain_permission(*permission));
        }
        Some(reply)
    }
    fn collaborator_user(&self, config: &Config, login: &[u8]) -> Option<u64> {
        if let Some(known) = self.find(login) {
            return Some(known.id);
        }
        let mut account = None;
        for (id, name) in &config.users {
            if name.as_ref() == login {
                account = Some(*id);
            }
        }
        account
    }
}
