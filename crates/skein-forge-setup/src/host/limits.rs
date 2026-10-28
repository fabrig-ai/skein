//! Admission and stack bounds for the host (forge-setup.md, 6.3).
use super::settings::{self, Kind, Settings, Transport};
use skein_forge::{forgejo::limits as docs, github as dialect};
use skein_json::{tokenizer, writer};
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    /// The most configured forges, with unique numbers.
    pub forges: u32,
    /// Run slots, including ended runs whose connections are settling (6.5).
    pub runs: u32,
    /// One-time states retained across all forges, nonzero (forge-setup.md, 5.7).
    pub states: u32,
    /// The longest token or secret item (6.4), at least 64 bytes.
    pub token_bytes: u32,
    /// The most bytes of response heads, their framing included.
    pub head_bytes: u32,
    /// The most fields retained from a response head.
    pub headers: u32,
    /// The most incoming JSON body bytes, before decoding.
    pub body_bytes_in: u32,
    /// The most nested JSON containers.
    pub depth: u32,
    /// The body stream's largest piece.
    pub chunk: u32,
    /// The most bytes retained in a document's scalar string; at least 27 for GitHub.
    pub string_bytes: u32,
    /// The most bytes retained in a document's body string.
    pub body_bytes: u32,
    /// The most elements in a document's inner list or a Forgejo listing's page;
    /// at least 14 for GitHub.
    pub inner: u32,
    /// The longest encoded request target.
    pub target_bytes: u32,
    /// The owner's io stream limits, checked against HTTP's demands.
    pub io: skein_io::Limits,
    /// The TLS client beneath HTTP.
    pub tls: skein_tls::client::Limits,
}
/// The documents' limits, with body strings bounded independently.
#[must_use]
pub fn documents(l: &Limits) -> docs::Limits {
    docs::Limits {
        json: tokenizer::Limits {
            depth: l.depth,
            string: l.body_bytes.max(l.string_bytes),
            number: 32,
            chunk: l.chunk,
            length: l.body_bytes_in,
        },
        writer: writer::Limits { depth: l.depth, length: l.body_bytes_in },
        string_bytes: l.string_bytes,
        body_bytes: l.body_bytes,
        inner: l.inner,
        target_bytes: l.target_bytes,
    }
}
/// GitHub documents' limits from the host's, as `documents` does Forgejo's.
#[must_use]
pub fn github(l: &Limits) -> dialect::limits::Limits {
    dialect::limits::Limits {
        json: tokenizer::Limits {
            depth: l.depth,
            string: l.body_bytes.max(l.string_bytes),
            number: 32,
            chunk: l.chunk,
            length: l.body_bytes_in,
        },
        writer: writer::Limits { depth: l.depth, length: l.body_bytes_in },
        string_bytes: l.string_bytes,
        body_bytes: l.body_bytes,
        inner: l.inner,
        target_bytes: l.target_bytes,
    }
}
/// HTTP's limits on plaintext, with complete heads sent within io's cap.
#[must_use]
pub fn http(l: &Limits) -> skein_http::client::Limits {
    http_transport(l, false)
}
pub(super) fn http_transport(l: &Limits, tls: bool) -> skein_http::client::Limits {
    skein_http::client::Limits {
        request: if tls { l.tls.send } else { l.io.output },
        head: l.head_bytes,
        headers: l.headers,
        read: l.chunk.max(5),
        send: if tls { l.tls.send } else { l.io.output },
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stacked {
    HttpOverIo,
    HttpOverTls,
    TlsOverIo,
    JsonOverHttp,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unusable {
    Zero,
    TokenBytes {
        token_bytes: u32,
    },
    Documents(docs::Problem),
    GitHubDocuments(dialect::limits::Problem),
    /// The required inner count and scalar bytes for the wanted App metadata.
    GitHubMetadata {
        inner: u32,
        string_bytes: u32,
    },
    Stack {
        stacked: Stacked,
        demand: u32,
        cap: u32,
    },
    Forges {
        forges: u32,
    },
    Duplicate {
        forge: u16,
    },
    Address {
        forge: u16,
    },
    PlaintextName {
        forge: u16,
    },
    Request {
        forge: u16,
    },
    Given {
        forge: u16,
        refused: crate::given::Refused,
    },
    GitHubGiven {
        forge: u16,
        refused: crate::github::given::Refused,
    },
    /// A forge's picker page could hold more rows than a listing keeps.
    Reachable {
        forge: u16,
        reachable: u32,
        inner: u32,
    },
}
pub(super) fn check(l: &Limits, settings: &Settings) -> Result<(), Unusable> {
    for value in [
        l.forges,
        l.runs,
        l.states,
        l.token_bytes,
        l.head_bytes,
        l.headers,
        l.body_bytes_in,
        l.depth,
        l.chunk,
        l.string_bytes,
        l.body_bytes,
        l.inner,
        l.target_bytes,
    ] {
        if value == 0 {
            return Err(Unusable::Zero);
        }
    }
    if !l.io.is_usable() || !skein_tls::client::valid(&l.tls) {
        return Err(Unusable::Zero);
    }
    if l.token_bytes < 64 {
        return Err(Unusable::TokenBytes { token_bytes: l.token_bytes });
    }
    if let Err(problem) = docs::check(&documents(l)) {
        return Err(Unusable::Documents(problem));
    }
    stacks(l)?;
    if settings.forges.len() > l.forges {
        return Err(Unusable::Forges { forges: l.forges });
    }
    for (at, forge) in settings.forges.as_slice().iter().enumerate() {
        let number = forge.number();
        for before in settings.forges.as_slice().get(..at).expect("prior forge positions") {
            if before.number() == number {
                return Err(Unusable::Duplicate { forge: number });
            }
        }
        let tls = match &forge.transport {
            Transport::Plaintext { name } => {
                if let Some(name) = name
                    && !plaintext_name(name)
                {
                    return Err(Unusable::PlaintextName { forge: number });
                }
                false
            }
            Transport::Tls { .. } => true,
        };
        match forge.address {
            skein_io::Addr::Ip(ip) => {
                if !tls && !ip.ip().is_loopback() {
                    return Err(Unusable::Address { forge: number });
                }
            }
            skein_io::Addr::InProcess(_) => return Err(Unusable::Address { forge: number }),
        }
        let h = http_transport(l, tls);
        let host = settings::host(forge).ok_or(Unusable::Request { forge: number })?;
        let (credential, framing) = match &forge.kind {
            Kind::Forgejo(_) => (u64::from(l.token_bytes), 192),
            Kind::GitHub(_) => {
                if let Err(problem) = dialect::limits::check(&github(l)) {
                    return Err(Unusable::GitHubDocuments(problem));
                }
                app_metadata(l)?;
                stack(Stacked::JsonOverHttp, tokenizer::largest_demand(&github(l).json), h.read)?;
                (u64::from(l.token_bytes).max(u64::try_from(dialect::jwt::JWT_BYTES).expect("JWT bound fits")), 320)
            }
        };
        let head = u64::from(l.target_bytes)
            .saturating_add(u64::try_from(host.len()).expect("host length fits"))
            .saturating_add(credential)
            .saturating_add(u64::try_from(settings.user_agent.len()).expect("agent length fits"))
            .saturating_add(framing);
        if !header_value(&settings.user_agent) || head > u64::from(h.request) {
            return Err(Unusable::Request { forge: number });
        }
        let reachable = match &forge.kind {
            Kind::Forgejo(given) => {
                if let Err(refused) = crate::given::check(given) {
                    return Err(Unusable::Given { forge: number, refused });
                }
                given.limits.reachable
            }
            Kind::GitHub(given) => {
                if let Err(refused) = crate::github::given::check(given) {
                    return Err(Unusable::GitHubGiven { forge: number, refused });
                }
                given.limits.reachable
            }
        };
        if reachable > l.inner {
            return Err(Unusable::Reachable { forge: number, reachable, inner: l.inner });
        }
        if !skein_http::client::valid(&h) || l.body_bytes_in == u32::MAX {
            return Err(Unusable::Zero);
        }
        super::memory::largest_page(l, &forge.kind).ok_or(Unusable::Zero)?;
        match &forge.kind {
            Kind::Forgejo(_) => {}
            Kind::GitHub(given) => {
                // Discovery reserves one pinned page per configured page.
                given.limits.pages.checked_mul(dialect::pin::PAGE_CAP).ok_or(Unusable::Zero)?;
                // PEM's base64 spelling is an edge with a u32 length.
                skein_lib::base64::encoded_len(skein_lib::base64::Alphabet::Standard, given.limits.key_bytes)
                    .ok_or(Unusable::Zero)?;
            }
        }
    }
    Ok(())
}
fn stacks(l: &Limits) -> Result<(), Unusable> {
    for tls in [true, false] {
        let h = http_transport(l, tls);
        let (stacked, read, send) = if tls {
            (Stacked::HttpOverTls, l.tls.read, l.tls.send)
        } else {
            (Stacked::HttpOverIo, l.io.intake, l.io.output)
        };
        stack(stacked, skein_http::client::largest_read(&h), read)?;
        stack(stacked, skein_http::client::largest_room(&h), send)?;
    }
    stack(Stacked::TlsOverIo, skein_tls::client::LARGEST_READ, l.io.intake)?;
    stack(Stacked::TlsOverIo, skein_tls::client::largest_room(&l.tls), l.io.output)?;
    let h = http(l);
    stack(Stacked::JsonOverHttp, tokenizer::largest_demand(&documents(l).json), h.read)?;
    Ok(())
}
fn stack(stacked: Stacked, demand: u32, cap: u32) -> Result<(), Unusable> {
    if demand > cap { Err(Unusable::Stack { stacked, demand, cap }) } else { Ok(()) }
}
fn plaintext_name(name: &[u8]) -> bool {
    if name.is_empty() {
        return false;
    }
    for byte in name {
        if *byte <= b' ' || *byte >= 127 || b"/:[]".contains(byte) {
            return false;
        }
    }
    true
}
fn header_value(value: &[u8]) -> bool {
    if value.is_empty() {
        return false;
    }
    for byte in value {
        if (*byte < 0x20 && *byte != b'\t') || *byte == 0x7f {
            return false;
        }
    }
    true
}

// Metadata is retained in full by the dialect; the wanted access must fit
// without truncating a subscription or a permission name (5.6, step 2).
fn app_metadata(l: &Limits) -> Result<(), Unusable> {
    let inner = u32::try_from(dialect::setup::PERMISSIONS.len().max(dialect::setup::EVENTS.len()))
        .expect("wanted metadata count fits");
    let mut string_bytes = 0;
    for (name, _) in dialect::setup::PERMISSIONS {
        string_bytes = string_bytes.max(u32::try_from(name.len()).expect("wanted name fits"));
    }
    for name in dialect::setup::EVENTS {
        string_bytes = string_bytes.max(u32::try_from(name.len()).expect("wanted name fits"));
    }
    if l.inner < inner || l.string_bytes < string_bytes {
        return Err(Unusable::GitHubMetadata { inner, string_bytes });
    }
    Ok(())
}
