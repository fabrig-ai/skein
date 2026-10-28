use std::collections::{BTreeMap, VecDeque};

use skein_browser::boundary::{Below, Down, Event, Request};
use skein_browser::wire::decode::Document;
use skein_browser::{Browser, Limits, down, fire, up};
use skein_lib::stream;
use skein_lib::{Env, Queue, Time, Token, Wall};

/// A fault applied to the next reply for a named CDP method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Drop,
    Delay,
    Error,
    Vanish,
    NoBox,
    ContextGone,
    ScopeGone,
    InvalidSelector,
    Oversize,
    Crash,
    PageCrash,
    Interleave,
}

/// An in-memory CDP peer and the browser step machine it drives.
#[derive(Debug)]
pub struct World {
    pub browser: Browser,
    pub env: Env<Limits>,
    pub above: Queue<Event>,
    pub below: Queue<Down>,
    pub sent: Vec<Vec<u8>>,
    pub ax_visible: bool,
    pub selected: u64,
    pub attributes: String,
    pub tree: String,
    pub styles: String,
    pub fonts: BTreeMap<u64, String>,
    pub children: String,
    pub border: String,
    pub metrics: String,
    pub png: String,
    pub loader: String,
    pub sheet: String,
    pub frontend: u64,
    pub gone: bool,
    pub disabled: bool,
    pub covered: bool,
    pub hidden: bool,
    pub hit_points: Vec<(i64, i64)>,
    fault: Option<(Vec<u8>, Fault)>,
    delayed: VecDeque<Vec<u8>>,
    outstanding: BTreeMap<u64, Time>,
    document: u64,
    download_on_press: Option<(u64, bool)>,
    download_serial: u64,
    pub downloads_begun: Vec<(Token, Token, Vec<u8>)>,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    pub fn new() -> World {
        Self::with_limits(Limits::default())
    }

