//! Account admission and removal through owner entrances (oauth.md, section 6.3).

use super::*;
use private::{Owner, private_keeper};
use skein_io::{file, kernel};

#[derive(Debug, PartialEq, Eq)]
struct Sent {
    failed: Option<Failed>,
    finished: private::RefreshOutcome,
}

const ADDED: u32 = 7;
const PATH_BOUND: usize = 4096; // Private paths must be shorter than io's bound.
const NAME_BYTES: usize = 8; // oauth.md, 6.2: a tag and a u32, including wanted.
const STATE_BYTES: usize = 208; // The 192-byte record state plus a 16-byte earliest Time.
const ROOMY_ACCOUNTS: u32 = 3; // One configured account and two added places.
const ADDRESS: core::net::SocketAddr =
    core::net::SocketAddr::new(core::net::IpAddr::V4(core::net::Ipv4Addr::LOCALHOST), 31000);

fn roomy() -> (Component, Env<Limits>) {
    let mut profile = limits();
    profile.accounts = ROOMY_ACCOUNTS;
    let mut configured = List::with_capacity(1);
    assert!(configured.push(Account::HandedIn).is_ok());
    (
        Component::new(configured, &profile, crypto(1)).expect("roomy component"),
        Env { now: Time::ZERO, wall: Wall::EPOCH, limits: profile },
    )
}

#[test]
fn an_added_name_is_refused_while_a_live_account_holds_it() {
    let (mut component, env) = roomy();
    let mut up = Queue::uncapped();
    component.add(&env, ADDED, Account::HandedIn, &mut up);
    component.add(&env, ADDED, Account::HandedIn, &mut up);
    assert_refused(up.pop(), ADDED, Asked::Add, Refusal::Taken);
}

#[test]
fn an_add_past_the_accounts_limit_is_refused_as_full() {
    let (mut component, env) = roomy();
    let mut up = Queue::uncapped();
    for account in [ADDED, ADDED.saturating_add(1)] {
        component.add(&env, account, Account::HandedIn, &mut up);
    }
    component.add(&env, ADDED.saturating_add(2), Account::HandedIn, &mut up);
    assert_refused(up.pop(), ADDED.saturating_add(2), Asked::Add, Refusal::Full { bound: ROOMY_ACCOUNTS });
}

fn bad_plaintext() -> Account {
    let mut configuration = refresh_account(ADDED, ADDRESS);
    if let Account::SignIn { endpoint, .. } = &mut configuration {
        endpoint.address = ([192, 0, 2, 1], 31000).into();
    }
    configuration
}
fn bad_redirect() -> Account {
    let mut configuration = refresh_account(ADDED, ADDRESS);
    if let Account::SignIn { registration, .. } = &mut configuration {
        registration.redirect_uri = bytes::copy_of(b"https://example.com/callback");
    }
    configuration
}
fn bad_registration() -> Account {
    let mut configuration = refresh_account(ADDED, ADDRESS);
    if let Account::SignIn { registration, .. } = &mut configuration {
        registration.client_id = bytes::copy_of(b"");
    }
    configuration
}
fn bad_record() -> Account {
    let mut configuration = refresh_account(ADDED, ADDRESS);
    if let Account::SignIn { keeper, .. } = &mut configuration {
        let mut record = tests_record(0);
        record.access_token = bytes::copy_of(b"");
        *keeper = Keeper::Owner { kept: Some(record) };
    }
    configuration
}
fn bad_path() -> Account {
    let mut configuration = refresh_account(ADDED, ADDRESS);
    if let Account::SignIn { keeper, .. } = &mut configuration {
        *keeper = private_keeper(bytes::zeroed(PATH_BOUND));
    }
    configuration
}

#[test]
fn an_added_configuration_new_refuses_is_refused_as_unusable() {
    for (name, configuration, why) in [
        ("plaintext", bad_plaintext(), Unusable::Plaintext { account: ADDED }),
        ("redirect", bad_redirect(), Unusable::Redirect { account: ADDED }),
        (
            "registration",
            bad_registration(),
            Unusable::Registration { account: ADDED, failure: skein_oauth::Failure::Limit },
        ),
        ("record", bad_record(), Unusable::Record { account: ADDED, why: skein_oauth::DecodeError::Malformed }),
        ("path", bad_path(), Unusable::Path { account: ADDED }),
    ] {
        let (mut component, env) = roomy();
        let mut up = Queue::uncapped();
        component.add(&env, ADDED, configuration, &mut up);
        assert_eq!(refusal_of(up.pop()), Some((ADDED, Asked::Add, Refusal::Unusable(why))), "{name}");
    }
}

