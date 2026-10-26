//! The page's browser end of the worker bridge (ui-web.md, 18).
use super::{Bridge, Heard, Limits, Speed};
use crate::{fetch::Report, inbox::Inbox, platform, transport::StreamDuty};
use skein_lib::{Time, Wall};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};

/// The page's platform over a world in a Web Worker (ui-web.md, 18).
pub struct Worker<R> {
    shared: Rc<Shared<R>>,
    document: web_sys::Document,
    visibility: Closure<dyn FnMut(web_sys::Event)>,
    _message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    _error: Closure<dyn FnMut(web_sys::Event)>,
}

/// Ends the world's worker from anywhere, a panic hook among them: it
/// borrows nothing the platform's calls borrow (playground.md, 7).
#[derive(Clone)]
pub struct Stop {
    state: Rc<StopState>,
}

struct StopState {
    worker: Option<web_sys::Worker>,
    ended: Cell<bool>,
    deadline: Cell<Option<Time>>,
    timer: Cell<Option<i32>>,
}

impl Stop {
    /// Idempotent; after it, nothing more is heard, as after `Worker::end`.
    pub fn stop(&self) {
        self.state.end();
    }
}

/// Reads the session log from anywhere, a panic hook among them
/// (playground.md, 6).
#[derive(Clone)]
pub struct Log {
    bridge: Rc<RefCell<Bridge>>,
}

impl Log {
    /// The log as a file, or none while the platform holds a conflicting
    /// bridge borrow: a panic there loses the offer, not the visit's other evidence.
    #[must_use]
    pub fn file(&self) -> Option<Box<[u8]>> {
        self.bridge.try_borrow().ok().map(|bridge| bridge.log())
    }
}

struct Shared<R> {
    bridge: Rc<RefCell<Bridge>>,
    stop: Rc<StopState>,
    inbox: Rc<Inbox<R>>,
    heard: fn(Heard) -> R,
    due: fn() -> R,
    timeout: Closure<dyn FnMut()>,
}