    pub fn with_limits(limits: Limits) -> World {
        World {
            browser: Browser::new(Token::new(1), &limits),
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits },
            above: Queue::uncapped(),
            below: Queue::uncapped(),
            sent: Vec::new(),
            ax_visible: true,
            selected: 7,
            attributes: r#"["value","first","data-note","named","empty",""]"#.into(),
            tree: r#"{"nodes":[{"nodeId":"1","role":{"value":"RootWebArea"},"childIds":["2"]},{"nodeId":"2","ignored":true,"childIds":["3"]},{"nodeId":"3","backendDOMNodeId":7,"role":{"value":"heading"},"name":{"value":"Title"},"value":{"value":"Words"},"properties":[{"name":"level","value":{"value":1}},{"name":"focused","value":{"value":true}},{"name":"disabled","value":{"value":true}},{"name":"checked","value":{"value":true}},{"name":"expanded","value":{"value":true}},{"name":"selected","value":{"value":true}},{"name":"multiline","value":{"value":true}}]}]}"#.into(),
            styles: r#"{"computedStyle":[{"name":"font-size","value":"16px"},{"name":"color","value":"rgb(17, 17, 17)"}]}"#.into(),
            children: r#"[{"nodeId":8,"nodeType":3,"nodeValue":"Fixture"}]"#.into(),
            fonts: [(8, r#"{"fonts":[{"familyName":"Fixture","isCustomFont":true,"glyphCount":5},{"familyName":"Fallback","isCustomFont":false,"glyphCount":2}]}"#.into())].into(),
            border: r#"{"model":{"border":[10.25,20.5,110.75,20.5,110.75,60.9,10.25,60.9]}}"#.into(),
            metrics: r#"{"cssLayoutViewport":{"pageX":0,"pageY":0},"cssVisualViewport":{"pageX":0,"pageY":0,"clientWidth":390,"clientHeight":844}}"#.into(),
            png: "iVBORw0KGgo=".into(),
            loader: "l0".into(),
            sheet: "sh1".into(),
            frontend: 7,
            gone: false,
            disabled: false,
            covered: false,
            hidden: false,
            hit_points: Vec::new(),
            fault: None,
            delayed: VecDeque::new(),
            outstanding: BTreeMap::new(),
            document: 0,
            download_on_press: None,
            download_serial: 0,
            downloads_begun: Vec::new(),
        }
    }

    /// The scenario's button saves a file; the seed chooses before or after its reply,
    /// and whether its progress waits for release_delayed.
    pub fn download_on_press(&mut self, seed: u64, canceled: bool) {
        self.download_on_press = Some((seed, canceled));
    }

    pub fn begin_download(&mut self, guid: &str, name: &str) {
        self.downloads_begun.push((Token::new(2), Token::new(3), name.as_bytes().to_vec()));
        self.notify(format!(r#"{{"method":"Browser.downloadWillBegin","params":{{"frameId":"t1","guid":"{guid}","suggestedFilename":"{name}"}}}}"#));
    }

    pub fn notify(&mut self, json: String) {
        let mut bytes = json.into_bytes();
        bytes.push(0);
        up(
            &mut self.browser,
            &self.env,
            Below::Replies(stream::Up::Bytes(bytes.into())),
            &mut self.above,
            &mut self.below,
        );
        self.drive();
    }

    pub fn fault(&mut self, method: &[u8], fault: Fault) {
        self.fault = Some((method.to_vec(), fault));
    }

    pub fn release_delayed(&mut self) {
        while let Some(reply) = self.delayed.pop_front() {
            self.mark_answered(&reply);
            up(
                &mut self.browser,
                &self.env,
                Below::Replies(stream::Up::Bytes(reply.into_boxed_slice())),
                &mut self.above,
                &mut self.below,
            );
        }
        self.drive();
    }

    pub fn tick(&mut self, now: Time) {
        self.env.now = now;
        fire(&mut self.browser, &self.env, now, &mut self.above, &mut self.below);
        self.outstanding.retain(|_, due| *due > now);
        self.drive();
    }

    /// Every command has received a reply or reached its answer deadline or
    /// the peer's failure. A delayed reply after the deadline stays harmless.
    pub fn assert_commands_settled(&self) {
        assert!(self.outstanding.is_empty(), "unanswered CDP commands: {:?}", self.outstanding);
    }

    fn mark_answered(&mut self, reply: &[u8]) {
        if let Some(document) =
            reply.strip_suffix(&[0]).and_then(|bytes| Document::parse(bytes, self.env.limits.message).ok())
            && let Some(id) = document.root().get(b"id").and_then(|id| id.u64())
        {
            self.outstanding.remove(&id);
        }
    }

    pub fn start(&mut self) {
        fire(&mut self.browser, &self.env, self.env.now, &mut self.above, &mut self.below);
        self.drive();
        assert!(matches!(self.above.pop(), Some(Event::Ready { version }) if version.as_ref() == b"Chrome/153"));
    }

    pub fn ask(&mut self, request: Request) {
        down(&mut self.browser, &self.env, request, &mut self.above, &mut self.below);
        self.drive();
    }

    pub fn drive(&mut self) {
        let mut replies = VecDeque::new();
        for _ in 0..1000 {
            if let Some(record) = self.below.pop() {
                match record {
                    Down::Commands(stream::Down::Demand { .. }) => {
                        up(
                            &mut self.browser,
                            &self.env,
                            Below::Commands(stream::Up::Room),
                            &mut self.above,
                            &mut self.below,
                        );
                    }
                    Down::Commands(stream::Down::Send(bytes)) => {
                        self.sent.push(bytes.to_vec());
                        let document =
                            Document::parse(&bytes[..bytes.len() - 1], self.env.limits.command).expect("valid command");
                        let root = document.root();
                        let id = root.get(b"id").and_then(|id| id.u64()).expect("command id");
                        self.outstanding.insert(id, self.env.now.saturating_add(self.env.limits.answer));
                        let method = root.get(b"method").and_then(|method| method.text()).expect("method");
                        let session = root.get(b"sessionId").and_then(|session| session.text());
                        if method.as_ref() == b"Page.navigate"
                            || method.as_ref() == b"Page.reload"
                            || method.as_ref() == b"Page.navigateToHistoryEntry"
                        {
                            self.document = self.document.checked_add(1).expect("world document id");
                            self.loader = format!("l{}", self.document);
                        }
                        if method.as_ref() == b"DOM.getNodeForLocation" {
                            let params = root.get(b"params").expect("hit test params");
                            let x = params.get(b"x").and_then(|value| value.i64()).expect("hit x");
                            let y = params.get(b"y").and_then(|value| value.i64()).expect("hit y");
                            self.hit_points.push((x, y));
                        }
                        let result = self.reply(&method, root.get(b"params"));
                        let session_field = session
                            .as_deref()
                            .map(|session| format!(",\"sessionId\":\"{}\"", String::from_utf8_lossy(session)))
                            .unwrap_or_default();
                        let reply = format!("{{\"id\":{id}{session_field},\"result\":{result}}}\0").into_bytes();
                        let fault = if self.fault.as_ref().is_some_and(|(target, _)| target == method.as_ref()) {
                            self.fault.take().map(|(_, fault)| fault)
                        } else {
                            None
                        };
                        // As Chromium does, a navigation starts loading before its command is answered.
                        if method.as_ref() == b"Page.navigate"
                            || method.as_ref() == b"Page.reload"
                            || method.as_ref() == b"Page.navigateToHistoryEntry"
                        {
                            replies.push_back(
                                b"{\"method\":\"Page.frameStartedLoading\",\"sessionId\":\"s1\",\"params\":{\"frameId\":\"f1\"}}\0"
                                    .to_vec(),
                            );
                        }
                        if method.as_ref() == b"Input.dispatchMouseEvent"
                            && root
                                .get(b"params")
                                .and_then(|params| params.get(b"type"))
                                .and_then(|kind| kind.text())
                                .as_deref()
                                == Some(b"mouseReleased")
                            && let Some((seed, canceled)) = self.download_on_press.take()
                        {
                            self.download_serial += 1;
                            let guid = format!("download-{}", self.download_serial);
                            let name = format!("recording-{}.json", self.download_serial);
                            self.downloads_begun.push((Token::new(2), Token::new(3), name.as_bytes().to_vec()));
                            let mut begin = format!(r#"{{"method":"Browser.downloadWillBegin","params":{{"frameId":"t1","guid":"{guid}","suggestedFilename":"{name}"}}}}"#).into_bytes();
                            let state = if canceled { "canceled" } else { "completed" };
                            let mut progress = format!(r#"{{"method":"Browser.downloadProgress","params":{{"guid":"{guid}","state":"{state}"}}}}"#).into_bytes();
                            begin.push(0);
                            progress.push(0);
                            if seed & 1 == 0 {
                                replies.push_front(begin);
                            } else {
                                replies.push_back(begin);
                            }
                            if seed & 2 == 0 {
                                replies.push_back(progress);
                            } else {
                                self.delayed.push_back(progress);
                            }
                        }
                        match fault {
                            Some(Fault::Drop) => {}
                            Some(Fault::Delay) => self.delayed.push_back(reply),
                            Some(Fault::InvalidSelector) => replies.push_back(format!("{{\"id\":{id}{session_field},\"error\":{{\"code\":-32000,\"message\":\"DOM Error while querying\"}}}}\0").into_bytes()),
                            Some(Fault::ScopeGone) => replies.push_back(format!("{{\"id\":{id}{session_field},\"error\":{{\"code\":-32000,\"message\":\"Node with given id does not belong to the document\"}}}}\0").into_bytes()),
                            Some(Fault::ContextGone) => replies.push_back(format!("{{\"id\":{id}{session_field},\"error\":{{\"code\":-32000,\"message\":\"Cannot find context with specified id\"}}}}\0").into_bytes()),
                            Some(Fault::Error | Fault::Vanish) => {
                                if fault == Some(Fault::Vanish) { self.gone = true; }
                                replies.push_back(format!("{{\"id\":{id}{session_field},\"error\":{{\"code\":-32000,\"message\":\"No node found for given backend id\"}}}}\0").into_bytes());
                            }
                            Some(Fault::NoBox) => replies.push_back(format!("{{\"id\":{id}{session_field},\"error\":{{\"code\":-32000,\"message\":\"Could not compute box model.\"}}}}\0").into_bytes()),
                            Some(Fault::Oversize) => replies.push_back(vec![b'x'; usize::try_from(self.env.limits.message).expect("message fits usize") + 2]),
                            Some(Fault::Crash) => {
                                up(&mut self.browser, &self.env, Below::Replies(stream::Up::End), &mut self.above, &mut self.below);
                                self.outstanding.clear();
                            }
                            Some(Fault::PageCrash) => {
                                replies.push_back(b"{\"method\":\"Inspector.targetCrashed\",\"sessionId\":\"s1\",\"params\":{}}\0".to_vec());
                            }
                            Some(Fault::Interleave) => {
                                replies.push_back(b"{\"method\":\"Runtime.exceptionThrown\",\"sessionId\":\"s1\",\"params\":{\"exceptionDetails\":{\"text\":\"injected\"}}}\0".to_vec());
                                replies.push_back(reply);
                            }
                            None => replies.push_back(reply),
                        }
                        if method.as_ref() == b"Page.navigate"
                            || method.as_ref() == b"Page.reload"
                            || method.as_ref() == b"Page.navigateToHistoryEntry"
                        {
                            replies.push_back(
                                b"{\"method\":\"Page.loadEventFired\",\"sessionId\":\"s1\",\"params\":{}}\0".to_vec(),
                            );
                        }
                    }
                    Down::Replies(stream::Down::Demand { .. }) | Down::Errors(stream::Down::Demand { .. }) => {}
                    Down::Commands(stream::Down::Finish)
                    | Down::Replies(stream::Down::Finish)
                    | Down::Errors(stream::Down::Finish) => {}
                    Down::Replies(stream::Down::Send(_)) | Down::Errors(stream::Down::Send(_)) => {
                        panic!("browser sends only commands")
                    }
                }
                continue;
            }
            if let Some(reply) = replies.pop_front() {
                self.mark_answered(&reply);
                if reply.len() > usize::try_from(self.env.limits.message).expect("message fits usize") + 1 {
                    self.outstanding.clear();
                }
                if reply.windows(b"Inspector.targetCrashed".len()).any(|part| part == b"Inspector.targetCrashed") {
                    self.outstanding.clear();
                }
                up(
                    &mut self.browser,
                    &self.env,
                    Below::Replies(stream::Up::Bytes(reply.into_boxed_slice())),
                    &mut self.above,
                    &mut self.below,
                );
                if !self.outstanding.is_empty() && self.browser.next_deadline().is_none() {
                    self.outstanding.clear();
                }
                continue;
            }
            if self.browser.work_pending() {
                fire(&mut self.browser, &self.env, self.env.now, &mut self.above, &mut self.below);
                continue;
            }
            return;
        }
        panic!("world did not settle");
    }

    fn reply(&self, method: &[u8], params: Option<skein_browser::wire::decode::Value<'_>>) -> String {
        match method {
            b"Page.getFrameTree" => {
                return format!(r#"{{"frameTree":{{"frame":{{"id":"f1","loaderId":"{}"}}}}}}"#, self.loader);
            }
            b"CSS.createStyleSheet" => return format!(r#"{{"styleSheetId":"{}"}}"#, self.sheet),
            b"CSS.setStyleSheetText" => return "{}".into(),
            b"Accessibility.getFullAXTree" => return self.tree.clone(),
            b"CSS.getComputedStyleForNode" => return self.styles.clone(),
            b"CSS.getPlatformFontsForNode" => {
                let node = params.and_then(|p| p.get(b"nodeId")).and_then(|n| n.u64()).expect("font node");
                return self.fonts.get(&node).cloned().unwrap_or_else(|| r#"{"fonts":[]}"#.into());
            }
            b"DOM.describeNode" if params.is_some_and(|p| p.get(b"depth").and_then(|d| d.i64()) == Some(-1)) => {
                return format!(
                    r#"{{"node":{{"nodeId":{},"backendNodeId":7,"nodeType":1,"children":{}}}}}"#,
                    self.frontend, self.children
                );
            }
            b"DOM.getBoxModel" => return self.border.clone(),
            b"DOM.pushNodesByBackendIdsToFrontend" => {
                let backend = params
                    .and_then(|p| p.get(b"backendNodeIds"))
                    .and_then(|a| a.array())
                    .and_then(|mut a| a.next())
                    .and_then(|n| n.u64())
                    .expect("pushed backend");
                return format!(
                    "{{\"nodeIds\":[{}]}}",
                    if self.gone {
                        0
                    } else if backend == 7 {
                        self.frontend
                    } else {
                        backend
                    }
                );
            }
            b"Page.captureScreenshot" => return format!("{{\"data\":\"{}\"}}", self.png),
            b"Page.getLayoutMetrics" => return self.metrics.clone(),
            _ => {}
        }
        if method == b"DOM.querySelector" {
            return format!("{{\"nodeId\":{}}}", self.selected);
        }
        if method == b"DOM.getAttributes" {
            return format!("{{\"attributes\":{}}}", self.attributes);
        }
        let result = match method {
            b"DOM.getDocument" => r#"{"root":{"nodeId":1}}"#,
            b"Browser.getVersion" => r#"{"product":"Chrome/153"}"#,
            b"Target.createBrowserContext" => r#"{"browserContextId":"ctx1"}"#,
            b"Browser.setDownloadBehavior" | b"Browser.cancelDownload" => "{}",
            b"Target.createTarget" => r#"{"targetId":"t1"}"#,
            b"Target.attachToTarget" => r#"{"sessionId":"s1"}"#,
            b"DOM.resolveNode" => r#"{"object":{"type":"object","objectId":"scope-7"}}"#,
            b"Runtime.evaluate" => r#"{"result":{"type":"object","objectId":"document-1"}}"#,
            b"Accessibility.queryAXTree"
                if self.ax_visible
                    && params.is_some_and(|params| {
                        params.get(b"role").and_then(|role| role.text()).as_deref() == Some(b"button")
                            && params.get(b"accessibleName").is_none_or(|name| name.text().as_deref() == Some(b"Save"))
                    }) =>
            {
                r#"{"nodes":[{"backendDOMNodeId":7,"role":{"value":"button"},"name":{"value":"Save"},"value":{"value":""},"properties":[]}]}"#
            }
            b"Accessibility.queryAXTree" => r#"{"nodes":[]}"#,
            b"Accessibility.getPartialAXTree" if self.disabled => {
                r#"{"nodes":[{"backendDOMNodeId":7,"properties":[{"name":"disabled","value":{"value":true}}]}]}"#
            }
            b"Accessibility.getPartialAXTree" => {
                r#"{"nodes":[{"backendDOMNodeId":7,"properties":[{"name":"disabled","value":{"value":false}}]}]}"#
            }
            b"DOM.getContentQuads" if self.hidden => r#"{"quads":[]}"#,
            b"DOM.getContentQuads" => r#"{"quads":[[10.25,10.25,110.75,10.25,110.75,40.75,10.25,40.75]]}"#,

            b"DOM.getNodeForLocation" if self.covered => r#"{"backendNodeId":99}"#,
            b"DOM.getNodeForLocation" => r#"{"backendNodeId":7}"#,
            b"DOM.describeNode" => r#"{"node":{"backendNodeId":7,"children":[]}}"#,
            b"Page.getNavigationHistory" => {
                r#"{"currentIndex":1,"entries":[{"id":1,"url":"https://example.test/"},{"id":2,"url":"https://example.test/notes/1"},{"id":3,"url":"https://example.test/notes/2"}]}"#
            }

            _ => "{}",
        };
        result.into()
    }

    pub fn open(&mut self) {
        self.open_with_downloads(None);
    }

    pub fn open_with_downloads(&mut self, downloads: Option<Box<[u8]>>) {
        self.ask(Request::Person { person: Token::new(2), downloads });
        self.ask(Request::Page {
            person: Token::new(2),
            page: Token::new(3),
            url: b"https://example.test/".to_vec().into_boxed_slice(),
        });
        assert!(matches!(self.above.pop(), Some(Event::Opened { page }) if page == Token::new(3)));
    }
}