#[test]
fn an_added_private_account_starts_its_load_at_the_next_fire() {
    let (mut component, env) = roomy();
    let mut up = Queue::uncapped();
    let mut configuration = refresh_account(ADDED, ADDRESS);
    if let Account::SignIn { keeper, .. } = &mut configuration {
        *keeper = private_keeper(bytes::copy_of(b"secret"));
    }
    component.add(&env, ADDED, configuration, &mut up);
    let mut files = Queue::uncapped();
    component.fire(&env, &mut up, &mut Queue::uncapped(), &mut files);
    match files.pop() {
        Some(FileLower::Request { request: file::Request::OpenPrivate { .. }, .. }) => {}
        Some(FileLower::Request { .. } | FileLower::Cancel { .. } | FileLower::Remove { .. }) | None => {
            panic!("the next fire starts the added private load")
        }
    }
}

fn waiting_sign_in() -> Owner {
    let mut owner = Owner::new();
    owner.ask(Request::SignIn { account: 0, subject: None });
    let load = owner.opened();
    owner.terminal(file::Event::Failed {
        owner: load,
        error: kernel::Error::NotFound,
        committed: false,
        residue: None,
    });
    owner.fire();
    drop(owner.above.pop());
    owner
}
fn held_grant() -> Owner {
    let mut owner = Owner::configured([Account::HandedIn], &limits());
    owner.ask(Request::HandIn { account: 0, record: tests_record(0) });
    owner.ask(Request::Grant { account: 0 });
    drop(owner.above.pop());
    owner
}
fn asked_grant() -> Owner {
    let mut owner = Owner::new();
    owner.ask(Request::Grant { account: 0 });
    owner
}
fn due_refresh() -> Owner {
    let mut owner = Owner::configured([refresh_account(0, ADDRESS)], &limits());
    owner.ask(Request::Grant { account: 0 });
    owner
}

#[test]
fn a_removal_cancels_the_unsent_work_of_its_account() {
    let rows = [
        (
            "sign in",
            waiting_sign_in(),
            Failed { account: 0, ends: Ends::SignIn, failure: Failure::Exchange(skein_oauth::Failure::Cancelled) },
        ),
        (
            "held grant",
            held_grant(),
            Failed { account: 0, ends: Ends::Grant, failure: Failure::Exchange(skein_oauth::Failure::Cancelled) },
        ),
        (
            "asked grant",
            asked_grant(),
            Failed { account: 0, ends: Ends::Grant, failure: Failure::Exchange(skein_oauth::Failure::Cancelled) },
        ),
        (
            "due refresh",
            due_refresh(),
            Failed { account: 0, ends: Ends::Grant, failure: Failure::Exchange(skein_oauth::Failure::Cancelled) },
        ),
    ];
    for (name, mut owner, expected) in rows {
        owner.ask(Request::Remove { account: 0 });
        owner.fire();
        assert_eq!(failure_of(owner.above.pop()), Some(expected), "{name}");
    }
}

#[test]
fn a_removing_account_refuses_its_requests() {
    let mut owner = due_refresh();
    owner.ask(Request::Remove { account: 0 });
    drop(owner.above.pop());
    for (request, asked) in [
        (Request::SignIn { account: 0, subject: None }, Asked::SignIn),
        (
            Request::Redirected { account: 0, uri: bytes::copy_of(b"http://localhost:31234/callback") },
            Asked::Redirected,
        ),
        (Request::HandIn { account: 0, record: tests_record(0) }, Asked::HandIn),
        (Request::Grant { account: 0 }, Asked::Grant),
        (Request::Remove { account: 0 }, Asked::Remove),
    ] {
        owner.ask(request);
        assert_eq!(refusal_of(owner.above.pop()), Some((0, asked, Refusal::Account)));
    }
}

#[test]
fn a_removed_name_is_free_for_add() {
    let mut owner = Owner::configured([Account::HandedIn], &limits());
    owner.ask(Request::Remove { account: 0 });
    owner.fire();
    let removed = owner.above.pop();
    owner.component.add(&owner.env, 0, Account::HandedIn, &mut owner.above);
    owner.ask(Request::HandIn { account: 0, record: tests_record(0) });
    owner.ask(Request::Grant { account: 0 });
    assert!(
        removed == Some(Event::Removed { account: 0 })
            && owner.above.pop()
                == Some(Event::Granted {
                    account: 0,
                    token: tests_record(0).access_token,
                    generation: 0,
                    valid: Duration::from_secs(30)
                })
    );
}