impl<R: 'static> Worker<R> {
    /// Starts the module worker from `worker.js` under the bundle's digest
    /// `directory`, whose last path segment names the session log's build,
    /// and sends the visit's start. The wrap functions give what the page
    /// hears and the application's timer firing to its inbox.
    #[must_use]
    pub fn new(
        directory: &str,
        start: crate::bridge::Start,
        limits: Limits,
        speed: Speed,
        inbox: Rc<Inbox<R>>,
        heard: fn(Heard) -> R,
        due: fn() -> R,
    ) -> Self {
        let options = web_sys::WorkerOptions::new();
        options.set_type(web_sys::WorkerType::Module);
        let directory = directory.trim_end_matches('/');
        let build = directory.rsplit('/').next().expect("a directory has a last segment").as_bytes();
        let address = format!("{directory}/worker.js");
        let worker = web_sys::Worker::new_with_options(&address, &options).ok();
        let document = web_sys::window().expect("browser window").document().expect("browser document");
        let shared = Rc::new_cyclic(|weak: &std::rc::Weak<Shared<R>>| {
            let weak = weak.clone();
            Shared {
                bridge: Rc::new(RefCell::new(Bridge::new(start, build, limits, speed, platform::clocks().0))),
                stop: Rc::new(StopState {
                    worker,
                    ended: Cell::new(false),
                    deadline: Cell::new(None),
                    timer: Cell::new(None),
                }),
                inbox,
                heard,
                due,
                timeout: Closure::new(move || {
                    if let Some(shared) = weak.upgrade() {
                        shared.fire();
                    }
                }),
            }
        });
        let weak = Rc::downgrade(&shared);
        let message = Closure::new(move |event: web_sys::MessageEvent| {
            if let Some(shared) = weak.upgrade() {
                shared.incoming(&event.data());
            }
        });
        let weak = Rc::downgrade(&shared);
        let error = Closure::new(move |event: web_sys::Event| {
            event.prevent_default();
            if let Some(shared) = weak.upgrade() {
                shared.fail(b"The world's worker could not load or stopped with an uncaught error.");
            }
        });
        let weak = Rc::downgrade(&shared);
        let target = document.clone();
        let visibility = Closure::new(move |_: web_sys::Event| {
            if let Some(shared) = weak.upgrade() {
                shared.visibility(target.hidden());
            }
        });
        document
            .add_event_listener_with_callback("visibilitychange", visibility.as_ref().unchecked_ref())
            .expect("visibility listener");
        if let Some(worker) = &shared.stop.worker {
            worker.set_onmessage(Some(message.as_ref().unchecked_ref()));
            worker.set_onerror(Some(error.as_ref().unchecked_ref()));
            shared.visibility(document.hidden());
        } else {
            shared.fail(b"The world's worker could not be started.");
        }
        Self { shared, document, visibility, _message: message, _error: error }
    }

    /// The frame's pace from now on.
    pub fn pace(&self, speed: Speed) {
        if self.shared.stop.ended.get() {
            return;
        }
        self.shared.bridge.borrow_mut().pace(speed, platform::clocks().0);
        self.shared.flush();
        self.shared.set_timer();
    }

    /// The application's clocks, in the world's time at the browser's instant now.
    #[must_use]
    pub fn clocks(&self) -> (Time, Wall) {
        self.shared.bridge.borrow_mut().now(platform::clocks().0)
    }

    /// The browser's facts with the world's instant and wall.
    #[must_use]
    pub fn facts(&self) -> platform::Facts {
        let mut facts = platform::facts();
        (facts.now, facts.wall) = self.clocks();
        facts
    }

    /// The application's world deadline, converted again at every world
    /// message, pace change and visibility change. None cancels the timer.
    pub fn deadline(&self, deadline: Option<Time>) {
        if !self.shared.stop.ended.get() {
            self.shared.stop.deadline.set(deadline);
            self.shared.set_timer();
        }
    }

    /// A handle for ending the worker without accessing the shell or bridge.
    #[must_use]
    pub fn stop(&self) -> Stop {
        Stop { state: Rc::clone(&self.shared.stop) }
    }

    /// The session log as a file, readable after a failure, end or stop.
    #[must_use]
    pub fn log(&self) -> Box<[u8]> {
        self.shared.bridge.borrow().log()
    }

    /// A handle for reading the log without accessing the shell.
    #[must_use]
    pub fn log_handle(&self) -> Log {
        Log { bridge: Rc::clone(&self.shared.bridge) }
    }

    /// Ends the worker at a failure, reset or new story. Nothing is heard after.
    pub fn end(&self) {
        self.shared.stop.end();
    }
}

