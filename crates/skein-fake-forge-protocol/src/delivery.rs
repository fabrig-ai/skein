//! Signed webhook requests for a world to write (fake-forge.md, section 4).
use crate::{Limits, Service, routes};
use alloc::boxed::Box;
use core::{net::SocketAddr, str::FromStr};
use skein_fake_forge_domain::{
    self as domain,
    api::{Change, Check},
};
use skein_forge::forgejo::{delivery, document::Body, request::ReviewEvent, server};
use skein_lib::{Decimal, List, Secret, Writer, sha256::Hmac};
/// One forge's destination and signing secret; no secret crosses into its domain.
#[derive(Debug)]
pub struct Endpoint {
    pub forge: u16,
    pub host: Box<[u8]>,
    pub secret: Secret,
}
/// A complete request, written head first and then body by the world.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Post {
    pub head: Box<[u8]>,
    pub body: Box<[u8]>,
}
/// One independently addressed, signed request to post over loopback.
#[derive(Debug)]
pub struct Delivery {
    pub address: SocketAddr,
    pub post: Post,
}

impl Service {
    /// Renders a configured endpoint's hook; replies send nothing.
    #[must_use]
    pub fn delivery(&self, hook: &domain::Request, limits: &Limits) -> Option<Post> {
        let endpoint = self.config.hooks.as_ref()?;
        let (_, sent) = self.payload(hook, limits)?;
        post(sent, &delivery::hook_target(endpoint.forge), &endpoint.host, &endpoint.secret)
    }

    /// Renders each eligible repository hook in id order, or the configured endpoint alone.
    /// The bounded list holds at most `documents.inner` setup hooks, or one endpoint.
    #[must_use]
    pub fn deliveries(&self, hook: &domain::Request, limits: &Limits) -> List<Delivery> {
        let mut posts = List::with_capacity(if self.config.hooks.is_some() { 1 } else { limits.documents.inner });
        if let Some(endpoint) = &self.config.hooks {
            if let Some(address) = address(&endpoint.host)
                && let Some(post) = self.delivery(hook, limits)
            {
                posts.push(Delivery { address, post }).expect("one endpoint");
            }
            return posts;
        }
        let Some((repository, sent)) = self.payload(hook, limits) else { return posts };
        let Some(events) = delivery::subscriptions(sent.event) else { return posts };
        for hook in self.setup.repository_hooks(repository) {
            let mut takes = false;
            for event in &hook.events {
                if events.contains(&event.as_ref()) {
                    takes = true;
                }
            }
            if !hook.active || hook.secret.is_empty() || !takes {
                continue;
            }
            let Some((address, host, target)) = destination(&hook.url) else { continue };
            if let Some(post) = post(sent.clone(), target, host, &hook.secret)
                && posts.push(Delivery { address, post }).is_err()
            {
                break;
            }
        }
        posts
    }

    fn payload(&self, hook: &domain::Request, limits: &Limits) -> Option<(u32, server::delivery::Sent)> {
        let (repository, change, number, branch, commit, by, body, ended) = match hook {
            domain::Request::Hook { repository, change, number, branch, commit, by, body, ended, .. } => {
                (repository, change, number, branch, commit, by, body, ended)
            }
            domain::Request::Reply { .. } => return None,
        };
        let mut id = None;
        for known in &self.config.repositories {
            if known.full == *repository {
                id = Some(known.id);
            }
        }
        let id = id?;
        let repository = u64::from(id);
        let value = match change {
            Change::Group { .. } => return None,
            Change::Issue => {
                delivery::Delivery::Issue { repository, number: (*number)?, body: Body::Kept(body.as_ref()?.clone()) }
            }
            Change::Pull => delivery::Delivery::Pull { repository, number: (*number)? },
            Change::Comment => {
                delivery::Delivery::Comment { repository, number: (*number)?, body: Body::Kept(body.as_ref()?.clone()) }
            }
            Change::Review => delivery::Delivery::Review {
                event: ReviewEvent::Approved,
                repository,
                number: (*number)?,
                body: Body::Kept(body.as_ref()?.clone()),
            },
            Change::Push => {
                let branch = branch.as_ref()?;
                let mut reference = Writer::new(11usize.checked_add(branch.len())?);
                reference.put(b"refs/heads/").expect("prefix fits");
                reference.put(branch).expect("branch fits");
                match commit {
                    Some(commit) => delivery::Delivery::Push {
                        repository,
                        reference: reference.finish(),
                        after: routes::object(*commit),
                        pusher: (*by)?,
                    },
                    None => delivery::Delivery::Ref {
                        created: false,
                        repository,
                        reference: reference.finish(),
                        ref_type: delivery::RefType::Branch,
                    },
                }
            }
            Change::Job => {
                let ended = ended.as_ref()?;
                let end = match ended.state {
                    Check::Passed => delivery::JobEnd::Success,
                    Check::Failed => delivery::JobEnd::Failure,
                    Check::Pending | Check::Skipped => return None,
                };
                delivery::Delivery::Job { end, repository, commit: routes::object(ended.commit) }
            }
        };
        Some((id, server::delivery::encode(&value, &limits.documents).ok()?))
    }
}

