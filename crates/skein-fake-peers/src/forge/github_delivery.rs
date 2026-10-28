//! GitHub's bounded best-effort POST exchange, retaining the recipient's status.
use super::{Delivery, Observation, Peer, layer};
use skein_fake_forge_github::{self as github, delivery::Post};
use skein_http::{Header, Method, client};
use skein_io as io;
use skein_lib::{
    Duration, Env, Queue, Time, Token, Wall, bytes,
    stream::{Down, Read, Up},
};
use std::net::SocketAddr;

pub(super) struct Outbound {
    owner: Token,
    socket: Option<Token>,
    post: Option<Post>,
    target: Box<[u8]>,
    client: client::Client,
    events: Queue<client::Event>,
    below: Queue<Down>,
    sent: usize,
    deadline: Time,
    closing: bool,
    close_sent: bool,
}
fn limits(limits: &github::Limits) -> client::Limits {
    client::Limits {
        request: limits.http.response,
        head: limits.http.head,
        headers: limits.http.headers,
        read: limits.http.read,
        send: limits.http.send,
    }
}
impl Peer {
    pub(super) fn github_hook(&mut self, hook: &skein_fake_forge_domain::Request, now: Time, wall: Wall) {
        let layer::Service::Github(service) = &mut self.service else { unreachable!("GitHub hook owner") };
        match service.hook(hook, wall).expect("admitted GitHub hook bounds") {
            Some(post) => self.github_post(post, now),
            None => {
                if self.kept.deliveries {
                    self.observe(Observation::Delivery { post: None, outcome: Delivery::NoAddress }, 0);
                }
            }
        }
    }
    fn github_post(&mut self, post: Post, now: Time) {
        if self.github_deliveries.len() == usize::try_from(self.face.limits.connections).expect("u32 fits") {
            self.github_result(post, 502, Delivery::NoRoom);
            return;
        }
        let layer::Service::Github(service) = &self.service else { unreachable!("GitHub sender") };
        let endpoint = service.endpoint().expect("post has endpoint");
        let address: SocketAddr = std::str::from_utf8(&endpoint.host)
            .expect("numeric loopback endpoint")
            .parse()
            .expect("admitted loopback endpoint");
        let layer::Limits::Github(protocol) = &self.protocol else { unreachable!("GitHub limits") };
        let owner = Token::new(self.next);
        self.next = self.next.checked_add(1).filter(|next| *next < u64::MAX).expect("finite client identities");
        self.github_deliveries.push(Outbound {
            owner,
            socket: None,
            post: Some(post),
            target: endpoint.target.clone(),
            client: client::Client::new(&limits(protocol)),
            events: Queue::uncapped(),
            below: Queue::uncapped(),
            sent: 0,
            deadline: now.saturating_add(Duration::from_secs(30)),
            closing: false,
            close_sent: false,
        });
        self.face.requests.push(io::Request::Connect { owner, addr: io::Addr::Ip(address) });
    }
    pub(super) fn github_progress(&mut self, now: Time, wall: Wall) {
        if let layer::Service::Github(service) = &mut self.service {
            service.poll(wall);
        } else {
            return;
        }
        for _ in 0..self.face.limits.connections {
            let layer::Service::Github(service) = &mut self.service else { unreachable!("GitHub service") };
            let Some(post) = service.next_post() else { break };
            self.github_post(post, now);
        }
        for index in 0..self.github_deliveries.len() {
            if self.github_deliveries[index].deadline <= now && !self.github_deliveries[index].closing {
                self.github_terminal(index, 502, Delivery::Refused, now, wall);
            }
            self.github_pump(index, now, wall);
        }
    }
    pub(super) fn github_delivery_event(&mut self, event: io::Event, now: Time, wall: Wall) -> Option<io::Event> {
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
        let Some(index) = self.github_deliveries.iter().position(|delivery| delivery.owner == owner) else {
            return Some(event);
        };
        match event {
            io::Event::Connecting { socket, .. } => self.github_deliveries[index].socket = Some(socket),
            io::Event::Connected { .. } => {
                if !self.github_deliveries[index].closing {
                    self.github_start(index, now, wall);
                }
            }
            io::Event::Stream { up, .. } => {
                let env = self.github_env(now, wall);
                let outbound = &mut self.github_deliveries[index];
                client::up(&mut outbound.client, &env, up, &mut outbound.events, &mut outbound.below);
            }
            io::Event::Failed { .. } => self.github_terminal(index, 502, Delivery::Refused, now, wall),
            io::Event::Closed { .. } => {
                if let Some(post) = self.github_deliveries[index].post.take() {
                    self.github_result(post, 502, Delivery::Reset);
                }
                self.github_deliveries.remove(index);
                return None;
            }
            io::Event::Listening { .. }
            | io::Event::Accepted { .. }
            | io::Event::Output { .. }
            | io::Event::Spawned { .. }
            | io::Event::Exited { .. }
            | io::Event::Usage { .. }
            | io::Event::Resized
            | io::Event::Window { .. }
            | io::Event::Shutdown { .. } => unreachable!("outbound owner"),
        }
        self.github_pump(index, now, wall);
        None
    }
    fn github_env(&self, now: Time, wall: Wall) -> Env<client::Limits> {
        let layer::Limits::Github(protocol) = &self.protocol else { unreachable!("GitHub limits") };
        Env { now, wall, limits: limits(protocol) }
    }
    fn github_start(&mut self, index: usize, now: Time, wall: Wall) {
        let env = self.github_env(now, wall);
        let outbound = &mut self.github_deliveries[index];
        let post = outbound.post.as_ref().expect("admitted post");
        let mut headers = Vec::with_capacity(8);
        for line in post.head.split(|byte| *byte == b'\n').skip(1) {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let Some(colon) = line.iter().position(|byte| *byte == b':') else { continue };
            let name = &line[..colon];
            if name.eq_ignore_ascii_case(b"content-length") || name.eq_ignore_ascii_case(b"connection") {
                continue;
            }
            headers.push(Header {
                name: bytes::copy_of(name),
                value: bytes::copy_of(line[colon + 1..].strip_prefix(b" ").unwrap_or(&line[colon + 1..])),
            });
        }
        let length = u64::try_from(post.body.len()).expect("bounded body");
        client::down(
            &mut outbound.client,
            &env,
            client::Request::Call(client::Call {
                method: Method::Post,
                target: outbound.target.clone(),
                headers: headers.into_boxed_slice(),
                body: client::Body::Length(length),
                close: true,
            }),
            &mut outbound.events,
            &mut outbound.below,
        );
        let room = u32::try_from(length).expect("bounded body").min(env.limits.send);
        client::down(
            &mut outbound.client,
            &env,
            client::Request::Upload(Down::Demand { read: Read::Nothing, room }),
            &mut outbound.events,
            &mut outbound.below,
        );
    }
    fn github_pump(&mut self, index: usize, now: Time, wall: Wall) {
        let env = self.github_env(now, wall);
        for _ in 0..8 {
            let Some(event) = self.github_deliveries[index].events.pop() else { break };
            match event {
                client::Event::Response(response) => {
                    self.github_terminal(index, response.status, Delivery::Written, now, wall);
                }
                client::Event::Failed(_) => self.github_terminal(index, 502, Delivery::Reset, now, wall),
                client::Event::Upload(Up::Room) => {
                    let outbound = &mut self.github_deliveries[index];
                    if outbound.closing {
                        continue;
                    }
                    let post = outbound.post.as_ref().expect("active post");
                    let count =
                        (post.body.len() - outbound.sent).min(usize::try_from(env.limits.send).expect("u32 fits"));
                    let chunk = bytes::copy_of(&post.body[outbound.sent..outbound.sent + count]);
                    outbound.sent += count;
                    client::down(
                        &mut outbound.client,
                        &env,
                        client::Request::Upload(Down::Send(chunk)),
                        &mut outbound.events,
                        &mut outbound.below,
                    );
                    let next = if outbound.sent == post.body.len() {
                        Down::Finish
                    } else {
                        Down::Demand {
                            read: Read::Nothing,
                            room: u32::try_from(post.body.len() - outbound.sent)
                                .expect("body size")
                                .min(env.limits.send),
                        }
                    };
                    client::down(
                        &mut outbound.client,
                        &env,
                        client::Request::Upload(next),
                        &mut outbound.events,
                        &mut outbound.below,
                    );
                }
                client::Event::Upload(Up::Bytes(_) | Up::End | Up::Failed(_))
                | client::Event::Body(_)
                | client::Event::Done(_)
                | client::Event::Closed => {}
            }
        }
        let outbound = &mut self.github_deliveries[index];
        if let Some(socket) = outbound.socket {
            for _ in 0..8 {
                let Some(down) = outbound.below.pop() else { break };
                self.face.requests.push(io::Request::Stream { stream: socket, down });
            }
            if outbound.closing && !outbound.close_sent {
                self.face.requests.push(io::Request::Close { entity: socket });
                outbound.close_sent = true;
            }
        }
    }
    fn github_terminal(&mut self, index: usize, status: u16, outcome: Delivery, now: Time, wall: Wall) {
        if self.github_deliveries[index].closing {
            return;
        }
        let env = self.github_env(now, wall);
        let outbound = &mut self.github_deliveries[index];
        outbound.closing = true;
        client::down(&mut outbound.client, &env, client::Request::Close, &mut outbound.events, &mut outbound.below);
        if let Some(post) = outbound.post.take() {
            self.github_result(post, status, outcome);
        }
    }
    fn github_result(&mut self, post: Post, status: u16, outcome: Delivery) {
        let layer::Service::Github(service) = &mut self.service else { unreachable!("GitHub service") };
        service.delivered(post.id, status);
        if !self.kept.deliveries {
            return;
        }
        let size = u64::try_from(post.head.len() + post.body.len()).expect("bounded post");
        let post = skein_fake_forge_protocol::delivery::Post { head: post.head, body: post.body };
        self.observe(Observation::Delivery { post: Some(post), outcome }, size);
    }
}
pub(super) fn deadline(outbound: &Outbound) -> Option<Time> {
    (!outbound.closing).then_some(outbound.deadline)
}
pub(super) fn work(outbound: &Outbound) -> bool {
    !outbound.events.is_empty() || !outbound.below.is_empty()
}
pub(super) fn valid(protocol: &github::Limits) -> bool {
    client::valid(&limits(protocol))
}
