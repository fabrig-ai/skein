//! Paste semantics at the Chromium text-insertion boundary.
use super::look::Css;
use super::{Browser, Page, Phase};
use crate::boundary::{Event, Request};
use crate::{Limits, wire::decode::Document};
use alloc::boxed::Box;
use skein_lib::{Env, Time, Token, Wall};

const PAGE: Token = Token::new(3);
const READ: Token = Token::new(4);

struct Peer {
    browser: Browser,
    env: Env<Limits>,
}
impl Peer {
    fn new(text: &[u8]) -> Self {
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
        browser.request(&env, Request::Type { page: PAGE, op: READ, node: 7, text: Box::from(text) });
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
}

#[test]
fn paste_strips_line_endings_only_in_single_line_inputs() {
    for (node, expected) in [
        (
            br#"{"node":{"nodeName":"INPUT","attributes":["type","password"]}}"#.as_slice(),
            br#"{"text":"firstsecondthird"}"#.as_slice(),
        ),
        (br#"{"node":{"nodeName":"TEXTAREA"}}"#, br#"{"text":"first\nsecond\r\nthird"}"#),
        (br#"{"node":{"nodeName":"DIV"}}"#, br#"{"text":"first\nsecond\r\nthird"}"#),
    ] {
        let mut peer = Peer::new(b"first\nsecond\r\nthird");
        peer.reply(b"DOM.describeNode", br#"{"backendNodeId":7}"#, b"result", node);
        peer.reply(b"DOM.focus", br#"{"backendNodeId":7}"#, b"result", b"{}");
        peer.reply(b"Input.insertText", expected, b"result", b"{}");
        assert!(matches!(peer.browser.events.pop_front(), Some(Event::Done { op: READ })));
    }
}

#[test]
fn a_single_line_value_needs_only_focus_and_insert() {
    let mut peer = Peer::new(b"first second");
    peer.reply(b"DOM.focus", br#"{"backendNodeId":7}"#, b"result", b"{}");
    peer.reply(b"Input.insertText", br#"{"text":"first second"}"#, b"result", b"{}");
    assert!(matches!(peer.browser.events.pop_front(), Some(Event::Done { op: READ })));
}
