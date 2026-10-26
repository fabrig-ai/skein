//! One HTTP client per request, named independently of the page's exchange id.
use super::{Limits, cookie::Jar};
use skein_http::{Header, client as http};
use skein_io as io;
use skein_lib::stream::{Down, Read, Up};
use skein_lib::{Env, Queue, Time, Token, Wall};
use skein_ui_web::fetch::Report;

pub(super) const ROOM: u32 = 8;

pub(super) enum Kind {
    Exchange { id: u32, bound: u32, received: Vec<u8> },
    Stream { bound: u32, opened: bool, reading: bool },
}
pub(super) struct Connection {
    pub(super) owner: Token,
    pub(super) socket: Option<Token>,
    pub(super) connected: bool,
    pub(super) lost: bool,
    pub(super) kind: Kind,
    machine: http::Client,
    events: Queue<http::Event>,
    requests: Queue<http::Request>,
    below: Queue<Down>,
    upload: Box<[u8]>,
    offset: usize,
    status: Option<u16>,
}
impl Connection {
    pub(super) fn new(owner: Token, kind: Kind, call: http::Call, upload: Box<[u8]>, limits: &Limits) -> Self {
        let mut requests = Queue::with_capacity(ROOM);
        requests.push(http::Request::Call(call));
        if !upload.is_empty() {
            requests.push(http::Request::Upload(Down::Demand {
                read: Read::Nothing,
                room: limits.http.send.min(u32::try_from(upload.len()).expect("bounded upload")),
            }));
        }
        Self {
            owner,
            kind,
            socket: None,
            connected: false,
            lost: false,
            machine: http::Client::new(&limits.http),
            events: Queue::with_capacity(ROOM),
            requests,
            below: Queue::with_capacity(ROOM),
            upload,
            offset: 0,
            status: None,
        }
    }
    pub(super) fn takes(&self) -> bool {
        self.events.room() >= http::UP_MAX_OUT.above && self.below.room() >= http::UP_MAX_OUT.below
    }
    pub(super) fn up(&mut self, up: Up, now: Time, wall: Wall, limits: &Limits) {
        http::up(&mut self.machine, &Env { now, wall, limits: limits.http }, up, &mut self.events, &mut self.below);
    }
    pub(super) fn read_on(&mut self, limits: &Limits) {
        if let Kind::Stream { bound, opened, reading } = &mut self.kind {
            // A read-on crossing the head or an end carries no new demand.
            if !*opened {
                return;
            }
            assert!(!*reading, "one stream read outstanding");
            *reading = true;
            self.requests.push(http::Request::Body(Down::Demand {
                read: Read::Arrived { max: (*bound).min(limits.http.read) },
                room: 0,
            }));
        }
    }
    fn demand(&mut self, limits: &Limits) {
        if let Kind::Exchange { bound, received, .. } = &self.kind {
            let remaining = bound.saturating_sub(u32::try_from(received.len()).expect("bounded answer"));
            self.requests.push(http::Request::Body(Down::Demand {
                // A final byte distinguishes an answer at its bound from one past it.
                read: Read::Arrived { max: remaining.saturating_add(1).min(limits.http.read).max(1) },
                room: 0,
            }));
        }
    }
    pub(super) fn failed(&self) -> Report {
        match self.kind {
            Kind::Exchange { id, .. } => Report::Failed { exchange: id },
            Kind::Stream { .. } => Report::StreamEnded,
        }
    }
    pub(super) fn progress(
        &mut self,
        now: Time,
        wall: Wall,
        limits: &Limits,
        jar: &mut Jar,
        reports: &mut Queue<Report>,
        io_requests: &mut Queue<io::Request>,
    ) -> bool {
        if self.lost && self.events.is_empty() {
            reports.push(self.failed());
            return true;
        }
        if !self.connected {
            return false;
        }
        if let Some(event) = self.events.pop() {
            match event {
                http::Event::Response(response) => {
                    for Header { name, value } in &response.headers {
                        if name.eq_ignore_ascii_case(b"set-cookie") {
                            jar.set(value, wall);
                        }
                    }
                    self.status = Some(response.status);
                    match &mut self.kind {
                        Kind::Exchange { .. } => self.demand(limits),
                        Kind::Stream { opened, .. } => {
                            if response.status != 200 {
                                reports.push(Report::StreamRefused { status: response.status });
                                return true;
                            }
                            *opened = true;
                            reports.push(Report::StreamOpened);
                        }
                    }
                }
                http::Event::Upload(Up::Room) => {
                    let end = self
                        .offset
                        .saturating_add(usize::try_from(limits.http.send).expect("send fits"))
                        .min(self.upload.len());
                    self.requests.push(http::Request::Upload(Down::Send(self.upload[self.offset..end].into())));
                    self.offset = end;
                    if end == self.upload.len() {
                        self.requests.push(http::Request::Upload(Down::Finish));
                        self.upload = Box::from([]);
                    } else {
                        self.requests.push(http::Request::Upload(Down::Demand {
                            read: Read::Nothing,
                            room: limits.http.send.min(u32::try_from(self.upload.len() - end).expect("bounded upload")),
                        }));
                    }
                }
                http::Event::Body(Up::Bytes(bytes)) => match &mut self.kind {
                    Kind::Exchange { bound, received, .. } => {
                        if bytes.len() > usize::try_from(*bound).expect("answer fits").saturating_sub(received.len()) {
                            reports.push(self.failed());
                            return true;
                        }
                        received.extend_from_slice(&bytes);
                        self.demand(limits);
                    }
                    Kind::Stream { reading, .. } => {
                        assert!(*reading, "a piece answers a read-on");
                        *reading = false;
                        reports.push(Report::StreamPiece { bytes });
                    }
                },
                http::Event::Body(Up::End) => {
                    let report = match &mut self.kind {
                        Kind::Exchange { id, received, .. } => Report::Answered {
                            exchange: *id,
                            status: self.status.expect("body after head"),
                            body: core::mem::take(received).into_boxed_slice(),
                        },
                        Kind::Stream { .. } => Report::StreamEnded,
                    };
                    reports.push(report);
                    return true;
                }
                http::Event::Failed(_) | http::Event::Body(Up::Failed(_)) => {
                    reports.push(self.failed());
                    return true;
                }
                // An early final response can stop the upload and still be answered.
                http::Event::Upload(Up::Failed(_)) | http::Event::Done(_) | http::Event::Closed => {}
                http::Event::Upload(Up::Bytes(_) | Up::End) | http::Event::Body(Up::Room) => {
                    unreachable!("HTTP client stream directions")
                }
            }
        }
        if self.events.room() >= http::DOWN_MAX_OUT.above
            && self.below.room() >= http::DOWN_MAX_OUT.below
            && let Some(request) = self.requests.pop()
        {
            http::down(
                &mut self.machine,
                &Env { now, wall, limits: limits.http },
                request,
                &mut self.events,
                &mut self.below,
            );
        }
        while let Some(down) = self.below.pop() {
            io_requests.push(io::Request::Stream { stream: self.socket.expect("connected socket"), down });
        }
        false
    }
    pub(super) fn has_work(&self) -> bool {
        self.connected && (!self.events.is_empty() || !self.requests.is_empty() || !self.below.is_empty())
    }
}
