//! One status-only HTTP exchange, on the harness's io loop.
use super::{Asked, Play, RequestFailure};
use skein_http::client::{self, Body, Call, Client};
use skein_io::{self as io, Request};
use skein_lib::{Env, Queue, Time, Token, Wall, stream};
use std::collections::VecDeque;
pub(super) struct Exchange {
    pub person: usize,
    pub status: Option<u16>,
    failure: Option<RequestFailure>,
    pub closed: bool,
    pub requests: VecDeque<Request>,
    owner: Token,
    socket: Option<Token>,
    client: Client,
    env: Env<client::Limits>,
    above: Queue<client::Event>,
    below: Queue<stream::Down>,
    call: Call,
    body: VecDeque<Box<[u8]>>,
}
impl Exchange {
    pub fn new(person: usize, serial: u64, asked: Asked, address: &str, now: Time) -> Self {
        let owner = Token::new((1 << 39) + serial);
        let limits = client::Limits { request: 65_536, head: 65_536, headers: 128, read: 1, send: 65_536 };
        let mut headers = vec![skein_http::Header { name: b"Host".as_slice().into(), value: asked.host }];
        headers.extend(asked.headers.into_iter().map(|(name, value)| skein_http::Header { name, value }));
        let body = if asked.body.is_empty() {
            Body::None
        } else {
            Body::Length(u64::try_from(asked.body.len()).expect("body fits"))
        };
        Self {
            person,
            status: None,
            failure: None,
            closed: false,
            owner,
            socket: None,
            requests: VecDeque::from([Request::Connect {
                owner,
                addr: io::Addr::Ip(address.parse().expect("server socket address")),
            }]),
            client: Client::new(&limits),
            env: Env { now, wall: Wall::EPOCH, limits },
            above: Queue::with_capacity(16),
            below: Queue::with_capacity(16),
            call: Call { method: asked.method, target: asked.path, headers: headers.into(), body, close: true },
            body: asked.body.chunks(65_536).map(Box::from).collect(),
        }
    }
    pub fn update(&mut self, now: Time, wall: Wall) {
        self.env.now = now;
        self.env.wall = wall;
    }
    pub fn answer(&mut self, play: &mut dyn Play) {
        if let Some(status) = self.status.take() {
            match self.failure.take() {
                Some(failure) => play.request_failed(failure),
                None => play.answered(status),
            }
        }
    }
    pub fn stop(&mut self) {
        // The client can be closed while the socket's terminal event is pending.
        if self.client.waiting() != client::Waiting::Nothing {
            client::down(&mut self.client, &self.env, client::Request::Close, &mut self.above, &mut self.below);
        }
        while self.below.pop().is_some() {}
        if let Some(entity) = self.socket.take() {
            self.requests.push_back(Request::Abort { entity });
        }
    }
    fn down(&mut self, request: client::Request) {
        client::down(&mut self.client, &self.env, request, &mut self.above, &mut self.below);
    }
    fn event(&mut self, event: io::Event) {
        match event {
            io::Event::Connecting { socket, .. } => self.socket = Some(socket),
            io::Event::Connected { .. } => {
                self.down(client::Request::Call(self.call.clone()));
                if !self.body.is_empty() {
                    self.down(client::Request::Upload(stream::Down::Demand {
                        read: stream::Read::Nothing,
                        room: self.env.limits.send,
                    }));
                }
            }
            io::Event::Stream { up, .. } => {
                client::up(&mut self.client, &self.env, up, &mut self.above, &mut self.below);
            }
            io::Event::Failed { error, .. } => {
                self.failure = Some(RequestFailure::Io(error));
                self.status = Some(0);
                self.stop();
            }
            io::Event::Closed { .. } => {
                self.closed = true;
                self.socket = None;
            }
            other @ (io::Event::Listening { .. }
            | io::Event::Accepted { .. }
            | io::Event::Output { .. }
            | io::Event::Spawned { .. }
            | io::Event::Exited { .. }
            | io::Event::Usage { .. }
            | io::Event::Resized
            | io::Event::Window { .. }
            | io::Event::Shutdown { .. }) => panic!("unexpected request event: {other:?}"),
        }
        while let Some(event) = self.above.pop() {
            match event {
                client::Event::Response(response) => {
                    self.status = Some(response.status);
                    self.stop();
                }
                client::Event::Failed(error) => {
                    self.failure = Some(RequestFailure::Http(error));
                    self.status = Some(0);
                    self.stop();
                }
                client::Event::Upload(stream::Up::Room) => {
                    let chunk = self.body.pop_front().expect("upload chunk");
                    self.down(client::Request::Upload(stream::Down::Send(chunk)));
                    self.down(client::Request::Upload(if self.body.is_empty() {
                        stream::Down::Finish
                    } else {
                        stream::Down::Demand { read: stream::Read::Nothing, room: self.env.limits.send }
                    }));
                }
                client::Event::Upload(stream::Up::Bytes(_) | stream::Up::End | stream::Up::Failed(_))
                | client::Event::Body(_)
                | client::Event::Done(_)
                | client::Event::Closed => {}
            }
        }
        while let Some(down) = self.below.pop() {
            if let Some(socket) = self.socket {
                self.requests.push_back(Request::Stream { stream: socket, down });
            }
        }
    }
}
pub(super) fn route(exchanges: &mut [Exchange], event: io::Event) -> Option<io::Event> {
    if let io::Event::Connecting { owner, .. }
    | io::Event::Connected { owner }
    | io::Event::Stream { owner, .. }
    | io::Event::Failed { owner, .. }
    | io::Event::Closed { owner } = &event
        && let Some(exchange) = exchanges.iter_mut().find(|exchange| exchange.owner == *owner)
    {
        exchange.event(event);
        return None;
    }
    Some(event)
}

#[cfg(test)]
mod tests;