fn post(sent: server::delivery::Sent, target: &[u8], host: &[u8], secret: &Secret) -> Option<Post> {
    let length = Decimal::of(u64::try_from(sent.body.len()).ok()?);
    let signature = sign(secret, &sent.body);
    let parts: [&[u8]; 16] = [
        b"POST ",
        target,
        b" HTTP/1.1\r\nHost: ",
        host,
        b"\r\nContent-Type: application/json\r\nContent-Length: ",
        length.as_bytes(),
        b"\r\n",
        delivery::EVENT_HEADER,
        b": ",
        sent.event,
        b"\r\n",
        delivery::SIGNATURE_HEADER,
        b": ",
        &signature,
        b"\r\n",
        b"\r\n",
    ];
    let mut size = 0usize;
    for part in parts {
        size = size.checked_add(part.len())?;
    }
    let mut head = Writer::new(size);
    for part in parts {
        head.put(part).expect("measured head");
    }
    Some(Post { head: head.finish(), body: sent.body })
}
/// Forgejo's lowercase hexadecimal HMAC-SHA256 signature.
#[must_use]
pub fn sign(secret: &Secret, body: &[u8]) -> [u8; 64] {
    let mut hmac = Hmac::new(secret.expose());
    hmac.update(body);
    let mut signature = [0; 64];
    for (out, byte) in signature.as_chunks_mut::<2>().0.iter_mut().zip(hmac.finish()) {
        for (digit, nibble) in out.iter_mut().zip([byte / 16, byte % 16]) {
            *digit =
                if nibble < 10 { b'0'.saturating_add(nibble) } else { b'a'.saturating_add(nibble.saturating_sub(10)) };
        }
    }
    signature
}

#[expect(
    clippy::disallowed_methods,
    reason = "the protocol boundary validates UTF-8 before core::net parses an IP literal and port"
)]
fn address(host: &[u8]) -> Option<SocketAddr> {
    let address = SocketAddr::from_str(core::str::from_utf8(host).ok()?).ok()?;
    address.ip().is_loopback().then_some(address)
}

fn destination(url: &[u8]) -> Option<(SocketAddr, &[u8], &[u8])> {
    let rest = url.strip_prefix(b"http://")?;
    for byte in rest {
        if !(0x21..=0x7e).contains(byte) {
            return None;
        }
    }
    let slash = skein_lib::bytes::find(rest, b"/")?;
    let (host, path) = rest.split_at_checked(slash)?;
    let target = match skein_lib::bytes::find(path, b"#") {
        Some(fragment) => path.get(..fragment)?,
        None => path,
    };
    Some((address(host)?, host, target))
}

pub(crate) fn worst_case(limits: &Limits, config: &crate::Config) -> Option<u64> {
    let count = limits.documents.inner.max(1);
    let host = match &config.hooks {
        Some(endpoint) => u64::try_from(endpoint.host.len()).ok()?,
        None => 0,
    }
    .max(u64::from(limits.documents.string_bytes));
    let post = host
        .checked_add(u64::from(limits.documents.string_bytes))?
        .checked_add(512)?
        .checked_add(u64::from(limits.documents.writer.length))?;
    List::<Delivery>::worst_case(count)?
        .checked_add(u64::from(count).checked_mul(post)?)?
        .checked_add(u64::from(limits.documents.writer.length))
}