#[test]
fn an_abort_settles_a_removal_before_closed() {
    let mut owner = due_refresh();
    owner.ask(Request::Remove { account: 0 });
    owner.ask(Request::Abort);
    owner.fire();
    let failed = failure_of(owner.above.pop());
    owner.fire();
    let removed = owner.above.pop();
    let closed = owner.above.pop();
    assert!(
        failed
            == Some(Failed {
                account: 0,
                ends: Ends::Grant,
                failure: Failure::Exchange(skein_oauth::Failure::Cancelled)
            })
            && removed == Some(Event::Removed { account: 0 })
            && closed == Some(Event::Closed)
    );
}

fn private_refresh() -> Owner {
    let mut owner = Owner::new();
    owner.ask(Request::Grant { account: 0 });
    let load = owner.opened();
    let record = match refresh_account(0, ADDRESS) {
        Account::SignIn { keeper: Keeper::Owner { kept: Some(record) }, .. } => record,
        Account::SignIn { .. } | Account::HandedIn => unreachable!("refresh fixture record"),
    };
    owner.terminal(file::Event::Loaded {
        owner: load,
        bytes: skein_oauth::encode_record(&record, &owner.env.limits.client.document).expect("kept record"),
    });
    owner
}

#[test]
fn a_removal_lets_a_sent_refresh_and_its_keep_finish() {
    let cancelled =
        Some(Failed { account: 0, ends: Ends::Grant, failure: Failure::Exchange(skein_oauth::Failure::Cancelled) });
    let finished = private::RefreshOutcome { kept: true, removed: 1, granted: false };
    let rows = [
        ("owner", due_refresh(), Sent { failed: cancelled, finished }),
        ("private", private_refresh(), Sent { failed: cancelled, finished }),
    ];
    for (name, mut owner, expected) in rows {
        let identity = owner.sent();
        owner.ask(Request::Remove { account: 0 });
        let failed = failure_of(owner.above.pop());
        let finished = owner.finish_refresh(identity);
        assert_eq!(Sent { failed, finished }, expected, "{name}");
    }
}

#[test]
fn a_removing_number_is_taken_until_its_terminal() {
    let mut owner = due_refresh();
    owner.ask(Request::Remove { account: 0 });
    drop(owner.above.pop());
    owner.component.add(&owner.env, 0, Account::HandedIn, &mut owner.above);
    assert_refused(owner.above.pop(), 0, Asked::Add, Refusal::Taken);
}

#[test]
fn an_add_after_close_is_refused_as_closed() {
    let mut owner = Owner::configured([Account::HandedIn], &limits());
    owner.ask(Request::Close);
    drop(owner.above.pop());
    owner.component.add(&owner.env, ADDED, Account::HandedIn, &mut owner.above);
    assert_refused(owner.above.pop(), ADDED, Asked::Add, Refusal::Closed);
}

#[test]
fn removing_an_unknown_account_is_refused() {
    let mut owner = Owner::configured([Account::HandedIn], &limits());
    owner.ask(Request::Remove { account: ADDED });
    assert_refused(owner.above.pop(), ADDED, Asked::Remove, Refusal::Account);
}

#[test]
fn removing_after_close_is_refused_as_closed() {
    let mut owner = Owner::configured([Account::HandedIn], &limits());
    owner.ask(Request::Close);
    drop(owner.above.pop());
    owner.ask(Request::Remove { account: ADDED });
    assert_refused(owner.above.pop(), ADDED, Asked::Remove, Refusal::Closed);
}

fn refresh_at(expiry: u64, earliest: Option<u64>) -> Account {
    let mut configuration = refresh_account(0, ADDRESS);
    if let Account::SignIn { keeper: Keeper::Owner { kept: Some(record) }, .. } = &mut configuration {
        record.expires_at = Wall::from_nanos(Duration::from_secs(expiry).as_nanos());
        record.earliest_refresh_at = None;
        if let Some(at) = earliest {
            record.earliest_refresh_at = Some(Wall::from_nanos(Duration::from_secs(at).as_nanos()));
        }
    }
    configuration
}
fn at(owner: &mut Owner, seconds: u64) {
    owner.env.now = Time::ZERO.saturating_add(Duration::from_secs(seconds));
    owner.env.wall = Wall::from_nanos(Duration::from_secs(seconds).as_nanos());
}
fn wanted_owner() -> Owner {
    let mut owner = Owner::configured([refresh_at(30, None)], &limits());
    owner.ask(Request::Want { account: 0, wanted: true });
    owner
}
fn wanted_private() -> Owner {
    let mut owner = Owner::new();
    owner.ask(Request::Want { account: 0, wanted: true });
    let load = owner.opened();
    let mut record = tests_record(0);
    record.refresh_token = Some(bytes::copy_of(b"refresh"));
    owner.terminal(file::Event::Loaded {
        owner: load,
        bytes: skein_oauth::encode_record(&record, &owner.env.limits.client.document).expect("kept record"),
    });
    owner
}

