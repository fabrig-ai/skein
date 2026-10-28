//! main's own tests: its configuration from the arguments, and startup's
//! checks of its limits. The loop runs in tests/echo, over the
//! simulator and the real ring.

use std::net::{Ipv4Addr, SocketAddr};
use std::vec;

use crate::{ADDRESS, configure, limits, startup};

fn args(list: &[&str]) -> vec::IntoIter<String> {
    let mut args = Vec::new();
    for arg in list {
        args.push((*arg).to_owned());
    }
    args.into_iter()
}

#[test]
fn the_address_comes_from_the_arguments_or_its_default() {
    let defaults = configure(args(&[])).expect("no arguments is a configuration");
    assert_eq!(defaults.addr, ADDRESS.parse::<SocketAddr>().expect("an address"));
    let given = configure(args(&["127.0.0.1:0"])).expect("address given");
    assert_eq!(given.addr, SocketAddr::from((Ipv4Addr::LOCALHOST, 0)));
}

#[test]
fn arguments_that_do_not_parse_are_refused() {
    assert!(configure(args(&["nowhere"])).is_err(), "not an address");
    assert!(configure(args(&["127.0.0.1:1", "127.0.0.1:2"])).is_err(), "one address");
}

#[test]
fn the_default_limits_pass_startup() {
    let configuration = configure(args(&[])).expect("defaults");
    startup(&configuration).expect("the defaults start");
    assert_eq!(limits().check(), Ok(()));
}

#[test]
fn startup_refuses_limits_that_cannot_run() {
    let configuration = configure(args(&[])).expect("defaults");
    let mut unusable = configuration;
    unusable.limits.io.intake = 16;
    let refused = startup(&unusable).expect_err("the intake cannot hold a line");
    assert!(refused.contains("Read"), "says why: {refused}");
}
