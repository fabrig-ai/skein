//! Pure plans and the private state table; no fake, signing or HTTP stack.
use super::{Purpose, States};
use crate::{
    github::{
        accept::Accept,
        convert::Convert,
        given::{Given, URL_BYTES},
        install::Install,
        manifest::{Manifest, Target},
        paste::Paste,
        reconcile::Reconcile,
    },
    host::run::Machine,
    item,
    reachable::Reachable,
    reconcile::{Event, Need, Reply, Stage, Why},
    tests::{fixtures::secret, github_fixtures as fixtures},
};
use skein_forge::github::{
    document::{self, App, Conversion, Document, Hook, Installation, Row},
    pin::PAGE_CAP,
};
use skein_heap::Meter;
use skein_lib::{List, Queue, Time, Wall, bytes};
#[global_allocator]
static HEAP: skein_heap::Counting = skein_heap::Counting;
const RUNS: u32 = 2;
const STATES: u32 = 4;
const KEY: u32 = 260; // Four-byte DER envelope and 256-byte payload; no RSA parsing.
const NAME: u32 = 34; // Fixture slug limit.
const WORD: u32 = 16; // marker::valid_word admits at most sixteen bytes.
const PAGE: u32 = 16384; // Finite ceiling for the origin and three maximum URLs.
const ACCEPTED: u32 = 8;
const PAGES: u32 = 3; // Two full pages and an empty terminal response.
const STATE: &[u8] = b"0123456789abcdef0123456789abcdef";
const APP: u64 = 19;
const WALL: Wall = Wall::from_nanos(4_000_000_000_000);
fn repeated(n: u32, byte: u8) -> Box<[u8]> {
    {
        let mut data = bytes::zeroed(usize::try_from(n).expect("fixture size"));
        data.fill(byte);
        data
    }
}
fn maximum() -> Given {
    let mut given = fixtures::given();
    given.read_only = false;
    let mut url = repeated(u32::try_from(URL_BYTES).expect("URL bound"), b'x');
    for (slot, byte) in url.iter_mut().zip(b"https://") {
        *slot = *byte;
    }
    given.web = url.clone();
    given.url = url.clone();
    given.redirect_url = url.clone();
    given.setup_url = url;
    given.word = repeated(WORD, b'k');
    given.limits.key_bytes = KEY;
    given.limits.name_bytes = NAME;
    given.limits.page_bytes = PAGE;
    given.limits.installations = ACCEPTED;
    given.limits.reachable = PAGE_CAP;
    given.limits.pages = PAGES;
    given
}
fn der() -> Box<[u8]> {
    let mut key = repeated(KEY, 7);
    for (slot, byte) in key.iter_mut().zip([0x30, 0x82, 1, 0]) {
        *slot = byte;
    }
    key
}
fn ids(count: u32) -> Box<[u64]> {
    let mut ids = List::with_capacity(count);
    for id in 1..=u64::from(count) {
        ids.push(id).expect("fixture capacity");
    }
    ids.into_boxed()
}
fn app() -> App {
    App { id: APP, slug: repeated(NAME, b'k'), permissions: Box::new([]), events: Box::new([]) }
}
fn document(value: Document) -> Event {
    Event::Reply(Reply::GithubDocument(value))
}
fn rows(page: u32) -> Event {
    let mut rows = List::with_capacity(PAGE_CAP);
    for offset in 1..=u64::from(PAGE_CAP) {
        let id = u64::from(page.checked_sub(1).expect("one-based page"))
            .checked_mul(u64::from(PAGE_CAP))
            .expect("fixture page")
            .checked_add(offset)
            .expect("fixture id");
        rows.push(Row::Installation(Installation {
            id,
            login: repeated(NAME, b'a'),
            account: 27,
            kind: document::Account::Organization,
            created: Wall::EPOCH,
            suspended: false,
        }))
        .expect("page capacity");
    }
    Event::Reply(Reply::GithubRows(rows.into_boxed()))
}
struct Counted {
    _target: Target,
    _code: Box<[u8]>,
    machines: List<Machine>,
    states: States,
    pending: Queue<Need>,
    out: Queue<Need>,
    meter: Meter,
    terminals: u32,
}
impl Counted {
    fn new(given: &Given, phase: u8) -> Self {
        let out = Queue::uncapped();
        let target = Target::Org(repeated(NAME, b'a'));
        let code = repeated(u32::try_from(URL_BYTES).expect("URL size"), b'c');
        let meter = Meter::new();
        meter.start();
        let mut states = States::new(STATES);
        for n in 0..STATES {
            let spelling = states
                .issue(
                    given.forge,
                    Purpose::Accept,
                    &[u8::try_from(n).expect("small state"); 16],
                    Time::ZERO,
                    given.limits.state_life,
                    b"install",
                )
                .expect("room");
            drop(spelling);
        }
        let mut machines = List::with_capacity(RUNS);
        for _ in 0..RUNS {
            let machine = match phase {
                0 => Machine::Manifest(Manifest::new(given, &target, STATE).expect("manifest")),
                1 => Machine::Convert(Convert::new(given, &code).expect("convert")),
                2 => Machine::Install(Install::new(given, STATE).expect("install")),
                3 => Machine::Accept(
                    Accept::new(given, core::num::NonZeroU64::new(9).expect("nonzero")).expect("accept"),
                ),
                4 | 6 => Machine::GitHubReconcile(Box::new(Reconcile::new(given, WALL).expect("reconcile"))),
                7 => Machine::Paste(Box::new(
                    Paste::new(given, core::num::NonZeroU64::new(APP).expect("id")).expect("paste"),
                )),
                5 => Machine::Reachable(Reachable::github(given, 1, 1).expect("reachable")),
                _ => panic!("named phase"),
            };
            machines.push(machine).expect("runs capacity");
        }
        let pending = Queue::with_capacity(RUNS);
        let measured = meter.end();
        meter.check(measured, GITHUB_PLANS_PEAK_CEILING, &phase);
        Self { _target: target, _code: code, machines, states, pending, out, meter, terminals: 0 }
    }
    fn go(&mut self, event: &Event) {
        for index in 0..RUNS {
            self.meter.start();
            if self.pending.len() == RUNS {
                drop(self.pending.pop());
            }
            let machine = self.machines.get_mut(index).expect("held run");
            machine.step(event.clone(), &mut self.out);
            let measured = self.meter.end();
            assert!(self.out.len() <= 1);
            if let Some(need) = self.out.pop() {
                match need {
                    need @ Need::Send { .. } => self.pending.push(need),
                    Need::Done(outcome) => {
                        self.terminals = self.terminals.checked_add(1).expect("bounded terminal count");
                        drop(outcome);
                    }
                    Need::Stop(failure) => {
                        assert_eq!(failure.why, Why::Crowded);
                        assert_eq!(failure.step, Stage::Installation);
                        assert_eq!(failure.subject.as_ref(), b"installations");
                        self.terminals = self.terminals.checked_add(1).expect("bounded terminal count");
                    }
                    Need::Read(_) | Need::Draw(_) | Need::Write { .. } => (),
                }
            }
            self.meter.check(measured, GITHUB_PLANS_PEAK_CEILING, &event);
        }
    }
    fn ended(mut self) {
        assert_eq!(self.terminals, RUNS);
        self.go(&Event::Begin);
        self.go(&Event::Reply(Reply::Down));
        assert_eq!(self.terminals, RUNS, "ended machines are silent");
        self.meter.start();
        self.states.clear();
        drop(self.pending);
        let measured = self.meter.end();
        self.meter.check(measured, GITHUB_PLANS_PEAK_CEILING, &"cleared states and pending credentials");
    }
}
fn add(events: &mut List<Event>, event: Event) {
    events.push(event).expect("bounded script");
}
fn script(phase: u8) -> List<Event> {
    // Ten prefix replies, one prune acknowledgment, and at most pages * cap deletes.
    let mut events = List::with_capacity(PAGE_CAP * PAGES + 11);
    add(&mut events, Event::Begin);
    match phase {
        0 => add(&mut events, Event::Drawn(secret(&[1, 2, 3]))),
        1 => {
            add(
                &mut events,
                document(Document::Conversion(Conversion {
                    id: APP,
                    slug: repeated(NAME, b'k'),
                    owner: repeated(NAME, b'a'),
                    owner_id: 23,
                    pem: pasted(),
                    webhook_secret: secret(b"sentinel-webhook"),
                })),
            );
            add(&mut events, document(Document::App(app())));
            add(&mut events, document(Document::BotUser(37)));
            for _ in 0_u32..4 {
                add(&mut events, Event::Written);
            }
        }
        7 => paste_events(&mut events),
        2 => add(&mut events, Event::Read(Some(secret(&repeated(NAME, b'k'))))),
        3 => {
            add(&mut events, Event::Read(Some(secret(&APP.to_le_bytes()))));
            add(&mut events, Event::Read(Some(secret(&der()))));
            add(&mut events, Event::Read(Some(item::accepted_item(&ids(ACCEPTED - 1)))));
            add(
                &mut events,
                document(Document::Installation(Installation {
                    id: 9,
                    account: 27,
                    login: repeated(NAME, b'a'),
                    kind: document::Account::Organization,
                    created: Wall::EPOCH,
                    suspended: false,
                })),
            );
            add(&mut events, Event::Written);
        }
        4 | 6 => {
            add(&mut events, Event::Read(Some(secret(&APP.to_le_bytes()))));
            add(&mut events, Event::Read(Some(secret(&der()))));
            let mut accepted = ids(ACCEPTED);
            if phase == 4 {
                *accepted.last_mut().expect("full accepted") = 999;
            }
            add(&mut events, Event::Read(Some(item::accepted_item(&accepted))));
            add(&mut events, document(Document::App(app())));
            add(&mut events, document(Document::HookConfig(Hook { url: b"https://wrong.test/".as_slice().into() })));
            add(&mut events, document(Document::HookConfig(Hook { url: repeated(NAME, b'k') })));
            add(&mut events, rows(1));
            add(&mut events, rows(2));
            if phase == 6 {
                add(&mut events, rows(3));
                for _ in ACCEPTED..PAGE_CAP * PAGES {
                    add(&mut events, Event::Reply(Reply::Empty));
                }
            } else {
                add(&mut events, Event::Reply(Reply::GithubRows(Box::new([]))));
                add(&mut events, Event::Written);
                for _ in (ACCEPTED - 1)..PAGE_CAP * 2 {
                    add(&mut events, Event::Reply(Reply::Empty));
                }
            }
        }
        5 => {
            add(&mut events, Event::Read(Some(item::accepted_item(&ids(ACCEPTED)))));
            add(&mut events, Event::Read(Some(secret(&APP.to_le_bytes()))));
            add(&mut events, Event::Read(Some(secret(&der()))));
            add(
                &mut events,
                document(Document::Minted(document::Minted {
                    permissions: document::Levels::NONE,
                    token: secret(b"sentinel-installation-token"),
                    expires: WALL,
                })),
            );
            let mut rows = List::with_capacity(PAGE_CAP);
            for id in 1..=PAGE_CAP {
                rows.push(Row::Reachable(document::Reachable {
                    id: u64::from(id),
                    owner: 27,
                    login: repeated(NAME, b'a'),
                    name: repeated(NAME, b'r'),
                    private: true,
                }))
                .expect("page capacity");
            }
            add(&mut events, Event::Reply(Reply::GithubRows(rows.into_boxed())));
        }
        _ => panic!("named phase"),
    }

    events
}
// Bounds the largest peak across all eight pure GitHub plan phases and states.
const GITHUB_PLANS_PEAK_CEILING: u64 = 61_000;

