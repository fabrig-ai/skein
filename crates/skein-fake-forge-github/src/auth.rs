//! Credentials remain in the layer; only the acting user crosses into the domain.
use crate::{
    installations::{Grant, Scope},
    service::Service,
};

use crate::{
    answers,
    installations::Reach,
    reads,
    service::{ContentType, Outcome, Pending, PendingKind, Request, Response},
};
use alloc::boxed::Box;
use skein_fake_forge_domain::{
    self as fake,
    api::{Answer, Error, Op, Permission, Read},
};
use skein_lib::{List, ReplyTo, Wall};

/// A resolved credential, valid while the owner completes this request.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Bearer {
    Installation { slot: u32, installation: u64 },
    Person { index: u32, user: u64 },
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum Access {
    Person(u64),
    Installation { user: u64, grant: Grant },
}
impl Access {
    pub(crate) fn user(self) -> u64 {
        match self {
            Self::Person(user) | Self::Installation { user, .. } => user,
        }
    }
    pub(crate) fn contents(self) -> bool {
        match self {
            Self::Person(_) => false,
            Self::Installation { grant, .. } => grant.scope == Scope::Contents,
        }
    }
    pub(crate) fn reaches(self, service: &Service, repository: u32) -> bool {
        match self {
            Self::Person(_) => true,
            Self::Installation { grant, .. } => service.reaches(&grant, repository),
        }
    }
}

impl Service {
    pub(crate) fn reaches(&self, grant: &Grant, repository: u32) -> bool {
        let Some(row) = self.repositories.get(&repository) else { return false };
        let Some(installation) = self.installations.get(grant.installation) else { return false };
        row.owner_id == installation.account && self.installations.reaches(grant, repository)
    }
    pub(crate) fn resolve(&self, bearer: &[u8], now: Wall) -> Option<Bearer> {
        for (index, (secret, user, _)) in self.tokens.iter().enumerate() {
            if secret.expose() == bearer {
                return Some(Bearer::Person { index: u32::try_from(index).expect("bounded token table"), user: *user });
            }
        }
        let (slot, grant) = self.installations.token_slot(bearer, now).ok()?;
        Some(Bearer::Installation { slot, installation: grant.installation })
    }
    #[expect(clippy::result_large_err, reason = "bounded inline responses avoid an allocation on refusals")]
    pub(crate) fn access(&self, bearer: Bearer) -> Result<Access, Response> {
        match bearer {
            Bearer::Person { index, user } => {
                let Some((_, stored, _)) = self.tokens.get(index) else {
                    return Err(answers::unauthorized(&self.limits));
                };
                if *stored != user {
                    return Err(answers::unauthorized(&self.limits));
                }
                Ok(Access::Person(user))
            }
            Bearer::Installation { slot, installation } => {
                let Some(grant) = self.installations.token_at(slot) else {
                    return Err(answers::unauthorized(&self.limits));
                };
                if grant.installation != installation {
                    return Err(answers::unauthorized(&self.limits));
                }
                let Some(installed) = self.installations.get(installation) else {
                    return Err(answers::unauthorized(&self.limits));
                };
                if installed.suspended {
                    return Err(answers::refusal(403, answers::SUSPENDED, &self.limits));
                }
                Ok(Access::Installation { user: self.bot, grant: *grant })
            }
        }
    }
}
pub(crate) fn empty() -> Response {
    Response {
        status: 204,
        content_type: ContentType::Json,
        link: None,
        location: None,
        etag: None,
        rate: None,
        body: Box::new([]),
    }
}
pub(crate) fn reachable(service: &Service, page: u32, access: Access, request: &Request<'_>) -> Outcome {
    Outcome::Reply(match access {
        Access::Person(_) => answers::refusal(403, crate::texts::INTEGRATION, &service.limits),
        Access::Installation { grant, .. } => {
            if service.body_fits(request.body) {
                service.reachable(page, &grant, request.target)
            } else {
                answers::refusal(413, b"Payload Too Large", &service.limits)
            }
        }
    })
}
pub(crate) fn edit_reach(
    service: &Service,
    access: Access,
    installation: u64,
    repository: u32,
    add: bool,
    reply_to: ReplyTo,
) -> Outcome {
    let Access::Person(user) = access else {
        return Outcome::Reply(answers::refusal(403, crate::texts::INTEGRATION, &service.limits));
    };
    let Some(row) = service.repositories.get(&repository) else {
        return Outcome::Reply(answers::missing(&service.limits));
    };
    let Some(installed) = service.installations.get(installation) else {
        return Outcome::Reply(answers::missing(&service.limits));
    };
    if installed.account != row.owner_id {
        return Outcome::Reply(answers::missing(&service.limits));
    }
    let Some(path) = reads::full(row) else {
        return Outcome::Reply(answers::failed());
    };
    Outcome::Call(
        Pending { rate: None, kind: PendingKind::EditReach { installation, repository, add } },
        fake::Event::Call { reply_to, user, repository: path, op: Op::Read(Read::Permission { user }) },
    )
}
pub(crate) fn finish_reach(
    service: &mut Service,
    installation: u64,
    repository: u32,
    add: bool,
    result: Result<Answer, Error>,
) -> Response {
    let Ok(Answer::Permission(Permission::Admin)) = result else {
        return answers::missing(&service.limits);
    };
    let Some(installed) = service.installations.get(installation) else {
        return answers::missing(&service.limits);
    };
    let mut repositories = List::with_capacity(service.limits.installations.listed);
    match &installed.reach {
        Reach::All => {
            if add {
                return empty();
            }
            return answers::refusal(422, crate::texts::REMOVE_ALL_REPOSITORIES, &service.limits);
        }
        Reach::Listed(ids) => {
            if !add && ids.len() == 1 && ids.as_slice().contains(&repository) {
                return answers::refusal(422, crate::texts::REMOVE_LAST_REPOSITORY, &service.limits);
            }
            for id in ids {
                if *id != repository && repositories.push(*id).is_err() {
                    return answers::refusal(422, b"Validation Failed", &service.limits);
                }
            }
            if add && repositories.push(repository).is_err() {
                return answers::refusal(422, b"Validation Failed", &service.limits);
            }
        }
    }
    if !service.installations.set_reach(installation, Reach::Listed(repositories)) {
        return answers::missing(&service.limits);
    }
    empty()
}

pub(crate) fn check(config: &crate::Config, limits: &crate::Limits) -> Result<(), crate::Problem> {
    if config.tokens.len() > limits.github.inner {
        return Err(crate::Problem::Config);
    }
    for (at, (secret, user, _)) in config.tokens.iter().enumerate() {
        let mut person = false;
        for account in &config.accounts {
            if account.id == *user && !account.organization {
                person = true;
            }
        }
        if secret.expose().is_empty()
            || secret.expose().len() > usize::try_from(limits.github.string_bytes).expect("u32 fits")
            || !person
        {
            return Err(crate::Problem::Config);
        }
        for (prior, _, _) in config.tokens.iter().take(at) {
            if prior.expose() == secret.expose() {
                return Err(crate::Problem::Config);
            }
        }
    }
    Ok(())
}