#[test]
fn a_wanted_account_is_refreshed_at_its_lead_with_no_grant() {
    let mut owner = wanted_owner();
    at(&mut owner, 20);
    owner.fire();
    owner.fire();
    let connects = match owner.io.pop() {
        Some(Lower::Connect { .. }) => true,
        Some(_) | None => false,
    };
    assert!(connects);
}

#[test]
fn a_wanted_refresh_keeps_its_record_and_lends_nothing() {
    let expected = private::RefreshOutcome { kept: true, removed: 0, granted: false };
    for (name, mut owner, expected) in [("owner", wanted_owner(), expected), ("private", wanted_private(), expected)] {
        at(&mut owner, 20);
        let identity = owner.sent();
        assert_eq!(owner.finish_refresh(identity), expected, "{name}");
    }
}

#[test]
fn a_wanted_refresh_that_fails_says_failed_as_refresh() {
    let mut owner = wanted_owner();
    at(&mut owner, 20);
    owner.fire();
    component::refuse_refresh(&mut owner.component, &owner.env, &mut owner.above, &mut owner.io);
    assert_eq!(
        failure_of(owner.above.pop()),
        Some(Failed { account: 0, ends: Ends::Refresh, failure: Failure::Exchange(skein_oauth::Failure::Refused) })
    );
}

#[test]
fn a_wanted_account_without_a_refreshable_record_is_left_alone() {
    let mut empty = refresh_at(30, None);
    if let Account::SignIn { keeper, .. } = &mut empty {
        *keeper = Keeper::Owner { kept: None };
    }
    let mut handed = Owner::configured([Account::HandedIn], &limits());
    handed.ask(Request::HandIn { account: 0, record: tests_record(0) });
    for (name, mut owner) in [("no record", Owner::configured([empty], &limits())), ("handed in", handed)] {
        owner.ask(Request::Want { account: 0, wanted: true });
        at(&mut owner, 20);
        owner.fire();
        // A handed-in record still announces its ordinary Expiring event.
        let refreshing = !owner.io.is_empty() || owner.component.has_work();
        assert!(!refreshing, "{name}");
    }
}

#[test]
fn a_want_answers_only_when_refused() {
    let mut closed = Owner::configured([Account::HandedIn], &limits());
    closed.ask(Request::Close);
    drop(closed.above.pop());
    for (name, mut owner, account, expected) in [
        ("live", Owner::configured([Account::HandedIn], &limits()), 0, None),
        (
            "free",
            Owner::configured([Account::HandedIn], &limits()),
            ADDED,
            Some((ADDED, Asked::Want, Refusal::Account)),
        ),
        ("closed", closed, 0, Some((0, Asked::Want, Refusal::Closed))),
    ] {
        owner.ask(Request::Want { account, wanted: true });
        assert_eq!(refusal_of(owner.above.pop()), expected, "{name}");
    }
}

struct RefreshInput {
    name: &'static str,
    owner: Owner,
    request: Option<Request>,
}
fn earliest_inputs() -> [RefreshInput; 4] {
    [
        RefreshInput { name: "lead", owner: held_before_earliest(), request: None },
        RefreshInput {
            name: "grant",
            owner: Owner::configured([refresh_at(30, Some(25))], &limits()),
            request: Some(Request::Grant { account: 0 }),
        },
        RefreshInput {
            name: "rejection",
            owner: held_before_earliest(),
            request: Some(Request::Rejected { account: 0, generation: 0 }),
        },
        RefreshInput {
            name: "want",
            owner: Owner::configured([refresh_at(30, Some(25))], &limits()),
            request: Some(Request::Want { account: 0, wanted: true }),
        },
    ]
}
fn held_before_earliest() -> Owner {
    let mut owner = Owner::configured([refresh_at(30, Some(25))], &limits());
    owner.ask(Request::Grant { account: 0 });
    drop(owner.above.pop());
    owner
}
fn ask_at(owner: &mut Owner, request: Option<Request>, seconds: u64) {
    at(owner, seconds);
    if let Some(request) = request {
        owner.ask(request);
    }
    owner.fire();
}

