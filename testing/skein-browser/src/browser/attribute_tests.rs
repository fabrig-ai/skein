//! A fake CDP peer for an attribute read whose element a render replaces.
use super::super::look::Css;
use super::super::{Browser, Page, Phase, fire};
use crate::boundary::{Event, Read, Refusal, Request};
use crate::{Limits, wire::decode::Document};
use alloc::boxed::Box;
use skein_lib::{Env, Queue, Time, Token, Wall};

const PAGE: Token = Token::new(3);
const READ: Token = Token::new(4);
const ROOT: &[u8] = br#"{"depth":0,"pierce":false}"#;
const SELECTOR: &[u8] = br#"{"nodeId":1,"selector":"button"}"#;
const MISSING: &[u8] = br#"{"code":-32000,"message":"Could not find node with given id"}"#;

struct Peer {
    browser: Browser,
    env: Env<Limits>,
}
impl Peer {
    fn new() -> Self {
        let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: Limits::default() };
        let mut browser = Browser::new(Token::new(1), &env.limits);
        browser.phase = Phase::Ready;
        assert!(browser.insert_page(
            Page {
                token: PAGE,
                person: Token::new(2),
                target: Some(Box::from(b"target".as_slice())),
                session: Some(Box::from(b"session".as_slice())),
                initial_url: Box::from(b"about:blank".as_slice()),
                frame: Some(Box::from(b"main".as_slice())),
                navigating: false,
                opening: false,
                load: None,
                load_due: None,
                closed: false,
                css: Css::Disabled,
                caret_document: None,
            },
            env.limits.pages
        ));
        let read = Request::Attribute {
            page: PAGE,
            op: READ,
            selector: Box::from(b"button".as_slice()),
            name: Box::from(b"aria-pressed".as_slice()),
        };
        browser.request(&env, read);
        Self { browser, env }
    }
    fn reply(&mut self, method: &[u8], params: &[u8], field: &[u8], result: &[u8]) {
        let index = self.browser.sendable().expect("next command is sendable");
        let command = self.browser.outgoing.remove(index).expect("next command");
        let bytes = command.bytes.strip_suffix(&[0]).expect("CDP delimiter");
        let document = Document::parse(bytes, self.env.limits.command).expect("encoded command");
        assert_eq!(document.root().get(b"method").expect("method").text().expect("method text").as_ref(), method);
        assert_eq!(document.root().get(b"params").expect("parameters").raw(), params);
        let mut reply = Vec::new();
        reply.extend_from_slice(b"{\"id\":");
        reply.extend_from_slice(document.root().get(b"id").expect("id").raw());
        reply.extend_from_slice(b",\"");
        reply.extend_from_slice(field);
        reply.extend_from_slice(b"\":");
        reply.extend_from_slice(result);
        reply.extend_from_slice(b"}");
        self.browser.message(&self.env, &reply);
    }
    /// The document and the selector answer; the selector finds node 7.
    fn selected(&mut self) {
        self.reply(b"DOM.getDocument", ROOT, b"result", br#"{"root":{"nodeId":1}}"#);
        self.reply(b"DOM.querySelector", SELECTOR, b"result", br#"{"nodeId":7}"#);
    }
    /// The read's element is gone by its attributes' read.
    fn replaced(&mut self) {
        self.selected();
        self.reply(b"DOM.getAttributes", br#"{"nodeId":7}"#, b"error", MISSING);
        assert!(self.browser.events.is_empty(), "a replaced element is no terminal");
    }
    fn polled(&mut self) {
        self.env.now = self.env.now.saturating_add(self.env.limits.poll);
        let (mut above, mut below) = (Queue::uncapped(), Queue::uncapped());
        fire(&mut self.browser, &self.env, self.env.now, &mut above, &mut below);
    }
}

#[test]
fn a_replaced_element_is_read_again_from_the_document() {
    let mut peer = Peer::new();
    peer.replaced();
    assert!(peer.browser.sendable().is_none(), "the read waits a poll");
    peer.polled();
    peer.selected();
    peer.reply(b"DOM.getAttributes", br#"{"nodeId":7}"#, b"result", br#"{"attributes":["aria-pressed","true"]}"#);
    let Some(Event::Attribute { op: READ, read: Read::Value(value) }) = peer.browser.events.pop_front() else {
        panic!("the read's value")
    };
    assert_eq!(value.as_ref(), b"true");
}

#[test]
fn a_document_replaced_before_the_selector_is_read_again() {
    let mut peer = Peer::new();
    peer.reply(b"DOM.getDocument", ROOT, b"result", br#"{"root":{"nodeId":1}}"#);
    peer.reply(b"DOM.querySelector", SELECTOR, b"error", MISSING);
    assert!(peer.browser.events.is_empty(), "a replaced document is no terminal");
    peer.polled();
    peer.selected();
    peer.reply(b"DOM.getAttributes", br#"{"nodeId":7}"#, b"result", br#"{"attributes":[]}"#);
    let Some(Event::Attribute { op: READ, read: Read::NoAttribute }) = peer.browser.events.pop_front() else {
        panic!("the read's answer")
    };
}

#[test]
fn an_element_replaced_past_the_deadline_is_gone() {
    let mut peer = Peer::new();
    peer.selected();
    peer.env.now = peer.env.now.saturating_add(peer.env.limits.answer);
    peer.reply(b"DOM.getAttributes", br#"{"nodeId":7}"#, b"error", MISSING);
    let Some(Event::Refused { op: READ, why: Refusal::Gone }) = peer.browser.events.pop_front() else {
        panic!("the read's refusal")
    };
    assert!(peer.browser.op(READ).is_none());
}
