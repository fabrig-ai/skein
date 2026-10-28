//! One public sign-in's loopback listener and bounded request head (oauth.md,
//! section 6.5). It knows the bound URI, no expected state or verifier. It hands a
//! decoded redirect to its component, answers its verdict, and settles at io.

#![expect(
    clippy::single_match,
    clippy::manual_let_else,
    reason = "explicit optional socket and callback presence at the io boundary"
)]

use crate::Limits;
use crate::exchange::Exchange;
use crate::redirect;
use crate::route::{self, Socket};
use alloc::boxed::Box;
use skein_http::{Header, server as http};
use skein_io::{Event, Request};
use skein_lib::stream::{Down, Read, Up};
use skein_lib::{Env, Id, Queue, Token, bytes};
use skein_oauth::Redirect;

const PAGE: &[u8] = b"Sign-in received. You may close this page.\n";
const REFUSAL: &[u8] = b"This is not the registered redirect.\n";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Listening,
    Received,
    Closing,
    Aborting,
    Closed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Head {
    Reading,
    Judging,
    Responding,
    Finishing,
    Closing,
    Aborting,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Binding {
    Fixed,
    Preferred,
    Retry,
    Ephemeral,
}

pub(crate) struct Listener {
    pub(crate) return_location: Option<Box<[u8]>>,
    pub(crate) registered: Box<[u8]>,
    binding: Binding,
    pub(crate) sent: bool,
    socket: Option<Token>,
    close_sent: bool,
    phase: Phase,
    callback: Option<Callback>,
    pub(crate) read: Option<Redirect>,
    pub(crate) failure: Option<skein_oauth::Failure>,
}

struct Callback {
    socket: Token,
    http: http::Server,
    events: Queue<http::Event>,
    requests: Queue<http::Request>,
    below: Queue<Down>,
    page: &'static [u8],
    send: u32,
    offset: usize,
    phase: Head,
}

impl Listener {
    pub(crate) fn new(registered: &[u8], varying: bool, return_location: Option<Box<[u8]>>) -> Self {
        Listener {
            return_location,
            registered: bytes::copy_of(registered),
            binding: if !varying {
                Binding::Fixed
            } else if redirect::port(registered) == Some(0) {
                Binding::Ephemeral
            } else {
                Binding::Preferred
            },
            sent: false,
            socket: None,
            close_sent: false,
            phase: Phase::Listening,
            callback: None,
            read: None,
            failure: None,
        }
    }

    pub(crate) fn address(&self) -> core::net::SocketAddr {
        redirect::listen_address(&self.registered, redirect::port(&self.registered).expect("public redirect"))
            .expect("public redirect")
    }

    pub(crate) fn fixed(&self) -> bool {
        self.binding == Binding::Fixed
    }

    pub(crate) fn listen(&mut self, id: Id<Exchange>, io: &mut Queue<Request>) {
        self.sent = true;
        io.push(Request::Listen {
            owner: route::owner(id, Socket::Listener),
            addr: skein_io::Addr::Ip(self.address()),
        });
    }

    pub(crate) fn bound_port(&self) -> Option<u16> {
        if self.binding == Binding::Fixed { None } else { redirect::port(&self.registered) }
    }

    pub(crate) fn ready(&self) -> bool {
        self.socket.is_some() && self.phase == Phase::Listening
    }

    pub(crate) fn settled(&self) -> bool {
        self.phase == Phase::Closed && self.callback.is_none()
    }

    pub(crate) fn up(
        &mut self,
        env: &Env<Limits>,
        id: Id<Exchange>,
        socket: Socket,
        event: Event,
        io: &mut Queue<Request>,
    ) {
        match socket {
            Socket::Listener => match event {
                Event::Listening { listener, addr, .. } => {
                    if let skein_io::Addr::Ip(address) = addr {
                        self.registered =
                            redirect::with_port(&self.registered, address.port()).expect("validated redirect");
                    }
                    if self.binding != Binding::Fixed {
                        self.binding = Binding::Ephemeral;
                    }
                    self.socket = Some(listener);
                    self.end_listener(io);
                }
                Event::Accepted { socket, .. } => {
                    if self.phase != Phase::Listening || self.callback.is_some() {
                        io.push(Request::Reject { socket });
                    } else {
                        self.callback = Some(Callback::new(socket, &env.limits));
                        io.push(Request::Bind { socket, owner: route::owner(id, Socket::Callback) });
                    }
                }
                Event::Failed { .. } => {
                    if self.socket.is_none() && self.binding == Binding::Preferred && self.phase == Phase::Listening {
                        self.binding = Binding::Retry;
                    } else {
                        self.failure = Some(skein_oauth::Failure::Unavailable);
                    }
                }
                Event::Closed { .. } => {
                    self.socket = None;
                    if self.binding == Binding::Retry && self.phase == Phase::Listening {
                        self.binding = Binding::Ephemeral;
                        io.push(Request::Listen {
                            owner: route::owner(id, Socket::Listener),
                            addr: skein_io::Addr::Ip(
                                redirect::listen_address(&self.registered, 0).expect("validated redirect"),
                            ),
                        });
                    } else {
                        self.phase = Phase::Closed;
                    }
                }
                Event::Connecting { .. }
                | Event::Connected { .. }
                | Event::Stream { .. }
                | Event::Output { .. }
                | Event::Spawned { .. }
                | Event::Exited { .. }
                | Event::Usage { .. }
                | Event::Resized
                | Event::Window { .. }
                | Event::Shutdown { .. } => {}
            },
            Socket::Callback => match event {
                Event::Stream { up, .. } => match &mut self.callback {
                    Some(callback) if callback.phase != Head::Closing && callback.phase != Head::Aborting => http::up(
                        &mut callback.http,
                        &Env { now: env.now, wall: env.wall, limits: env.limits.server },
                        up,
                        &mut callback.events,
                        &mut callback.below,
                    ),
                    Some(_) | None => {}
                },
                Event::Closed { .. } => self.callback = None,
                Event::Failed { .. } => match &mut self.callback {
                    Some(callback) => callback.stop(env, false, io),
                    None => {}
                },
                Event::Listening { .. }
                | Event::Accepted { .. }
                | Event::Connecting { .. }
                | Event::Connected { .. }
                | Event::Output { .. }
                | Event::Spawned { .. }
                | Event::Exited { .. }
                | Event::Usage { .. }
                | Event::Resized
                | Event::Window { .. }
                | Event::Shutdown { .. } => {}
            },
            Socket::Web => {}
        }
    }

    pub(crate) fn stop_waiting(&mut self) {
        if self.phase == Phase::Listening {
            self.phase = Phase::Received;
        }
        if self.read.take().is_some() {
            self.answer(false);
        }
    }

    /// Answers only the component's judgement (oauth.md, 6.5).
    pub(crate) fn answer(&mut self, taken: bool) {
        let location = if taken { self.return_location.take() } else { None };
        if let Some(callback) = &mut self.callback
            && callback.phase == Head::Judging
        {
            callback.answer(taken, location);
        }
    }

    fn end_listener(&mut self, io: &mut Queue<Request>) {
        if (self.phase == Phase::Received || self.phase == Phase::Closing || self.phase == Phase::Aborting)
            && !self.close_sent
        {
            match self.socket {
                Some(socket) => {
                    self.close_sent = true;
                    io.push(if self.phase == Phase::Aborting {
                        Request::Abort { entity: socket }
                    } else {
                        Request::Close { entity: socket }
                    });
                }
                None => {}
            }
        }
    }

    pub(crate) fn close(&mut self, env: &Env<Limits>, abort: bool, io: &mut Queue<Request>) {
        if !self.sent {
            self.phase = Phase::Closed;
        }
        match self.phase {
            Phase::Listening | Phase::Received => self.phase = if abort { Phase::Aborting } else { Phase::Closing },
            Phase::Closing if abort => {
                self.phase = Phase::Aborting;
                self.close_sent = false;
            }
            Phase::Closing | Phase::Aborting | Phase::Closed => {}
        }
        self.end_listener(io);
        match &mut self.callback {
            Some(callback) if abort || callback.phase == Head::Reading || callback.phase == Head::Judging => {
                callback.stop(env, abort, io);
            }
            Some(_) | None => {}
        }
        self.read = None;
        self.return_location = None;
    }

    pub(crate) fn progress(&mut self, env: &Env<Limits>, io: &mut Queue<Request>) {
        self.end_listener(io);
        let callback = match &mut self.callback {
            Some(callback) => callback,
            None => return,
        };
        if callback.phase == Head::Closing || callback.phase == Head::Aborting || callback.phase == Head::Judging {
            return;
        }
        let http_env = Env { now: env.now, wall: env.wall, limits: env.limits.server };
        if let Some(down) = callback.below.pop() {
            io.push(Request::Stream { stream: callback.socket, down });
            return;
        }
        if let Some(request) = callback.requests.pop() {
            http::down(&mut callback.http, &http_env, request, &mut callback.events, &mut callback.below);
            return;
        }
        if let Some(event) = callback.events.pop() {
            match event {
                http::Event::Call(call) => {
                    let redirect = redirect::from_head(&call, &self.registered, &env.limits.client);
                    callback.phase = Head::Judging;
                    if self.phase == Phase::Listening && redirect.is_some() {
                        self.read = redirect;
                    } else {
                        callback.answer(false, None);
                    }
                }
                http::Event::Reply(Up::Room) => {
                    let end = callback
                        .offset
                        .saturating_add(usize::try_from(env.limits.server.send).expect("u32 fits"))
                        .min(callback.page.len());
                    callback.requests.push(http::Request::Reply(Down::Send(bytes::copy_of(
                        callback.page.get(callback.offset..end).expect("fixed page offset"),
                    ))));
                    callback.offset = end;
                    if end == callback.page.len() {
                        callback.requests.push(http::Request::Reply(Down::Finish));
                    } else {
                        callback.requests.push(http::Request::Reply(Down::Demand {
                            read: Read::Nothing,
                            room: env.limits.server.send.min(
                                u32::try_from(callback.page.len().checked_sub(end).expect("page remainder"))
                                    .expect("fixed page"),
                            ),
                        }));
                    }
                }
                http::Event::Done(_)
                | http::Event::Ended
                | http::Event::Failed(_)
                | http::Event::Reply(Up::Failed(_) | Up::End | Up::Bytes(_))
                | http::Event::Refused(_) => callback.phase = Head::Finishing,
                http::Event::Body(_) | http::Event::Closed => {}
            }
            return;
        }
        if callback.phase == Head::Finishing {
            callback.stop(env, false, io);
        }
    }

    pub(crate) fn has_work(&self) -> bool {
        (self.phase == Phase::Received && !self.close_sent && self.socket.is_some())
            || self.read.is_some()
            || self.failure.is_some()
            || match &self.callback {
                Some(callback) => {
                    callback.phase != Head::Closing
                        && callback.phase != Head::Aborting
                        && (callback.phase == Head::Finishing
                            || !callback.events.is_empty()
                            || !callback.requests.is_empty()
                            || !callback.below.is_empty())
                }
                None => false,
            }
    }
}

impl Callback {
    fn new(socket: Token, limits: &Limits) -> Self {
        let mut requests = Queue::uncapped();
        requests.push(http::Request::Next);
        Callback {
            socket,
            http: http::Server::new(&limits.server),
            events: Queue::uncapped(),
            requests,
            below: Queue::uncapped(),
            page: PAGE,
            send: limits.server.send,
            offset: 0,
            phase: Head::Reading,
        }
    }
    fn answer(&mut self, taken: bool, location: Option<Box<[u8]>>) {
        self.phase = Head::Responding;
        self.page = if taken { PAGE } else { REFUSAL };
        self.requests.push(http::Request::Discard);
        let content_type =
            Header { name: bytes::copy_of(b"content-type"), value: bytes::copy_of(b"text/plain; charset=utf-8") };
        let (status, headers): (u16, Box<[Header]>) = match location {
            Some(location) => (
                303,
                Box::new([
                    Header { name: bytes::copy_of(b"location"), value: location },
                    Header { name: bytes::copy_of(b"cache-control"), value: bytes::copy_of(b"no-store") },
                    Header { name: bytes::copy_of(b"referrer-policy"), value: bytes::copy_of(b"no-referrer") },
                    content_type,
                ]),
            ),
            None => (if taken { 200 } else { 404 }, Box::new([content_type])),
        };
        self.requests.push(http::Request::Respond(http::Response {
            status,
            headers,
            body: http::Body::Length(u64::try_from(self.page.len()).expect("fixed page")),
            close: true,
        }));
        self.requests.push(http::Request::Reply(Down::Demand {
            read: Read::Nothing,
            room: self.send.min(u32::try_from(self.page.len()).expect("fixed page")),
        }));
    }

    fn stop(&mut self, env: &Env<Limits>, abort: bool, io: &mut Queue<Request>) {
        if self.phase == Head::Aborting || (self.phase == Head::Closing && !abort) {
            return;
        }
        if self.phase != Head::Closing {
            for _ in 0..self.events.len() {
                drop(self.events.pop());
            }
            for _ in 0..self.requests.len() {
                drop(self.requests.pop());
            }
            for _ in 0..self.below.len() {
                drop(self.below.pop());
            }
            http::down(
                &mut self.http,
                &Env { now: env.now, wall: env.wall, limits: env.limits.server },
                http::Request::Close,
                &mut self.events,
                &mut self.below,
            );
            for _ in 0..self.events.len() {
                drop(self.events.pop());
            }
            for _ in 0..self.below.len() {
                drop(self.below.pop());
            }
        }
        self.phase = if abort { Head::Aborting } else { Head::Closing };
        io.push(if abort { Request::Abort { entity: self.socket } } else { Request::Close { entity: self.socket } });
    }
}

pub(crate) fn worst_case(limits: &Limits) -> Option<u64> {
    u64::from(limits.client.url_bytes)
        .checked_mul(2)?
        .checked_add(u64::from(limits.client.uri_bytes))?
        .checked_add(u64::from(limits.client.document.issued_client_bytes))?
        .checked_add(512)
}
