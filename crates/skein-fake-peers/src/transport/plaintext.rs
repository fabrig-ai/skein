//! The portable transport forwards plaintext without a TLS server (testing.md, 10).

use skein_io as io;
use skein_lib::stream::{Down, OutputUp, Up};
use skein_lib::{Queue, Token};

use super::Transport;
use crate::Limits;

pub(crate) struct Wire {
    socket: Token,
    closing: bool,
}

#[expect(clippy::unused_self, reason = "plaintext shares the native Wire instance interface")]
impl Wire {
    pub fn new(socket: Token, transport: Transport, _limits: Limits) -> Self {
        assert_eq!(transport, Transport::Plaintext, "the peer refuses TLS before accepting a connection");
        Self { socket, closing: false }
    }

    pub fn up(&mut self, up: Up, above: &mut Queue<Up>, _requests: &mut Queue<io::Request>) {
        if !self.closing {
            above.push(up);
        }
    }

    pub fn down(&mut self, down: Down, _above: &mut Queue<Up>, requests: &mut Queue<io::Request>) {
        requests.push(io::Request::Stream { stream: self.socket, down });
    }

    pub fn output(&mut self, _up: OutputUp, _above: &mut Queue<Up>, _requests: &mut Queue<io::Request>) {
        unreachable!("plaintext has no independent ciphertext output right");
    }

    pub fn pump(&mut self, _above: &mut Queue<Up>, _requests: &mut Queue<io::Request>) {}

    pub fn close(&mut self, requests: &mut Queue<io::Request>) {
        if !self.closing {
            self.closing = true;
            requests.push(io::Request::Close { entity: self.socket });
        }
    }

    pub fn work_pending(&self) -> bool {
        false
    }
}