#[test]
fn github_plans_and_states_stay_under_their_peak_ceiling() {
    let given = maximum();
    for phase in 0..8 {
        // Templates belong to the harness and precede the meter's baseline.
        // A metered clone is the delivered reply; output payloads are handed away.
        let events = script(phase);
        let mut held = Counted::new(&given, phase);
        for event in &events {
            held.go(event);
        }
        held.ended();
    }
}
#[test]
fn github_listing_capacity_refuses_unrepresentable_limits() {
    use crate::host::{Host, Kind, limits};
    let limits = crate::tests::host::limits();
    for pages in [u32::MAX / PAGE_CAP, u32::MAX / PAGE_CAP + 1, u32::MAX] {
        let mut settings = crate::tests::host::github_settings();
        for index in 0..settings.forges.len() {
            let forge = settings.forges.get_mut(index).expect("configured forge");
            if let Kind::GitHub(given) = &mut forge.kind {
                given.limits.pages = pages;
            }
        }
        if pages == u32::MAX / PAGE_CAP {
            assert_eq!(limits::check(&limits, &settings), Ok(()));
        } else {
            let refused = limits::check(&limits, &settings).expect_err("unrepresentable listing capacity");
            assert_eq!(Host::new(&limits, settings, crate::tests::host::crypto()).err(), Some(refused));
        }
    }
}

