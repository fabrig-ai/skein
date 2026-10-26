mod bridge;
mod fixtures;
mod log;
mod mapping;
mod page;
mod svg;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::inbox::Inbox;

#[test]
fn a_report_raised_in_a_turn_waits_for_that_turn() {
    let inbox: Rc<Inbox<u8>> = Inbox::new();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let calls = Rc::new(Cell::new(0));
    let (output, count) = (Rc::clone(&seen), Rc::clone(&calls));
    inbox.start(Box::new(move |inbox| {
        count.set(count.get() + 1);
        while let Some(report) = inbox.next() {
            if report == 1 {
                inbox.raise(2);
            }
            output.borrow_mut().push(report);
        }
    }));
    inbox.raise(1);
    assert_eq!((seen.borrow().clone(), calls.get()), (vec![1, 2], 1));
}

#[test]
fn a_report_outside_a_turn_calls_it_at_once() {
    let (inbox, seen) = fixtures::collecting();
    inbox.raise(7);
    assert_eq!(*seen.borrow(), vec![7]);
}

#[test]
fn reports_keep_their_order() {
    let inbox = Inbox::new();
    inbox.raise(3);
    inbox.raise(1);
    inbox.raise(2);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let output = Rc::clone(&seen);
    inbox.start(Box::new(move |inbox| {
        while let Some(report) = inbox.next() {
            output.borrow_mut().push(report);
        }
    }));
    assert_eq!(*seen.borrow(), vec![3, 1, 2]);
}

mod fetch;
mod hidden;
mod platform;
mod store;
mod transport;
mod worker;
mod worker_bridge;

mod history;
mod restart;