impl<R: 'static> Shared<R> {
    fn flush(&self) {
        if self.stop.ended.get() {
            return;
        }
        let worker = self.stop.worker.as_ref().expect("live worker");
        loop {
            let bytes = self.bridge.borrow_mut().outgoing();
            let Some(bytes) = bytes else { break };
            let bytes = js_sys::Uint8Array::from(bytes.as_ref());
            let transfer = js_sys::Array::of1(&bytes.buffer());
            if worker.post_message_with_transfer(&bytes, &transfer).is_err() {
                self.fail(b"A message could not be sent to the world's worker.");
                return;
            }
        }
    }

    fn incoming(&self, value: &JsValue) {
        if self.stop.ended.get() {
            return;
        }
        let bytes = if let Some(bytes) = value.dyn_ref::<js_sys::Uint8Array>() {
            bytes.to_vec()
        } else if value.is_instance_of::<js_sys::ArrayBuffer>() {
            js_sys::Uint8Array::new(value).to_vec()
        } else {
            self.fail(b"The world's worker sent a message that was not bytes.");
            return;
        };
        let heard = self.bridge.borrow_mut().incoming(&bytes, platform::clocks().0);
        match heard {
            Ok(Heard::Nothing) => self.set_timer(),
            Ok(Heard::Failed { words }) => {
                self.stop.end();
                self.inbox.raise((self.heard)(Heard::Failed { words }));
            }
            Ok(heard) => {
                self.set_timer();
                self.inbox.raise((self.heard)(heard));
            }
            Err(crate::bridge::Malformed) => self.fail(b"The world's worker sent a malformed bridge message."),
        }
    }

    fn fail(&self, words: &[u8]) {
        if !self.stop.ended.get() {
            self.stop.end();
            self.inbox.raise((self.heard)(Heard::Failed { words: words.into() }));
        }
    }

    fn report(&self, report: Option<Report>) {
        self.flush();
        if !self.stop.ended.get()
            && let Some(report) = report
        {
            self.inbox.raise((self.heard)(Heard::Report(report)));
        }
    }

    fn visibility(&self, hidden: bool) {
        if self.stop.ended.get() {
            return;
        }
        self.bridge.borrow_mut().visibility(hidden, platform::clocks().0);
        self.flush();
        self.set_timer();
    }

    fn set_timer(&self) {
        self.stop.clear_timer();
        if self.stop.ended.get() {
            return;
        }
        let at = self.stop.deadline.get().and_then(|deadline| self.bridge.borrow().timer(deadline));
        if let Some(at) = at {
            let nanos = at.saturating_since(platform::clocks().0).as_nanos();
            let millis = i32::try_from(nanos.div_ceil(1_000_000)).unwrap_or(i32::MAX);
            self.stop.timer.set(Some(
                web_sys::window()
                    .expect("browser window")
                    .set_timeout_with_callback_and_timeout_and_arguments_0(
                        self.timeout.as_ref().unchecked_ref(),
                        millis,
                    )
                    .expect("world deadline"),
            ));
        }
    }

    fn fire(&self) {
        self.stop.timer.set(None);
        if self.stop.ended.get() {
            return;
        }
        let at = self.stop.deadline.get().and_then(|deadline| self.bridge.borrow().timer(deadline));
        if let Some(at) = at {
            if at > platform::clocks().0 {
                self.set_timer();
                return;
            }
            self.stop.deadline.set(None);
            self.inbox.raise((self.due)());
        }
    }
}

impl StopState {
    fn clear_timer(&self) {
        if let Some(timer) = self.timer.take() {
            web_sys::window().expect("browser window").clear_timeout_with_handle(timer);
        }
    }

    fn end(&self) {
        if self.ended.replace(true) {
            return;
        }
        self.deadline.set(None);
        self.clear_timer();
        if let Some(worker) = &self.worker {
            worker.set_onmessage(None);
            worker.set_onerror(None);
            worker.terminate();
        }
    }
}

impl<R: 'static> platform::Platform for Worker<R> {
    fn read_all(&self) -> Vec<crate::store::Record> {
        self.shared.bridge.borrow_mut().store().read_all()
    }

    fn write(&mut self, key: &[u8], value: &[u8]) -> bool {
        self.shared.bridge.borrow_mut().store().write(key, value)
    }

    fn remove(&mut self, key: &[u8]) {
        self.shared.bridge.borrow_mut().store().remove(key);
    }

    fn send(&mut self, exchange: &crate::fetch::Exchange) {
        if !self.shared.stop.ended.get() {
            let report = self.shared.bridge.borrow_mut().send(exchange);
            self.shared.report(report);
        }
    }

    fn abandon(&mut self, exchange: u32) {
        if !self.shared.stop.ended.get() {
            self.shared.bridge.borrow_mut().abandon(exchange);
            self.shared.flush();
        }
    }

    fn read_on(&mut self) {
        if !self.shared.stop.ended.get() {
            let report = self.shared.bridge.borrow_mut().read_on();
            self.shared.report(report);
        }
    }

    fn navigate(&mut self, to: &[u8]) {
        if !self.shared.stop.ended.get() && super::navigable(to) {
            let to = std::str::from_utf8(to).expect("visible ASCII path");
            web_sys::window()
                .expect("browser window")
                .history()
                .expect("browser history")
                .push_state_with_url(&JsValue::NULL, "", Some(to))
                .expect("same origin path");
        }
    }

    fn stream(&mut self, duty: StreamDuty) {
        if !self.shared.stop.ended.get() {
            let report = self.shared.bridge.borrow_mut().stream(duty);
            self.shared.report(report);
        }
    }
}

impl<R> Drop for Worker<R> {
    fn drop(&mut self) {
        self.shared.stop.end();
        let _removed = self
            .document
            .remove_event_listener_with_callback("visibilitychange", self.visibility.as_ref().unchecked_ref());
    }
}