#[test]
fn github_items_and_states_enforce_their_capacities() {
    let given = maximum();
    let key = der();
    assert!(item::app_slug(&repeated(NAME, b'k'), NAME).is_some());
    assert!(item::app_slug(&repeated(NAME + 1, b'k'), NAME).is_none());
    assert!(item::app_key(&key, KEY).is_some());
    assert!(item::app_key(&key, KEY - 1).is_none());
    let ids = ids(ACCEPTED);
    let encoded = item::accepted_item(&ids);
    assert!(item::accepted(encoded.expose(), ACCEPTED).is_some());
    assert!(item::accepted(encoded.expose(), ACCEPTED - 1).is_none());
    let mut states = States::new(STATES);
    let mut first = Box::<[u8]>::default();
    for n in 0..=STATES {
        let value = states
            .issue(
                given.forge,
                Purpose::Accept,
                &[u8::try_from(n).expect("small"); 16],
                Time::ZERO,
                given.limits.state_life,
                b"install",
            )
            .expect("bounded eviction");
        if n == 0 {
            first = value;
        }
    }
    assert!(states.take(given.forge, Purpose::Accept, &first, Time::ZERO).is_err());
}

fn pasted() -> skein_lib::Secret {
    let pem = fixtures::pem(&der(), b"\n", b"");
    let mut padded = repeated(KEY * 2, b' ');
    for (slot, byte) in padded.iter_mut().zip(pem.expose()) {
        *slot = *byte;
    }
    secret(&padded)
}
fn paste_events(events: &mut List<Event>) {
    add(events, Event::Read(Some(pasted())));
    add(events, Event::Read(Some(secret(&APP.to_le_bytes()))));
    add(events, Event::Read(Some(secret(&37u64.to_le_bytes()))));
    add(events, document(Document::App(app())));
    add(events, document(Document::BotUser(37)));
    add(events, document(Document::HookConfig(Hook { url: b"wrong".as_slice().into() })));
    add(events, document(Document::HookConfig(Hook { url: repeated(NAME, b'k') })));
    for _ in 0_u8..4 {
        add(events, Event::Written);
    }
}
