//! One streaming, authenticated webhook connection (forge-connection.md, 5 and 8).
use super::Hinted;
use crate::{Event, Limits, limits, settings::Settings};
use alloc::boxed::Box;
use skein_forge::api;
use skein_forge::forgejo::plans::hint;
use skein_forge::forgejo::{
    client::delivery as decode,
    delivery::{self, Delivery},
};
use skein_http::{Method, server as http};
use skein_json::tokenizer::{self as json, Tokenizer};
use skein_lib::stream::{Down, Read, Up};
use skein_lib::{
    Env, Queue, Time, Token,
    sha256::{self, Hmac},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Closing {
    Open,
    Pending,
    Sent,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum HintState {
    Awaited,
    Ready,
    Refused,
}
#[derive(Debug)]
pub(crate) struct Hooked {
    pub(super) socket: Token,
    pub(super) deadline: Time,
    pub(super) http: http::Server,
    pub(super) events: Queue<http::Event>,
    pub(super) below: Queue<Down>,
    pub(super) json: Tokenizer,
    pub(super) json_events: Queue<json::Event>,
    pub(super) json_down: Queue<Down>,
    pub(super) decoder: Option<decode::Decoder>,
    pub(super) decoded: Option<Delivery>,
    pub(super) mac: Option<Hmac>,
    pub(super) signature: [u8; 64],
    pub(super) forge: u16,
    pub(super) malformed: bool,
    pub(super) ended: bool,
    pub(super) done: bool,
    pub(super) hint: Option<api::Hint>,
    pub(super) hint_state: HintState,
    pub(super) closing: Closing,
}
impl Hooked {
    pub fn new(socket: Token, env: &Env<Limits>) -> Self {
        let mut c = Self {
            socket,
            deadline: env.now.saturating_add(env.limits.hook_deadline),
            http: http::Server::new(&limits::hook_http(&env.limits)),
            events: Queue::uncapped(),
            below: Queue::uncapped(),
            json: Tokenizer::new(&limits::hook_documents(&env.limits).json),
            json_events: Queue::uncapped(),
            json_down: Queue::uncapped(),
            decoder: None,
            decoded: None,
            mac: None,
            signature: [0; 64],
            forge: 0,
            malformed: false,
            ended: false,
            done: false,
            hint: None,
            hint_state: HintState::Awaited,
            closing: Closing::Open,
        };
        c.down(env, http::Request::Next);
        c
    }
    fn down(&mut self, env: &Env<Limits>, request: http::Request) {
        http::down(
            &mut self.http,
            &Env { now: env.now, wall: env.wall, limits: limits::hook_http(&env.limits) },
            request,
            &mut self.events,
            &mut self.below,
        );
    }
    pub fn stream(&mut self, env: &Env<Limits>, event: Up) {
        if self.closing == Closing::Open {
            http::up(
                &mut self.http,
                &Env { now: env.now, wall: env.wall, limits: limits::hook_http(&env.limits) },
                event,
                &mut self.events,
                &mut self.below,
            );
        }
    }
    fn json_request(&mut self, env: &Env<Limits>, request: json::Request) {
        json::down(
            &mut self.json,
            &Env { now: env.now, wall: env.wall, limits: limits::hook_documents(&env.limits).json },
            request,
            &mut self.json_events,
            &mut self.json_down,
        );
    }
    fn read(&mut self, env: &Env<Limits>) {
        self.down(
            env,
            http::Request::Body(Down::Demand {
                read: Read::Arrived { max: limits::hook_http(&env.limits).read },
                room: 0,
            }),
        );
    }
    fn respond(&mut self, env: &Env<Limits>, status: u16) {
        self.down(
            env,
            http::Request::Respond(http::Response {
                status,
                headers: Box::new([]),
                body: http::Body::None,
                close: status != 204,
            }),
        );
    }
    fn call(&mut self, env: &Env<Limits>, settings: &Settings, call: http::Call) {
        let forge = delivery::hook_forge(&call.target);
        let mut secret = None;
        for configured in &settings.forges {
            if Some(configured.number) == forge {
                secret = match &configured.access {
                    Some(access) => access.webhook_secret.as_ref(),
                    None => None,
                };
                break;
            }
        }
        let status = if secret.is_none() {
            Some(404)
        } else if call.method != Method::Post {
            Some(405)
        } else {
            match signature(call.header(delivery::SIGNATURE_HEADER)) {
                Some(bytes) => {
                    self.signature = bytes;
                    None
                }
                None => Some(403),
            }
        };
        if let Some(status) = status {
            // Respond before Discard: a refusal never demands any body bytes.
            self.respond(env, status);
            self.down(env, http::Request::Discard);
            return;
        }
        self.forge = forge.expect("configured forge");
        self.mac = Some(Hmac::new(secret.expect("configured secret").expose()));
        self.decoder = match call.header(delivery::EVENT_HEADER) {
            Some(event) => decode::decoder(event, &limits::hook_documents(&env.limits)),
            None => None,
        };
        self.json = Tokenizer::new(&limits::hook_documents(&env.limits).json);
        self.decoded = None;
        self.malformed = false;
        self.ended = false;
        self.done = false;
        if let Some(decoder) = &self.decoder {
            self.json_request(env, decode::first(decoder));
        } else {
            self.read(env);
        }
    }
    fn body(&mut self, env: &Env<Limits>, event: Up) {
        match &event {
            Up::Bytes(bytes) => self.mac.as_mut().expect("authenticated head").update(bytes),
            Up::End => self.ended = true,
            Up::Failed(_) => {
                self.closing = Closing::Pending;
                return;
            }
            Up::Room => unreachable!("request body is read only"),
        }
        if self.decoder.is_some() {
            json::up(
                &mut self.json,
                &Env { now: env.now, wall: env.wall, limits: limits::hook_documents(&env.limits).json },
                event,
                &mut self.json_events,
                &mut self.json_down,
            );
        } else if !self.ended {
            self.read(env);
        }
    }
    fn verified(&mut self, env: &Env<Limits>, word: &[u8]) {
        let digest = self.mac.take().expect("body signed").finish();
        let mut signature = [0; 64];
        for (digits, byte) in signature.chunks_exact_mut(2).zip(digest) {
            let [high, low] = digits else { unreachable!("hexadecimal pair") };
            *high = *b"0123456789abcdef".get(usize::from(byte / 16)).expect("hexadecimal digit");
            *low = *b"0123456789abcdef".get(usize::from(byte % 16)).expect("hexadecimal digit");
        }
        let status = if !sha256::equal(&signature, &self.signature) {
            403
        } else if self.malformed {
            400
        } else {
            204
        };
        if status == 204 {
            self.hint = match self.decoded.take() {
                Some(delivery) => hint::hint(self.forge, word, &delivery),
                None => None,
            };
        }
        self.respond(env, status);
    }
    pub fn refused(&mut self, hint: api::Hint) {
        self.hint = Some(hint);
        self.hint_state = HintState::Refused;
    }
    pub fn offer(&mut self) {
        if self.hint_state == HintState::Refused {
            self.hint_state = HintState::Ready;
        }
    }
    pub fn pending(&self) -> bool {
        self.closing == Closing::Pending
            || !self.below.is_empty()
            || !self.events.is_empty() && (self.hint.is_none() || self.hint_state != HintState::Refused)
            || !self.json_events.is_empty()
            || !self.json_down.is_empty()
            || self.ended && self.decoder.is_none() && self.mac.is_some()
            || self.done && self.hint.is_none()
            || self.hint_state == HintState::Ready && self.hint.is_some()
    }
    pub fn pump(
        &mut self,
        owner: Token,
        env: &Env<Limits>,
        settings: &Settings,
        up: &mut Queue<Event>,
        io: &mut Queue<skein_io::Request>,
    ) {
        for _ in 0_u32..64 {
            if let Some(down) = self.below.pop() {
                io.push(skein_io::Request::Stream { stream: self.socket, down });
            } else if self.hint_state == HintState::Ready && self.hint.is_some() {
                self.hint_state = HintState::Awaited;
                up.push(Event::Hint(Hinted { connection: owner, hint: self.hint.take().expect("ready hint") }));
            } else if self.closing == Closing::Pending {
                io.push(skein_io::Request::Close { entity: self.socket });
                self.closing = Closing::Sent;
                break;
            } else if let Some(event) = self.json_events.pop() {
                let decoder = self.decoder.as_mut().expect("decoding body");
                match decode::step(decoder, &limits::hook_documents(&env.limits), &event) {
                    decode::Step::Demand(request) => self.json_request(env, request),
                    decode::Step::Done(delivery) => {
                        self.decoded = Some(delivery);
                        self.decoder = None;
                    }
                    decode::Step::Malformed(_) => {
                        self.malformed = true;
                        self.decoder = None;
                        if !self.ended {
                            self.read(env);
                        }
                    }
                }
            } else if let Some(down) = self.json_down.pop() {
                self.down(env, http::Request::Body(down));
            } else if self.ended && self.decoder.is_none() && self.mac.is_some() {
                self.verified(env, &settings.word);
            } else if let Some(event) = self.events.pop() {
                match event {
                    http::Event::Call(call) => {
                        if self.hint.is_some() {
                            self.events.push(http::Event::Call(call));
                            break;
                        }
                        self.call(env, settings, call);
                    }
                    http::Event::Body(event) => self.body(env, event),
                    http::Event::Done(reuse) => {
                        self.deadline = env.now.saturating_add(env.limits.hook_deadline);
                        self.done = true;
                        self.hint_state = HintState::Ready;
                        if reuse == http::Reuse::Close {
                            self.closing = Closing::Pending;
                        }
                    }
                    http::Event::Closed => {}
                    http::Event::Failed(_) | http::Event::Refused(_) | http::Event::Reply(_) | http::Event::Ended => {
                        self.closing = Closing::Pending;
                    }
                }
            } else if self.done && self.hint.is_none() {
                self.done = false;
                self.down(env, http::Request::Next);
            } else {
                break;
            }
        }
    }
}

fn signature(header: Option<&[u8]>) -> Option<[u8; 64]> {
    let bytes = <[u8; 64]>::try_from(header?).ok()?;
    for byte in &bytes {
        if !byte.is_ascii_digit() && !(b'a'..=b'f').contains(byte) {
            return None;
        }
    }
    Some(bytes)
}
