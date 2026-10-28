//! The two HTTP faces use the same connection lifecycle and shared-forge host.
pub(super) enum Event {
    Domain(domain::Event),
    Rate { key: domain::RateKey, limit: u32, window: skein_lib::Duration },
    Close,
    Closed,
}
use skein_fake_forge_domain as domain;
use skein_fake_forge_github as github;
use skein_fake_forge_protocol as forgejo;
use skein_lib::{
    Env, Queue, Token,
    stream::{Down, Up},
};

#[derive(Clone, Copy)]
pub(super) enum Limits {
    Forgejo(forgejo::Limits),
    Github(github::Limits),
}
impl Limits {
    pub fn http(self) -> skein_http::server::Limits {
        match self {
            Self::Forgejo(limits) => limits.http,
            Self::Github(limits) => limits.http,
        }
    }
    pub fn calls(self, hosted: u32) -> u32 {
        match self {
            Self::Forgejo(limits) => limits.calls,
            Self::Github(_) => hosted,
        }
    }
    pub fn connections(self, hosted: u32) -> u32 {
        match self {
            Self::Forgejo(limits) => limits.connections,
            Self::Github(_) => hosted,
        }
    }
}
#[expect(
    clippy::large_enum_variant,
    reason = "the peer owns one inline service; each dialect budgets its stores' heap"
)]
pub(super) enum Service {
    Forgejo(forgejo::Service),
    Github(github::Service),
}
impl Service {
    pub fn is_github(&self) -> bool {
        matches!(self, Self::Github(_))
    }
    pub fn set_hooks(
        &mut self,
        endpoint: Option<forgejo::delivery::Endpoint>,
    ) -> Result<Option<forgejo::delivery::Endpoint>, forgejo::Error> {
        match self {
            Self::Forgejo(service) => service.set_hooks(endpoint),
            Self::Github(_) => Ok(endpoint),
        }
    }
    pub fn reclaim(&mut self) {
        match self {
            Self::Forgejo(service) => service.reclaim(),
            Self::Github(_) => {}
        }
    }
    pub fn target(&mut self, reply: domain::Request) -> Option<(Token, domain::Request)> {
        match self {
            Self::Forgejo(service) => service.target(reply).ok(),
            Self::Github(_) => match reply {
                domain::Request::Reply { to, result } => {
                    let owner = to.into_token();
                    Some((owner, domain::Request::Reply { to: skein_lib::ReplyTo::new(owner), result }))
                }
                domain::Request::Hook { .. } => None,
            },
        }
    }
    pub fn deliveries(
        &mut self,
        hook: &domain::Request,
        limits: &Limits,
    ) -> skein_lib::List<forgejo::delivery::Delivery> {
        match (self, limits) {
            (Self::Forgejo(service), Limits::Forgejo(limits)) => service.deliveries(hook, limits),
            (Self::Github(_), Limits::Github(_)) => skein_lib::List::with_capacity(0),
            (Self::Forgejo(_), Limits::Github(_)) | (Self::Github(_), Limits::Forgejo(_)) => {
                unreachable!("paired layer")
            }
        }
    }
    pub fn delivery(&mut self, hook: &domain::Request, limits: &Limits) -> Option<forgejo::delivery::Post> {
        match (self, limits) {
            (Self::Forgejo(service), Limits::Forgejo(limits)) => service.delivery(hook, limits),
            (Self::Github(_), Limits::Github(_)) => None,
            (Self::Forgejo(_), Limits::Github(_)) | (Self::Github(_), Limits::Forgejo(_)) => {
                unreachable!("paired layer")
            }
        }
    }
}
#[expect(clippy::large_enum_variant, reason = "shared::bound counts the full inline server slot for each connection")]
pub(super) enum Server {
    Forgejo(forgejo::Server),
    Github(github::Server),
}
impl Server {
    pub fn new(owner: Token, limits: &Limits) -> Result<Self, crate::Error> {
        match limits {
            Limits::Forgejo(limits) => {
                forgejo::Server::new(owner, limits).map(Self::Forgejo).map_err(|_| crate::Error::Limits)
            }
            Limits::Github(limits) => {
                github::Server::new(owner, limits).map(Self::Github).map_err(|_| crate::Error::Limits)
            }
        }
    }
    pub fn blob(owner: Token, limits: &Limits) -> Result<Self, crate::Error> {
        match limits {
            Limits::Github(limits) => {
                github::Server::blob(owner, limits).map(Self::Github).map_err(|_| crate::Error::Limits)
            }
            Limits::Forgejo(_) => Err(crate::Error::Limits),
        }
    }
    pub fn take_head(&mut self) -> Option<skein_http::server::Call> {
        match self {
            Self::Forgejo(server) => server.take_head(),
            Self::Github(server) => server.take_head(),
        }
    }
    pub fn is_settled(&self) -> bool {
        match self {
            Self::Forgejo(server) => server.is_settled(),
            Self::Github(server) => server.is_settled(),
        }
    }
    pub fn has_work(&self) -> bool {
        match self {
            Self::Forgejo(server) => server.has_work(),
            Self::Github(server) => server.has_work(),
        }
    }
}
// Each adapter transfers the records emitted by one layer entrance.
macro_rules! entry {
    ($name:ident => $forgejo:ident, $github:ident $(; $domain:ident : $domain_ty:ty)? $(, $arg:ident : $ty:ty)*) => {
        pub(super) fn $name(server: &mut Server, service: &mut Service, $($domain: $domain_ty,)? env: &Env<Limits>, $($arg: $ty,)* above: &mut Queue<Event>, below: &mut Queue<Down>) {
            match (server, service, &env.limits) {
                (Server::Forgejo(server), Service::Forgejo(service), Limits::Forgejo(limits)) => {
                    let mut events=Queue::uncapped();
                    forgejo::$forgejo(server, service, $($domain,)? &Env { now: env.now, wall: env.wall, limits: *limits }, $($arg,)* &mut events, below);
                    translate_forgejo(&mut events,above);
                },
                (Server::Github(server), Service::Github(service), Limits::Github(limits)) => {
                    let mut events = Queue::uncapped();
                    github::$github(server, service, $($domain,)? &Env { now: env.now, wall: env.wall, limits: *limits }, $($arg,)* &mut events, below);
                    translate(&mut events, above);
                }
                (Server::Forgejo(_), Service::Forgejo(_) | Service::Github(_), Limits::Github(_))
                | (Server::Forgejo(_), Service::Github(_), Limits::Forgejo(_))
                | (Server::Github(_), Service::Forgejo(_) | Service::Github(_), Limits::Forgejo(_))
                | (Server::Github(_), Service::Forgejo(_), Limits::Github(_)) => unreachable!("paired layer"),
            }
        }
    }
}
entry!(start => start, start);
entry!(up_with_domain => up_with_domain, up_with_domain; domain: forgejo::Hosted<'_>, event: Up);
entry!(resume_with_domain => resume_with_domain, resume_with_domain; domain: forgejo::Hosted<'_>);
entry!(close => close, close);
entry!(closed => closed, closed);
pub(super) fn down(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    reply: domain::Request,
    domain: &domain::Domain,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    match (server, service, &env.limits) {
        (Server::Forgejo(server), Service::Forgejo(service), Limits::Forgejo(limits)) => {
            let mut events = Queue::uncapped();
            forgejo::down(
                server,
                service,
                &Env { now: env.now, wall: env.wall, limits: *limits },
                reply,
                &mut events,
                below,
            );
            translate_forgejo(&mut events, above);
        }
        (Server::Github(server), Service::Github(service), Limits::Github(limits)) => {
            let mut events = Queue::uncapped();
            github::down(
                server,
                service,
                &Env { now: env.now, wall: env.wall, limits: *limits },
                reply,
                domain,
                &mut events,
                below,
            );
            translate(&mut events, above);
        }
        (Server::Forgejo(_), Service::Forgejo(_) | Service::Github(_), Limits::Github(_))
        | (Server::Forgejo(_), Service::Github(_), Limits::Forgejo(_))
        | (Server::Github(_), Service::Forgejo(_) | Service::Github(_), Limits::Forgejo(_))
        | (Server::Github(_), Service::Forgejo(_), Limits::Github(_)) => unreachable!("paired layer"),
    }
}
fn translate(events: &mut Queue<github::Event>, above: &mut Queue<Event>) {
    while let Some(event) = events.pop() {
        above.push(match event {
            github::Event::Domain(event) => Event::Domain(event),
            github::Event::Rate { key, limit, window } => Event::Rate { key, limit, window },
            github::Event::Close => Event::Close,
            github::Event::Closed => Event::Closed,
        });
    }
}

fn translate_forgejo(events: &mut Queue<forgejo::Event>, above: &mut Queue<Event>) {
    while let Some(event) = events.pop() {
        above.push(match event {
            forgejo::Event::Domain(event) => Event::Domain(event),
            forgejo::Event::Close => Event::Close,
            forgejo::Event::Closed => Event::Closed,
        });
    }
}
pub(super) fn admitted(
    server: &mut Server,
    service: &mut Service,
    env: &Env<Limits>,
    result: Result<domain::Rate, domain::RateDenied>,
    above: &mut Queue<Event>,
    below: &mut Queue<Down>,
) {
    let (Server::Github(server), Service::Github(service), Limits::Github(limits)) = (server, service, env.limits)
    else {
        unreachable!("GitHub admission")
    };
    let mut events = Queue::uncapped();
    github::server::admitted(
        server,
        service,
        &Env { now: env.now, wall: env.wall, limits },
        result,
        &mut events,
        below,
    );
    translate(&mut events, above);
}

impl super::Peer {
    /// The hosted GitHub layer, including its installation store and git records.
    #[must_use]
    pub fn github_layer(&self) -> Option<&github::Service> {
        match &self.service {
            Service::Github(service) => Some(service),
            Service::Forgejo(_) => None,
        }
    }
    /// Scenario access to installations and token minting on the hosted GitHub layer.
    pub fn github_layer_mut(&mut self) -> Option<&mut github::Service> {
        match &mut self.service {
            Service::Github(service) => Some(service),
            Service::Forgejo(_) => None,
        }
    }
    /// Recent private git calls, without their credentials.
    #[must_use]
    pub fn git_calls(&self) -> Option<&Queue<github::GitCall>> {
        Some(self.github_layer()?.git_calls())
    }
}
