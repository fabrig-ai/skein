//! The world's Web Worker: messages in, one timer, transferred messages out (playground.md, 7).
use crate::slices::{Composition, Slices};
use skein_lib::{Time, Wall};
use skein_sim_world::World;
use skein_ui_web::{
    bridge::{Said, Start, ToPage, ToWorld, decode_to_world, encode_to_page},
    platform::worker::{Clock, Speed},
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

thread_local! {
    static LAST_CLOCK: Cell<Clock> = const { Cell::new(Clock {
        step: 0, now: Time::ZERO, wall: Wall::EPOCH, idle: false, due: None,
    }) };
}

type Make<C> = fn(&Start) -> World<<C as Composition>::Process, <C as Composition>::Machine>;

struct Worker<C: Composition> {
    scope: DedicatedWorkerGlobalScope,
    make: Make<C>,
    budget: u32,
    slices: Option<Slices<C>>,
    timer: Option<i32>,
    timeout: Option<Closure<dyn FnMut()>>,
    message: Option<Closure<dyn FnMut(MessageEvent)>>,
    first: bool,
}

/// Install the worker's loop. The first message must be the visit's start.
/// Call once, from the worker module's entry.
pub fn run<C: Composition + 'static>(make: Make<C>, budget: u32) {
    let scope = js_sys::global().dyn_into::<DedicatedWorkerGlobalScope>().expect("dedicated worker");
    // The hook reads a separate cell even while the loop is mutably borrowed.
    std::panic::set_hook(Box::new(|panic| {
        let scope = js_sys::global().unchecked_into::<DedicatedWorkerGlobalScope>();
        let clock = LAST_CLOCK.with(Cell::get);
        post(&scope, &ToPage { clock, said: Said::Failed { words: panic.to_string().into_bytes().into() } });
        core::arch::wasm32::unreachable();
    }));
    let shared = Rc::new(RefCell::new(Worker::<C> {
        scope: scope.clone(),
        make,
        budget,
        slices: None,
        timer: None,
        timeout: None,
        message: None,
        first: true,
    }));
    let timed = Rc::clone(&shared);
    let timeout = Closure::new(move || {
        let mut worker = timed.borrow_mut();
        worker.timer = None;
        let at = instant(&worker.scope);
        let first = std::mem::take(&mut worker.first);
        if let Some(slices) = &mut worker.slices
            && (first || slices.wake(at).is_some_and(|due| due <= at))
        {
            slices.slice(at);
        }
        worker.flush();
        worker.schedule(at);
    });
    shared.borrow_mut().timeout = Some(timeout);
    let messaged = Rc::clone(&shared);
    let message = Closure::new(move |event: MessageEvent| {
        let bytes = event.data().dyn_into::<js_sys::ArrayBuffer>().expect("bridge ArrayBuffer");
        let message = decode_to_world(&js_sys::Uint8Array::new(&bytes).to_vec()).expect("bridge message");
        let mut worker = messaged.borrow_mut();
        let at = instant(&worker.scope);
        if let Some(slices) = &mut worker.slices {
            slices.heard(message, at);
        } else {
            let ToWorld::Start(start) = message else { panic!("start is the first worker message") };
            LAST_CLOCK.with(|clock| clock.set(Clock { wall: start.start, ..clock.get() }));
            let world = (worker.make)(&start);
            worker.slices = Some(Slices::new(world, worker.budget, Speed(1), at));
        }
        worker.schedule(at);
    });
    scope.set_onmessage(Some(message.as_ref().unchecked_ref()));
    // The callbacks and loop live for the worker's lifetime; ending the worker releases them.
    shared.borrow_mut().message = Some(message);
}

impl<C: Composition> Worker<C> {
    fn flush(&mut self) {
        if let Some(slices) = &mut self.slices {
            LAST_CLOCK.with(|clock| clock.set(slices.clock()));
            while let Some(message) = slices.outgoing() {
                post(&self.scope, &message);
            }
        }
    }

    fn schedule(&mut self, at: Time) {
        if let Some(handle) = self.timer.take() {
            self.scope.clear_timeout_with_handle(handle);
        }
        let due = if self.first { Some(at) } else { self.slices.as_ref().and_then(|slices| slices.wake(at)) };
        if let Some(due) = due {
            let millis = due.saturating_since(instant(&self.scope)).as_nanos().div_ceil(1_000_000);
            self.timer = Some(
                self.scope
                    .set_timeout_with_callback_and_timeout_and_arguments_0(
                        self.timeout.as_ref().expect("installed timeout").as_ref().unchecked_ref(),
                        i32::try_from(millis).unwrap_or(i32::MAX),
                    )
                    .expect("worker timeout"),
            );
        }
    }
}

fn post(scope: &DedicatedWorkerGlobalScope, message: &ToPage) {
    let bytes = js_sys::Uint8Array::from(encode_to_page(message).as_ref());
    let buffer = bytes.buffer();
    let transfer = js_sys::Array::new();
    transfer.push(&buffer);
    scope.post_message_with_transfer(&buffer, &transfer).expect("worker postMessage");
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "performance.now is nonnegative milliseconds; the saturating cast bounds nanoseconds"
)]
fn instant(scope: &DedicatedWorkerGlobalScope) -> Time {
    Time::from_nanos((scope.performance().expect("worker performance clock").now().max(0.0) * 1_000_000.0) as u64)
}
