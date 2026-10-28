//! Bounded one-shot loopback clients, separate from server admission and Wire.
use super::{Delivery, Observation, Peer};
use crate::Limits;
use skein_fake_forge_domain as domain;
use skein_fake_forge_protocol::{
    self as protocol,
    delivery::{Endpoint, Post},
};
use skein_io as io;
use skein_lib::{
    Token, bytes,
    stream::{Down, Read, Up},
};
use std::mem::size_of;

pub(super) struct Outbound {
    owner: Token,
    socket: Option<Token>,
    post: Option<Post>,
    sent: usize,
}

impl Peer {
    pub(super) fn deliver(&mut self, hook: &domain::Request) {
        if self.endpoint == Some(false) {
            self.delivery_observation(None, Delivery::NoAddress);
            return;
        }
        // Each domain hook leaves one record at least: a hook no address takes is NoAddress.
        let posts = self.service.deliveries(hook, &self.protocol);
        if posts.is_empty() {
            let post = self.service.delivery(hook, &self.protocol);
            self.delivery_observation(post, Delivery::NoAddress);
        }
        for delivery in posts.into_boxed() {
            self.deliver_to(delivery);
        }
    }

    fn deliver_to(&mut self, delivery: skein_fake_forge_protocol::delivery::Delivery) {
        let skein_fake_forge_protocol::delivery::Delivery { address, post } = delivery;
        if self.deliveries.len() == usize::try_from(self.face.limits.connections).expect("u32 fits")
        {
            self.delivery_observation(Some(post), Delivery::NoRoom);
            return;
        }
        let owner = Token::new(self.next);
        self.next = self.next.checked_add(1).filter(|next| *next < u64::MAX).expect("finite client identities");
        self.deliveries.push(Outbound { owner, socket: None, post: Some(post), sent: 0 });
        self.face.requests.push(io::Request::Connect { owner, addr: io::Addr::Ip(address) });
    }

    pub(super) fn delivery_event(&mut self, event: io::Event) -> Option<io::Event> {
        let owner = match &event {
            io::Event::Connecting { owner, .. }
            | io::Event::Connected { owner }
            | io::Event::Stream { owner, .. }
            | io::Event::Failed { owner, .. }
            | io::Event::Closed { owner } => *owner,
            io::Event::Listening { .. }
            | io::Event::Accepted { .. }
            | io::Event::Output { .. }
            | io::Event::Spawned { .. }
            | io::Event::Exited { .. }
            | io::Event::Usage { .. }
            | io::Event::Resized
            | io::Event::Window { .. }
            | io::Event::Shutdown { .. } => return Some(event),
        };
        let Some(index) = self.deliveries.iter().position(|delivery| delivery.owner == owner) else {
            return Some(event);
        };
        match event {
            io::Event::Connecting { socket, .. } => self.deliveries[index].socket = Some(socket),
            io::Event::Connected { .. } => self.delivery_room(index),
            io::Event::Stream { up: Up::Room, .. } => self.delivery_write(index),
            io::Event::Stream { up: Up::Failed(_), .. } => {
                self.delivery_terminal(index, Delivery::Reset);
                if let Some(socket) = self.deliveries[index].socket {
                    self.face.requests.push(io::Request::Abort { entity: socket });
                }
            }
            io::Event::Stream { up: Up::End, .. } => {}
            io::Event::Failed { error, .. } => {
                let outcome = match error {
                    io::Error::Busy => Delivery::NoRoom,
                    io::Error::Reset => Delivery::Reset,
                    io::Error::Refused | io::Error::Unreachable | io::Error::TimedOut | io::Error::Other => {
                        Delivery::Refused
                    }
                };
                self.delivery_terminal(index, outcome);
            }
            io::Event::Closed { .. } => {
                self.delivery_terminal(index, Delivery::Reset);
                self.deliveries.remove(index);
            }
            io::Event::Listening { .. }
            | io::Event::Accepted { .. }
            | io::Event::Output { .. }
            | io::Event::Spawned { .. }
            | io::Event::Exited { .. }
            | io::Event::Usage { .. }
            | io::Event::Resized
            | io::Event::Window { .. }
            | io::Event::Shutdown { .. }
            | io::Event::Stream { up: Up::Bytes(_), .. } => {
                unreachable!("a write-only client receives only its connection and stream events")
            }
        }
        None
    }

    fn delivery_room(&mut self, index: usize) {
        let socket = self.deliveries[index].socket.expect("connecting precedes connected");
        // Asking for the whole cap grants only after the preceding Send has drained.
        self.face.requests.push(io::Request::Stream {
            stream: socket,
            down: Down::Demand { read: Read::Nothing, room: self.face.limits.io.output },
        });
    }

    fn delivery_write(&mut self, index: usize) {
        let outbound = &mut self.deliveries[index];
        let Some(post) = &outbound.post else { return };
        let socket = outbound.socket.expect("a connected client has its socket");
        let length = post.head.len().checked_add(post.body.len()).expect("bounded request");
        if outbound.sent == length {
            self.delivery_terminal(index, Delivery::Written);
            self.face.requests.push(io::Request::Close { entity: socket });
            return;
        }
        let remaining = if outbound.sent < post.head.len() {
            post.head.get(outbound.sent..).expect("head offset")
        } else {
            post.body.get(outbound.sent.checked_sub(post.head.len()).expect("body offset")..).expect("body offset")
        };
        let count = remaining.len().min(usize::try_from(self.face.limits.io.output).expect("u32 fits"));
        let chunk = bytes::copy_of(remaining.get(..count).expect("bounded chunk"));
        outbound.sent = outbound.sent.checked_add(count).expect("bounded write offset");
        self.face.requests.push(io::Request::Stream { stream: socket, down: Down::Send(chunk) });
        self.delivery_room(index);
    }

    fn delivery_terminal(&mut self, index: usize, outcome: Delivery) {
        if let Some(post) = self.deliveries[index].post.take() {
            self.delivery_observation(Some(post), outcome);
        }
    }
    fn delivery_observation(&mut self, post: Option<Post>, outcome: Delivery) {
        if !self.kept.deliveries {
            return;
        }
        let bytes = post.as_ref().map_or(0, |post| {
            u64::try_from(post.head.len())
                .expect("head size")
                .checked_add(u64::try_from(post.body.len()).expect("body size"))
                .expect("bounded post")
        });
        self.observe(Observation::Delivery { post, outcome }, bytes);
    }
}

pub(super) fn worst_case(limits: &Limits, protocol: &protocol::Limits, endpoint: Option<&Endpoint>) -> Option<u64> {
    let host = u64::try_from(endpoint.map_or(0, |endpoint| endpoint.host.len()))
        .ok()?
        .max(u64::from(protocol.documents.string_bytes));
    let secret = u64::try_from(endpoint.map_or(0, |endpoint| endpoint.secret.len()))
        .ok()?
        .max(u64::from(protocol.documents.string_bytes));
    // A setup URL and secret each decode at documents.string_bytes; target and Host
    // each fit that URL bound. Fixed fields and delimiters fit in 512 bytes.
    let post = host
        .checked_add(u64::from(protocol.documents.string_bytes))?
        .checked_add(512)?
        .checked_add(u64::from(protocol.documents.writer.length))?;
    let slot =
        u64::try_from(size_of::<Outbound>()).ok()?.checked_add(post)?.checked_add(u64::from(limits.io.output))?;
    u64::from(limits.connections).checked_mul(slot)?.checked_add(host)?.checked_add(secret)?.checked_add(post)
}
