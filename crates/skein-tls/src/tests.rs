//! skein-tls's step tests (testing-strategy.md, 2.1): its limits, its
//! configuration, the bytes it holds for rustls, and the machine fed by
//! hand, one event at a time, with records written by hand in place of a
//! server's. The handshakes and exchanges with a server are its machine
//! worlds, in tests/tls.

#![expect(clippy::disallowed_types, reason = "a test collects what a step emitted in a Vec")]

mod config;
mod held;
mod machine;
mod rs256;

use alloc::boxed::Box;
use alloc::vec::Vec;

use rustls::RootCertStore;
use rustls::pki_types::{Der, TrustAnchor};
use skein_lib::stream::{Down, Read, Up};
use skein_lib::{Env, Queue, Time, Wall};

use crate::client::{self, Client, Error, Event, Limits, Request};
use crate::{Config, Name};

/// The limits shared by the step tests.
pub(crate) const LIMITS: Limits = Limits { read: 16, send: 16, records: client::MAX_RECORD };

/// 2030-01-01, within every fixture certificate's validity.
pub(crate) const VALID: Wall = Wall::from_nanos(1_893_456_000 * 1_000_000_000);
pub(crate) const WITHDRAW: Down = Down::Demand { read: Read::Nothing, room: 0 };

/// A certificate or key in tests/tls/fixtures, read from disk.
pub(crate) fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::PathBuf::from("../../tests/tls/fixtures").join(name);
    std::fs::read(path).expect("fixture bytes")
}

/// What a failure tells the side above.
pub(crate) fn failed(error: Error) -> Vec<Event> {
    Vec::from([Event::Stream(Up::Failed(error.fault())), Event::Failed(error)])
}

/// A TLS 1.2 rustls server for the native step tests.
pub(crate) fn native_peer() -> (Config, rustls::server::UnbufferedServerConnection) {
    use alloc::sync::Arc;
    use rustls::crypto::ring;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(fixture("root.der"))).expect("fixture root");
    let config = Config::new(roots, &[]).expect("client roots");
    let certificates =
        Vec::from([CertificateDer::from(fixture("leaf.der")), CertificateDer::from(fixture("intermediate.der"))]);
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(fixture("leaf.key")));
    let server = rustls::server::ServerConfig::builder_with_details(
        Arc::new(ring::default_provider()),
        config.at(VALID).time_provider.clone(),
    )
    .with_protocol_versions(&[&rustls::version::TLS12])
    .expect("TLS 1.2 provider")
    .with_no_client_auth()
    .with_single_cert(certificates, key)
    .expect("fixture key");
    let peer = rustls::server::UnbufferedServerConnection::new(Arc::new(server)).expect("rustls peer");
    (config, peer)
}

/// Roots of one anchor, which nothing a test sends is signed by: a
/// configuration needs a root, and these handshakes never reach one.
fn roots() -> RootCertStore {
    let mut roots = RootCertStore::empty();
    roots.roots.push(TrustAnchor {
        subject: Der::from_slice(b"a test root"),
        subject_public_key_info: Der::from_slice(b"its key"),
        name_constraints: None,
    });
    roots
}

/// A configuration with the step tests' synthetic root.
pub(crate) fn config() -> Config {
    Config::new(roots(), &[]).unwrap()
}

/// A client and its two queues, with room for what one call emits.
struct Machine {
    client: Client,
    env: Env<Limits>,
    above: Queue<Event>,
    below: Queue<Down>,
}

impl Machine {
    fn new(limits: Limits) -> Machine {
        Machine {
            client: Client::new(&config(), Name::new("skein.test").unwrap(), &limits),
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            above: Queue::uncapped(),
            below: Queue::uncapped(),
        }
    }

    /// Sends `rq` down; what came of it, above and below.
    fn down(&mut self, rq: Request) -> (Vec<Event>, Vec<Down>) {
        client::down(&mut self.client, &self.env, rq, &mut self.above, &mut self.below);

        self.take()
    }

    /// Sends `ev` up; what came of it, above and below.
    fn up(&mut self, ev: Up) -> (Vec<Event>, Vec<Down>) {
        client::up(&mut self.client, &self.env, ev, &mut self.above, &mut self.below);

        self.take()
    }

    fn bytes(&mut self, bytes: &[u8]) -> (Vec<Event>, Vec<Down>) {
        self.up(Up::Bytes(Box::from(bytes)))
    }

    fn take(&mut self) -> (Vec<Event>, Vec<Down>) {
        let mut events = Vec::new();
        while let Some(event) = self.above.pop() {
            events.push(event);
        }
        let mut requests = Vec::new();
        while let Some(request) = self.below.pop() {
            requests.push(request);
        }
        (events, requests)
    }
}