#[test]
fn no_refresh_starts_before_the_earliest_refresh_time() {
    for RefreshInput { name, mut owner, request } in earliest_inputs() {
        ask_at(&mut owner, request, 24);
        assert_eq!((owner.io.is_empty(), owner.component.has_work()), (true, false), "{name}");
    }
}

#[test]
fn a_refresh_starts_at_the_earliest_refresh_time() {
    for RefreshInput { name, mut owner, request } in earliest_inputs() {
        ask_at(&mut owner, request, 25);
        // The account step admits the exchange; the following fire emits its Connect.
        owner.fire();
        let connects = match owner.io.pop() {
            Some(Lower::Connect { .. }) => true,
            Some(_) | None => false,
        };
        assert!(connects, "{name}");
    }
}

#[test]
fn a_grant_before_it_is_answered_with_the_held_token() {
    let mut owner = Owner::configured([refresh_at(30, Some(25))], &limits());
    at(&mut owner, 20);
    owner.ask(Request::Grant { account: 0 });
    assert_granted(owner.above.pop(), 0, 10);
}

#[test]
fn an_expired_grant_waits_for_its_earliest_refresh() {
    let mut owner = Owner::configured([refresh_at(5, Some(25))], &limits());
    at(&mut owner, 10);
    owner.ask(Request::Grant { account: 0 });
    owner.fire();
    assert!(owner.above.is_empty() && owner.io.is_empty());
}

#[test]
fn a_cancelled_wanted_refresh_ends_as_refresh() {
    let mut owner = wanted_owner();
    at(&mut owner, 20);
    owner.fire();
    owner.ask(Request::Cancel { account: 0 });
    assert_eq!(
        failure_of(owner.above.pop()),
        Some(Failed { account: 0, ends: Ends::Refresh, failure: Failure::Exchange(skein_oauth::Failure::Cancelled) })
    );
}

#[test]
fn an_account_no_longer_wanted_has_no_refresh_deadline() {
    let mut owner = wanted_owner();
    owner.ask(Request::Want { account: 0, wanted: false });
    assert_eq!(owner.component.next_deadline(), None);
}

#[test]
fn a_wanted_name_and_earliest_time_fit_the_account_table_bound() {
    assert_eq!((size_of::<component::Name>(), size_of::<component::State>()), (NAME_BYTES, STATE_BYTES));
}

#[test]
fn releasing_a_wanted_grant_keeps_its_rejection_waiting_at_the_earliest_time() {
    let mut owner = Owner::configured([refresh_at(60, Some(25))], &limits());
    owner.ask(Request::Want { account: 0, wanted: true });
    owner.ask(Request::Grant { account: 0 });
    drop(owner.above.pop());
    owner.ask(Request::Rejected { account: 0, generation: 0 });
    owner.ask(Request::Release { account: 0 });
    assert_eq!(owner.component.next_deadline(), Some(Time::from_nanos(25_000_000_000)));
}

#[test]
fn an_account_no_longer_wanted_drops_its_unsent_refresh_deadline() {
    let mut owner = Owner::configured([refresh_at(30, Some(25))], &limits());
    owner.ask(Request::Want { account: 0, wanted: true });
    at(&mut owner, 20);
    owner.fire();
    owner.ask(Request::Want { account: 0, wanted: false });
    at(&mut owner, 25);
    owner.fire();
    assert_eq!(owner.component.next_deadline(), None);
}

#[test]
fn a_free_name_is_added_with_no_answer() {
    let (mut component, env) = roomy();
    let mut up = Queue::uncapped();
    component.add(&env, ADDED, Account::HandedIn, &mut up);
    assert!(up.is_empty());
}
#[test]
fn an_add_at_the_accounts_limit_is_admitted() {
    let (mut component, env) = roomy();
    let mut up = Queue::uncapped();
    for account in [ADDED, ADDED.saturating_add(1)] {
        component.add(&env, account, Account::HandedIn, &mut up);
    }
    assert!(up.is_empty());
}
#[test]
fn a_wanted_account_is_left_alone_before_its_lead() {
    let mut owner = wanted_owner();
    at(&mut owner, 19);
    owner.fire();
    assert!(owner.io.is_empty());
}
