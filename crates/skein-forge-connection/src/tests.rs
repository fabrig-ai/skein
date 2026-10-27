mod artifact;
mod connection;
mod contents;
mod dialects;
pub(crate) mod fixtures;
mod free_rate;
mod github_calls;
mod github_files;
mod github_installation_loss;
mod github_installations;
mod github_logs;
pub(crate) mod github_renewal;
mod github_start;
mod github_unknown_repository;
mod github_writes_heap;
mod hooks;
mod puller;
mod reads;
mod writes;
use crate::{
    Component, StartRead, Stopped,
    start::{self, Start},
};
use alloc::boxed::Box;
use core::net::{Ipv4Addr, SocketAddr};
use skein_lib::{Duration, Time};

#[global_allocator]
static HEAP: skein_heap::Counting = skein_heap::Counting;

#[test]
fn the_start_stops_on_another_user_or_a_4xx_and_waits_otherwise() {
    let limits = fixtures::limits();
    let now = Time::from_nanos(1_000_000_000);
    let checking = Start::Checking { next: limits.start_backoff };
    let settings = Start::Settings { next: limits.start_backoff };
    let again = Start::Waiting { until: Time::from_nanos(2_000_000_000), next: Duration::from_secs(2) };
    let rows = [
        ("configured user", checking, Heard::User { id: fixtures::WRITER }, settings),
        ("another user", checking, Heard::User { id: 1 }, Start::Stopped(Stopped::OtherUser { user: 1 })),
        (
            "refused token",
            checking,
            Heard::Status { status: 401, retry: None },
            Start::Stopped(Stopped::Refused { read: StartRead::User, status: 401 }),
        ),
        (
            "no API",
            settings,
            Heard::Status { status: 404, retry: None },
            Start::Stopped(Stopped::Refused { read: StartRead::ApiSettings, status: 404 }),
        ),
        ("page cap", settings, Heard::ApiSettings { max_response_items: 2 }, Start::Ready { cap: 2 }),
        ("5xx", checking, Heard::Status { status: 503, retry: None }, again),
        (
            "429",
            checking,
            Heard::Status { status: 429, retry: Some(Duration::from_secs(5)) },
            Start::Waiting { until: Time::from_nanos(6_000_000_000), next: Duration::from_secs(2) },
        ),
        ("unreachable", checking, Heard::Unanswered, again),
        ("empty cap", settings, Heard::ApiSettings { max_response_items: 0 }, again),
        ("wrong document", settings, Heard::User { id: fixtures::WRITER }, again),
        ("redirect", checking, Heard::Status { status: 301, retry: None }, again),
        ("ready stays ready", Start::Ready { cap: 2 }, Heard::Unanswered, Start::Ready { cap: 2 }),
    ];
    for (name, state, heard, expected) in rows {
        assert_eq!(start::heard(state, heard, fixtures::WRITER, now, &limits), expected, "{name}");
    }
}

use crate::{
    Answered, Event, Limits, Request, Unusable,
    limits::{self, Listing},
};
use skein_forge::{api, forgejo::plans::start::Heard};
use skein_lib::{Secret, Token};

#[test]
fn limits_that_disagree_with_the_client_name_both_limits() {
    let l = fixtures::limits();
    let s = fixtures::shared();
    let short_answer = crate::Shared { answer_bytes: fixtures::ROW_ITEMS - 1, ..s };
    let rows = [
        ("connections", Limits { connections: 3, ..l }, s, Unusable::Connections { connections: 3, calls: 2 }),
        (
            "deadline",
            Limits { deadline: Duration::from_secs(11), ..l },
            s,
            Unusable::Deadline { deadline: Duration::from_secs(11), lifetime: Duration::from_secs(10) },
        ),
        ("body", Limits { body_bytes: 1041, ..l }, s, Unusable::BodyBytes { body_bytes: 1041, needed: 1042 }),
        ("name", Limits { string_bytes: 127, ..l }, s, Unusable::StringBytes { string_bytes: 127, name_bytes: 128 }),
        ("repositories", Limits { repositories: 3, ..l }, s, Unusable::Repositories { repositories: 3, forge: 2 }),
        ("answer", l, short_answer, Unusable::PageSize { listing: Listing::Items }),
    ];
    for (name, limits, shared, expected) in rows {
        assert_eq!(limits::check(&limits, &shared, &fixtures::settings()), Err(expected), "{name}");
    }
    assert_eq!(limits::check(&l, &s, &fixtures::settings()), Ok(()), "fixture agrees");
}

#[test]
fn a_page_is_the_smallest_of_rows_cap_and_answer_room() {
    let limits = fixtures::limits();
    let shared = fixtures::shared();
    let rows =
        [("forge cap", 2, 65_536, 2), ("client rows", 50, 65_536, 8), ("answer room", 50, fixtures::ROW_ITEMS, 1)];
    for (name, cap, answer_bytes, expected) in rows {
        let shared = crate::Shared { answer_bytes, ..shared };
        assert_eq!(limits::page_size(Listing::Items, cap, &limits, &shared), expected, "{name}");
    }
}

#[test]
fn a_call_waiting_for_its_forge_is_unavailable_at_its_deadline() {
    let mut h = fixtures::Harness::new(fixtures::settings());
    assert_eq!(h.start(fixtures::FORGE), Start::Checking { next: Duration::from_secs(1) }, "start checking");
    h.call(1, fixtures::REPO, api::Op::Read(api::Read::Settings));
    assert_eq!(h.component.next_deadline(), Some(Time::from_nanos(10_000_000_000)), "arrival plus deadline");
    h.run(Duration::from_secs(9));
    assert_eq!(h.answered(1), None, "before deadline");
    h.run(Duration::from_secs(1));
    assert_eq!(
        h.answered(1),
        Some(&Answered { call: Token::new(1), cost: 0, free: 0, rate: None, result: Err(api::Error::Unavailable) }),
        "at deadline"
    );
    h.run(Duration::from_secs(1));
    assert_eq!(h.answers.len(), 1, "answered once");
    assert_eq!(h.component.next_deadline(), Some(Time::from_nanos(21_000_000_000)), "only the new start waits");
    assert_eq!(h.io.len(), 0, "nothing sent");
}

#[test]
fn refused_answers_are_offered_once_and_returned_answers_stay_until_close() {
    let mut h = fixtures::Harness::new(fixtures::settings());
    let answer = Answered { call: Token::new(1), cost: 0, free: 0, rate: None, result: Err(api::Error::Unavailable) };
    h.send(Request::Refused(answer.clone()));
    h.send(Request::Offer);
    let Some(Event::Answered(offered)) = h.up.pop() else {
        unreachable!("one offered answer");
    };
    assert_eq!(offered, answer, "same answer offered");
    h.send(Request::Returned(offered));
    h.send(Request::Offer);
    assert_eq!(h.up.len(), 0, "returned answer never offered");
    h.send(Request::Close);
    assert_eq!(h.up.len(), 0, "connections close before the terminal");
    h.run(Duration::ZERO);
    assert!(h.closed, "closed emitted after io closed");
    h.send(Request::Offer);
    h.send(Request::Close);
    assert_eq!(h.up.len(), 0, "nothing after closed");
}

#[test]
fn closing_waiting_calls_emits_only_the_close_terminal() {
    let mut h = fixtures::Harness::new(fixtures::settings());
    h.call(1, fixtures::REPO, api::Op::Read(api::Read::Settings));
    h.send(Request::Close);
    assert_eq!(h.up.len(), 0, "close waits for io");
    h.run(Duration::ZERO);
    assert!(h.closed, "close terminal after last socket");
    h.run(Duration::from_secs(20));
    assert_eq!(h.answered(1), None, "no call answer after root stopped");
    assert_eq!(h.component.next_deadline(), None, "no live deadlines");
}

#[test]
fn unusable_forge_settings_name_the_forge() {
    forge_setting_boundaries();
    forge_count_token_and_hook_boundaries();
    plaintext_name_boundaries();
}

fn forge_setting_boundaries() {
    let loopback = SocketAddr::from((Ipv4Addr::LOCALHOST, 3000));
    let remote = SocketAddr::from((Ipv4Addr::new(192, 0, 2, 1), 3000));
    let rows = [
        ("empty token", skein_io::Addr::Ip(loopback), b"".as_slice(), 2, Unusable::Token { forge: 1 }),
        ("zero user", skein_io::Addr::Ip(loopback), fixtures::TOKEN, 0, Unusable::User { forge: 1 }),
        ("remote plaintext", skein_io::Addr::Ip(remote), fixtures::TOKEN, 2, Unusable::Address { forge: 1 }),
        (
            "in process",
            skein_io::Addr::InProcess(skein_io::Name::EMPTY),
            fixtures::TOKEN,
            2,
            Unusable::Address { forge: 1 },
        ),
    ];
    for (name, address, token, user, expected) in rows {
        let mut settings = fixtures::settings();
        let forge = settings.forges.get_mut(0).expect("one forge");
        forge.address = address;
        forge.access.as_mut().expect("fixture access").credential =
            skein_forge::credential::Credential::Token(Secret::new(Box::from(token)));
        forge.access.as_mut().expect("fixture access").user = user;
        assert_eq!(limits::check(&fixtures::limits(), &fixtures::shared(), &settings), Err(expected), "{name}");
    }
    for (label, access, expected) in [
        ("no access", None, Ok(())),
        (
            "App credential",
            Some(crate::Access {
                credential: skein_forge::credential::Credential::App { id: 1, key: Secret::new(Box::new([])) },
                user: fixtures::WRITER,
                webhook_secret: None,
            }),
            Err(Unusable::Credential { forge: fixtures::FORGE }),
        ),
    ] {
        let mut settings = fixtures::settings();
        settings.forges.get_mut(0).expect("forge").access = access;
        fixtures::admission(settings, &fixtures::limits(), expected, label.as_bytes());
    }
    let mut settings = fixtures::settings();
    settings.forges.get_mut(0).expect("forge").address = skein_io::Addr::Ip(([0, 0, 0, 0, 0, 0, 0, 1], 3000).into());
    fixtures::admission(settings, &fixtures::limits(), Ok(()), b"IPv6 loopback");
}

#[expect(clippy::manual_map, reason = "step fixtures use explicit matches without closures")]
fn plaintext_name_boundaries() {
    for (name, expected) in [
        (None, Ok(())),
        (Some(b"".as_slice()), Err(Unusable::PlaintextName { forge: fixtures::FORGE })),
        (Some(b"\0"), Err(Unusable::PlaintextName { forge: fixtures::FORGE })),
        (Some(b"\t"), Err(Unusable::PlaintextName { forge: fixtures::FORGE })),
        (Some(b"\x1f"), Err(Unusable::PlaintextName { forge: fixtures::FORGE })),
        (Some(b" "), Err(Unusable::PlaintextName { forge: fixtures::FORGE })),
        (Some(b"\x7f"), Err(Unusable::PlaintextName { forge: fixtures::FORGE })),
        (Some(b"\x80"), Err(Unusable::PlaintextName { forge: fixtures::FORGE })),
        (Some(b"\xff"), Err(Unusable::PlaintextName { forge: fixtures::FORGE })),
        (Some(b"/"), Err(Unusable::PlaintextName { forge: fixtures::FORGE })),
        (Some(b":"), Err(Unusable::PlaintextName { forge: fixtures::FORGE })),
        (Some(b"["), Err(Unusable::PlaintextName { forge: fixtures::FORGE })),
        (Some(b"]"), Err(Unusable::PlaintextName { forge: fixtures::FORGE })),
        (Some(b"!"), Ok(())),
        (Some(b"~"), Ok(())),
        (Some(b"."), Ok(())),
        (Some(b"0"), Ok(())),
        (Some(b"9"), Ok(())),
        (Some(b";"), Ok(())),
        (Some(b"Z"), Ok(())),
        (Some(b"\\"), Ok(())),
        (Some(b"^"), Ok(())),
    ] {
        let mut settings = fixtures::settings();
        settings.forges.get_mut(0).expect("one forge").transport = crate::settings::Transport::Plaintext {
            name: match name {
                Some(name) => Some(Box::from(name)),
                None => None,
            },
        };
        fixtures::admission(settings, &fixtures::limits(), expected, name.unwrap_or(b"unnamed"));
    }
}

#[test]
fn zero_documents_and_stacked_demands_are_refused_in_order() {
    numeric_boundaries();
    neighbour_boundaries();
    stack_boundaries();
    let l = fixtures::limits();
    let s = fixtures::shared();
    let rows = [
        ("zero chunk", Limits { chunk: 0, ..l }, s, Unusable::Zero),
        (
            "document depth",
            Limits { depth: 7, ..l },
            s,
            Unusable::Documents(skein_forge::forgejo::limits::Problem::Depth),
        ),
        (
            "HTTP over TLS",
            Limits { head_bytes: 16_385, ..l },
            s,
            Unusable::Stack { stacked: limits::Stacked::HttpOverTls, demand: 16_385, cap: 16_384 },
        ),
        (
            "TLS over io",
            l,
            crate::Shared { io: skein_io::Limits { intake: 18_431, ..s.io }, ..s },
            Unusable::Stack { stacked: limits::Stacked::TlsOverIo, demand: 18_432, cap: 18_431 },
        ),
    ];
    for (name, limits, shared, expected) in rows {
        assert_eq!(limits::check(&limits, &shared, &fixtures::settings()), Err(expected), "{name}");
    }
}

#[test]
fn duplicate_forge_numbers_are_refused_before_any_connection() {
    let mut settings = fixtures::settings();
    let extra = fixtures::settings().forges.into_boxed();
    for forge in extra {
        settings.forges.push(forge).expect("two fixture forges fit");
    }
    assert_eq!(
        limits::check(&fixtures::limits(), &fixtures::shared(), &settings),
        Err(Unusable::Duplicate { forge: 1 }),
        "duplicate forge"
    );
}

// Complete Forgejo fixture (forge-connection.md, 7.5), in bytes on the
// 64-bit layout. Decoder = 32*128 + 8*(3*128 + 2*48 + 2*16)
// + 2*1042 + 16*128 + 4096 + fixed decoder 312 = 16_732.
// Socket = 32768 + 4096 + 32768 + 4*16 = 69_696.
// Each of two pooled connections = HTTP 44_032 + TLS 456_809 + tokenizer
// 2082 + decoder 16_732 + raw 2*65536 + routing 5056 + send 32768
// + encoding (6*1042 + 1024) + socket 69_696
// + local JSON intake/delivery 2*1024 = 767_571.
// Four hooks = 4*(HTTP 18_418 + tokenizer 2082 + decoder 16_732
// + socket 69_696 + HMAC 208 + hint 88 + text/body 1170 + queues 368
// + head 8192 + chunks 2048) + hook slab/index 4832 = 480_840.
// Common calls = 2*560 (Waiting slab slots) + 288 (holders tree)
// + 2*16 (drive list) + 288 (call index) + 2*3*512 (operation copies)
// + 2752 (held tree) + 2*65536 (held answers) = 138_624.
// Forgejo's call payload = read/write boxes 1896+456, staging 64*8
// + 72*16 + 72*128, answer 65536, largest read reserve 3008,
// texts 4*128 + 13*128 and users 8*48, commit-page reserve 8*64 = 84_848.
// Read reserve = ChangedFile/File/Commit rows and paths 8*(40+48+32+2*128).
// These fixed row containers also cover Run, RunJob and Entry capacities;
// turn text shares the answer envelope. Artifact's archive and inflation use
// the connection's two raw envelopes; its output shares the retained answer.
// Tables = repository tree 904 + 2*3*128 + user tree 1104 + 8*128 = 3800.
// Configured lists = 2*(Forge 168 + Start 24 + optional GitHub box 8
// + optional token 16 + optional OwnPlan 360) = 1152. Each forge's
// common credential/secret/send/TLS = 64 + 128 + 32768 + 456_809 = 489_769.
// Owner/framing/answer = 2*16 + (3+9) + 16 + Answered 224 = 284.
// Total = 2*767571 + connection index 288 + slab 2*1504 + 480840
// + 138624 + 2*84848 + 3800 + 1152 + 489769 + 284 = 2_822_603.
const FIXTURE_HEAP: u64 = 2_822_603;

// Complete GitHub fixture: App/document overlap needs 592 per inner entry.
// Decoder = 32*128 + 8*592 + 2*1042 + 16*128 + 4096 + 39+100
// + fixed decoder 2400 = 19_599. Each connection holds a 136-byte boxed
// request/retention/comparison exchange. Three connections are 770_574 each;
// four hooks are 4*121869 + slab/index 4832 = 492_308.
// Installation storage = live Row tree 2136 + metadata 2*552
// + accepted accounts 2*552 + missing accounts 464 + id sets 3*200
// + id lists 2*3*8 + installation pages 2*100*48 + 2*100*128
// + row accounts/tokens 3*(6*128+64) + token handoffs 7*64
// + Event 224 + cursor/event accounts 3*128 = 44_208.
// Log origins = 2*64 + 2*24 + 2*(253+1024) = 2730.
// GitHub inline state 776 + key/JWT 1200+512 + installation storage
// + log origins = 49_426 per GitHub forge. Tables are 3888;
// discovery's one-row staging tables are 2056.
// Call payload = probe/original read boxes 2*1512 + write box 512
// + staging 2056 + revision-bearing candidates 3*24 + 3*65536 answer
// envelopes + largest read reserve 3008 + reviewers 400 + ruleset ids 8*8
// + row/request/login/id text 11*128 + marked body 1042 + refusal header
// 2*128+1024 + write prerequisites (login slots 8*16, reviewer ids 8*8,
// pull/GraphQL/error text 6*128) = 210_434.
// Puller = page/header handoffs 2*(8*(104+2*128)+2*128) + validators 4*128
// + LogPage/LogDelivery boxes 416+336 + pushed decoder 304
// + common decoder 19599 + row event copies 2*128
// + held hint/text/body 88+128+1042 = 28_953.
// Total = 3*770574 + index 288 + slab 3*1504 + 492308 + 138624
// + 2*210434 + 3888 + 1152 + 489769 + 49426 + 284 + 28953 = 3_941_794.
const GITHUB_FIXTURE_HEAP: u64 = 3_941_794;

// At calls 2 -> 3 the B-tree node counts stay fixed. A slot adds Waiting's
// slab 560 + drive-list pair 16 + 3*512 operation copies + 65536 held
// answer bytes, plus the selected dialect's payload from the ledgers above.
const FORGEJO_CALL_SLOT: u64 = 152_496; // 560 + 16 + 1536 + 65536 + 84848.
const GITHUB_CALL_SLOT: u64 = 278_082; // 560 + 16 + 1536 + 65536 + 210434.
const FORGEJO: limits::Forges = limits::Forges { forgejo: 1, github: 0 };
const GITHUB: limits::Forges = limits::Forges { forgejo: 0, github: 1 };
const MIXED: limits::Forges = limits::Forges { forgejo: 1, github: 1 };
const NO_FORGES: limits::Forges = limits::Forges { forgejo: 0, github: 0 };

fn fixture_bound(forges: limits::Forges) -> u64 {
    limits::worst_case(
        &fixtures::limits(),
        &fixtures::shared(),
        forges,
        crate::settings::owned_bytes(&fixtures::settings()).expect("owner bytes"),
    )
    .expect("fixture bound")
}

#[test]
fn a_forgejo_forge_pays_no_github_share() {
    assert_eq!(fixture_bound(FORGEJO), FIXTURE_HEAP);
    assert!(fixture_bound(FORGEJO) < fixture_bound(GITHUB));
    let l = fixtures::limits();
    let larger = Limits {
        key_bytes: u32::MAX,
        installations: u32::MAX,
        log_origins: u32::MAX,
        delivery_bytes: u32::MAX,
        log_pages: u32::MAX,
        ..l
    };
    assert_eq!(
        limits::worst_case(&l, &fixtures::shared(), FORGEJO, 0),
        limits::worst_case(&larger, &fixtures::shared(), FORGEJO, 0),
    );
}

#[test]
fn a_github_forge_pays_its_app_share() {
    assert_eq!(size_of::<skein_forge::github::decode::Decoder>(), 2400);
    assert_eq!(size_of::<skein_forge::github::plans::reads::Plan>(), 1512);
    assert_eq!(limits::github_call_heap(&fixtures::limits(), &fixtures::shared()), Some(210_434));
    assert_eq!(size_of::<crate::installations::GitHub>(), 776);
    assert_eq!(limits::puller_heap(&fixtures::limits()), Some(28_953));
    assert_eq!(3_912_841_u64.checked_add(28_953), Some(GITHUB_FIXTURE_HEAP));
    let settings = fixtures::github_settings();
    assert_eq!(limits::Forges::of(&settings), Some(GITHUB));
    assert_eq!(
        limits::worst_case(
            &fixtures::limits(),
            &fixtures::shared(),
            GITHUB,
            crate::settings::owned_bytes(&settings).expect("owner bytes"),
        ),
        Some(GITHUB_FIXTURE_HEAP),
    );
}

#[test]
fn a_github_slot_prices_its_files_state() {
    use skein_forge::{github, measure};
    use skein_lib::{List, Queue};
    let l = fixtures::limits();
    let s = fixtures::shared();
    let files = s.rows.min(
        s.answer_bytes
            .checked_div(u32::try_from(measure::FILE_BYTES).expect("file width"))
            .expect("nonzero file width"),
    );
    let commits = s.rows.min(
        s.answer_bytes
            .checked_div(u32::try_from(measure::COMMIT_BYTES).expect("commit width"))
            .expect("nonzero commit width"),
    );
    // files_admission reserves these capacities before page_size limits requests.
    let containers = Queue::<github::document::File>::worst_case(files)
        .expect("descriptors")
        .checked_add(List::<api::File>::worst_case(files).expect("files"))
        .expect("files storage")
        .checked_add(List::<api::Commit>::worst_case(commits).expect("commits"))
        .expect("all containers");
    let names = u64::from(files)
        .checked_mul(2)
        .expect("two names")
        .checked_mul(u64::from(l.string_bytes))
        .expect("bounded names")
        .min(u64::from(s.answer_bytes));
    let slot = limits::github_call_heap(&l, &s).expect("GitHub slot");
    assert!(slot >= containers.checked_add(names).expect("file state"));
    // Isolate the reserve from the fixed plans and answer envelopes: this catches
    // a missing files term even when the fixture's answer budget dwarfs its rows.
    let empty = limits::github_call_heap(&l, &crate::Shared { rows: 0, ..s }).expect("empty reserve");
    assert!(slot.checked_sub(empty).expect("row growth") >= containers + names);
}

#[test]
fn a_call_slot_holds_one_dialect_at_a_time() {
    assert!(fixture_bound(MIXED) < fixture_bound(FORGEJO) + fixture_bound(GITHUB));
    let l = fixtures::limits();
    let s = fixtures::shared();
    // Keep connection capacity fixed to measure the call slot alone.
    let more = crate::Shared { calls: s.calls + 1, ..s };
    let mut growth = [0; 3];
    for (forges, delta) in [FORGEJO, GITHUB, MIXED].into_iter().zip(growth.iter_mut()) {
        *delta = limits::worst_case(&l, &more, forges, 0).expect("extra slot")
            - limits::worst_case(&l, &s, forges, 0).expect("fixture slots");
    }
    let [forgejo, github, mixed] = growth;
    assert_eq!(forgejo, FORGEJO_CALL_SLOT);
    assert_eq!(github, GITHUB_CALL_SLOT);
    assert_eq!(mixed, forgejo.max(github));
}

#[test]
fn the_worst_case_grows_with_each_kind() {
    // The grid includes absent kinds, one of each, and repeated forges of each kind.
    const COUNTS: [u32; 3] = [0, 1, 2];
    let l = fixtures::limits();
    let s = fixtures::shared();
    for forgejo in COUNTS {
        for github in COUNTS {
            let counts = limits::Forges { forgejo, github };
            let base = limits::worst_case(&l, &s, counts, 0).expect("grid bound");
            for more in
                [limits::Forges { forgejo: forgejo + 1, github }, limits::Forges { forgejo, github: github + 1 }]
            {
                assert!(limits::worst_case(&l, &s, more, 0).expect("larger bound") > base, "{counts:?} -> {more:?}");
            }
            let mut settings = fixtures::settings();
            settings.forges = skein_lib::List::with_capacity(forgejo + github);
            for (kind, count) in [(crate::settings::Kind::Forgejo, forgejo), (crate::settings::Kind::GitHub, github)] {
                for _ in 0..count {
                    for mut forge in fixtures::settings().forges.into_boxed() {
                        forge.kind = kind;
                        settings.forges.push(forge).expect("counted capacity");
                    }
                }
            }
            assert_eq!(limits::Forges::of(&settings), Some(counts));
        }
    }
}

#[test]
fn no_configured_forge_pays_neither_dialects_share() {
    let l = fixtures::limits();
    let s = fixtures::shared();
    let larger = Limits { key_bytes: u32::MAX, installations: u32::MAX, repositories: u32::MAX, users: u32::MAX, ..l };
    assert_eq!(limits::worst_case(&l, &s, NO_FORGES, 0), limits::worst_case(&larger, &s, NO_FORGES, 0));
    let mut settings = fixtures::settings();
    settings.forges = skein_lib::List::with_capacity(l.forges);
    assert_eq!(limits::Forges::of(&settings), Some(NO_FORGES));
    assert_eq!(limits::check(&l, &s, &settings), Ok(()));
    assert_eq!(limits::worst_case(&l, &s, limits::Forges { forgejo: u32::MAX, github: 1 }, 0), None);
}

#[test]
fn the_worst_case_counts_answers_and_refuses_overflow() {
    let limits = fixtures::limits();
    let shared = fixtures::shared();
    let bytes = limits::worst_case(
        &limits,
        &shared,
        limits::Forges::of(&fixtures::settings()).expect("forge counts"),
        crate::settings::owned_bytes(&fixtures::settings()).expect("owner bytes"),
    )
    .expect("fixture bound fits");

    let owner_bytes = u64::try_from(fixtures::WORD.len() + fixtures::USER_AGENT.len()).expect("fixture lengths");
    assert_eq!(crate::settings::owned_bytes(&fixtures::settings()), Some(owner_bytes));
    assert_eq!(
        limits::worst_case(&limits, &shared, limits::Forges { forgejo: 1, github: 0 }, 0).expect("fixed bound")
            + owner_bytes,
        bytes
    );
    assert_eq!(
        limits::worst_case(&limits, &shared, limits::Forges { forgejo: 1, github: 0 }, u64::MAX),
        None,
        "owner bytes cannot overflow"
    );
    let trust = fixtures::trust();
    for (name, extra) in [("forge.example", 13), ("192.0.2.7", 0), ("2001:db8::7", 0)] {
        let mut settings = fixtures::settings();
        settings.forges.get_mut(0).expect("one forge").transport = crate::settings::Transport::Tls {
            server_name: skein_tls::Name::new(name).expect("valid name"),
            trust: trust.clone(),
        };
        assert_eq!(crate::settings::owned_bytes(&settings), Some(owner_bytes + extra), "{name}");
    }
    let mut settings = fixtures::settings();
    settings.forges.get_mut(0).expect("one forge").transport =
        crate::settings::Transport::Plaintext { name: Some(Box::from(&b"forge.example"[..])) };
    assert_eq!(crate::settings::owned_bytes(&settings), Some(owner_bytes + 13));
    let huge = Limits { string_bytes: u32::MAX, inner: u32::MAX, connections: u32::MAX, ..limits };
    assert_eq!(
        limits::worst_case(
            &huge,
            &shared,
            limits::Forges { forgejo: 1, github: 0 },
            crate::settings::owned_bytes(&fixtures::settings()).expect("owner bytes")
        ),
        None,
        "multipliers overflow checked"
    );
}

#[test]
fn many_due_calls_keep_the_timer_output_bound() {
    let limits = Limits { connections: 20, ..fixtures::limits() };
    let shared = fixtures::shared();
    let shared = crate::Shared { calls: 20, ..shared };
    let mut h = fixtures::Harness::with(limits, shared, fixtures::settings());
    for call in 0..20 {
        h.call(call, fixtures::REPO, api::Op::Read(api::Read::Settings));
    }
    for (name, span, count, deadline) in [
        ("first batch", Duration::from_secs(10), 16, Some(Time::from_nanos(10_000_000_000))),
        ("rest", Duration::ZERO, 20, Some(Time::from_nanos(11_000_000_000))),
    ] {
        h.run(span);
        assert_eq!(h.answers.len(), count, "{name}: calls ended");
        assert_eq!(h.component.next_deadline(), deadline, "{name}: earliest remaining");
    }
}

#[test]
fn many_held_answers_keep_the_offer_output_bound() {
    let limits = Limits { connections: 20, ..fixtures::limits() };
    let shared = fixtures::shared();
    let shared = crate::Shared { calls: 20, ..shared };
    let mut h = fixtures::Harness::with(limits, shared, fixtures::settings());
    for call in 0..20 {
        h.send(Request::Refused(Answered {
            call: Token::new(call),
            cost: 0,
            free: 0,
            rate: None,
            result: Err(api::Error::Unavailable),
        }));
    }
    for (name, count) in [("first batch", 16), ("second batch", 4), ("empty", 0)] {
        h.send(Request::Offer);
        assert_eq!(h.up.len(), count, "{name}: offer batch");
        for _ in 0..count {
            drop(h.up.pop());
        }
    }
}

#[test]
fn owner_words_are_checked_before_io() {
    // forge.md 6 admits one through sixteen lowercase letters, digits or hyphens.
    const LONGEST: &[u8] = b"a0123456789-abcd";
    const TOO_LONG: &[u8] = b"a0123456789-abcde";
    let limits = Limits { body_bytes: fixtures::MAX_WORD_BODY_ROOM, ..fixtures::limits() };
    for (word, expected) in [
        (b"".as_slice(), Err(Unusable::Word)),
        (b"a".as_slice(), Ok(())),
        (LONGEST, Ok(())),
        (TOO_LONG, Err(Unusable::Word)),
        (b"K".as_slice(), Err(Unusable::Word)),
    ] {
        let mut settings = fixtures::settings();
        settings.word = Box::from(word);
        fixtures::admission(settings, &limits, expected, word);
    }
}

#[test]
fn body_room_includes_the_owner_word() {
    let shared = fixtures::shared();
    let settings = fixtures::settings();
    for (body_bytes, expected) in [
        (
            fixtures::BODY_ROOM_SHORT,
            Err(Unusable::BodyBytes { body_bytes: fixtures::BODY_ROOM_SHORT, needed: u64::from(fixtures::BODY_ROOM) }),
        ),
        (fixtures::BODY_ROOM, Ok(())),
    ] {
        let limits = Limits { body_bytes, ..fixtures::limits() };
        assert_eq!(limits::check(&limits, &shared, &settings), expected, "body bytes {body_bytes}");
    }
}

#[test]
#[expect(clippy::manual_map, reason = "step fixtures use explicit matches without closures")]
fn each_request_carries_the_host_its_transport_derives() {
    use crate::settings::Transport;
    let trust = fixtures::trust();
    let v4 = core::net::IpAddr::V4(Ipv4Addr::LOCALHOST);
    let v6 = core::net::IpAddr::V6(core::net::Ipv6Addr::LOCALHOST);
    let cases = [
        (SocketAddr::new(v4, 80), None, false, "127.0.0.1"),
        (SocketAddr::new(v4, 443), None, false, "127.0.0.1"),
        (SocketAddr::new(v4, 3000), None, false, "127.0.0.1:3000"),
        (SocketAddr::new(v6, 80), None, false, "[0:0:0:0:0:0:0:1]"),
        (SocketAddr::new(v6, 3000), None, false, "[0:0:0:0:0:0:0:1]:3000"),
        (SocketAddr::new(v4, 80), Some("forge.example"), false, "forge.example"),
        (SocketAddr::new(v6, 443), Some("forge.example"), false, "forge.example"),
        (SocketAddr::new(v6, 3000), Some("forge.example"), false, "forge.example:3000"),
        (SocketAddr::new(v4, 80), Some("forge.example"), true, "forge.example"),
        (SocketAddr::new(v4, 443), Some("forge.example"), true, "forge.example"),
        (SocketAddr::new(v6, 3000), Some("forge.example"), true, "forge.example:3000"),
        (SocketAddr::new(v4, 3000), Some("192.0.2.7"), true, "192.0.2.7:3000"),
        (SocketAddr::new(v4, 443), Some("2001:db8::7"), true, "[2001:db8:0:0:0:0:0:7]"),
        (SocketAddr::new(v6, 443), Some("::1"), true, "[0:0:0:0:0:0:0:1]"),
    ];
    for (address, name, tls, expected) in cases {
        let mut settings = fixtures::settings();
        let forge = settings.forges.get_mut(0).expect("one forge");
        forge.address = skein_io::Addr::Ip(address);
        forge.transport = if tls {
            Transport::Tls {
                server_name: skein_tls::Name::new(name.expect("TLS name")).expect("valid name"),
                trust: trust.clone(),
            }
        } else {
            Transport::Plaintext {
                name: match name {
                    Some(text) => Some(Box::from(text.as_bytes())),
                    None => None,
                },
            }
        };
        assert_eq!(
            limits::check(&fixtures::limits(), &fixtures::shared(), &settings),
            Ok(()),
            "{address} {name:?} TLS={tls}"
        );
        let sent = fixtures::request_bytes(settings);
        assert_eq!(fixtures::header(&sent, b"Host: "), expected.as_bytes(), "{address} {name:?} TLS={tls}");
        assert_eq!(fixtures::header(&sent, b"User-Agent: "), fixtures::USER_AGENT, "{address} {name:?} TLS={tls}");
    }
}

#[test]
fn request_head_counts_user_agent() {
    for (name, tls, host, room) in [
        ("forge.example", true, b"forge.example:3000".as_slice(), fixtures::TLS_REQUEST_ROOM),
        ("2001:db8::7", true, b"[2001:db8:0:0:0:0:0:7]:3000".as_slice(), fixtures::TLS_REQUEST_ROOM),
        ("forge.example", false, b"forge.example:3000".as_slice(), fixtures::PLAIN_REQUEST_ROOM),
    ] {
        let limits = fixtures::limits();
        let length = room
            .checked_sub(usize::try_from(limits.target_bytes).expect("target fits"))
            .expect("target room")
            .checked_sub(host.len())
            .expect("host room")
            .checked_sub(usize::try_from(limits.token_bytes).expect("token fits"))
            .expect("token room")
            .checked_sub(fixtures::HEAD_FRAMING)
            .expect("framing room");
        for (bytes, expected) in [
            (length, Ok(())),
            (length.checked_add(1).expect("one past"), Err(Unusable::Request { forge: fixtures::FORGE })),
        ] {
            fixtures::admission(fixtures::head_settings(name, tls, bytes), &limits, expected, host);
        }
    }
}

#[test]
fn header_controls_are_refused_before_io() {
    for (value, valid) in [
        (b"".as_slice(), false),
        (b"\0", false),
        (b"\r", false),
        (b"\n", false),
        (b"\x1f", false),
        (b"\x7f", false),
        (b"\t", true),
        (b" ", true),
        (b"~", true),
        (b"\x80", true),
        (b"\xff", true),
    ] {
        let mut settings = fixtures::settings();
        settings.forges.get_mut(0).expect("forge").access.as_mut().expect("fixture access").credential =
            skein_forge::credential::Credential::Token(Secret::new(Box::from(value)));
        let expected = if valid { Ok(()) } else { Err(Unusable::Token { forge: fixtures::FORGE }) };
        fixtures::admission(settings, &fixtures::limits(), expected, value);
        let mut settings = fixtures::settings();
        settings.user_agent = Box::from(value);
        let expected = if valid { Ok(()) } else { Err(Unusable::Request { forge: fixtures::FORGE }) };
        fixtures::admission(settings, &fixtures::limits(), expected, value);
    }
    for (user, expected) in [(0, Err(Unusable::User { forge: fixtures::FORGE })), (1, Ok(()))] {
        let mut settings = fixtures::settings();
        settings.forges.get_mut(0).expect("forge").access.as_mut().expect("fixture access").user = user;
        fixtures::admission(settings, &fixtures::limits(), expected, b"user");
    }
}

fn forge_count_token_and_hook_boundaries() {
    let limits = fixtures::limits();
    for (count, expected) in [
        (limits.forges, Ok(())),
        (limits.forges.checked_add(1).expect("fixture count"), Err(Unusable::Forges { forges: limits.forges })),
    ] {
        let mut settings = fixtures::settings();
        settings.forges = skein_lib::List::with_capacity(count);
        for number in 1..=count {
            let mut forge = fixtures::settings().forges.into_boxed().into_iter().next().expect("forge");
            forge.number = u16::try_from(number).expect("fixture number");
            settings.forges.push(forge).expect("fixture capacity");
        }
        fixtures::admission(settings, &limits, expected, b"forge count");
    }
    for (length, expected) in [
        (0, Err(Unusable::Token { forge: fixtures::FORGE })),
        (fixtures::TOKEN_BOUND, Ok(())),
        (fixtures::TOKEN_BOUND + 1, Err(Unusable::Token { forge: fixtures::FORGE })),
    ] {
        let mut settings = fixtures::settings();
        settings.forges.get_mut(0).expect("forge").access.as_mut().expect("fixture access").credential =
            skein_forge::credential::Credential::Token(Secret::new(fixtures::filled(length, b'a')));
        fixtures::admission(settings, &limits, expected, b"token bound");
    }
    hook_setting_boundaries();
}
fn hook_setting_boundaries() {
    for (listener, expected) in [
        (skein_io::Addr::Ip(([127, 0, 0, 1], 7200).into()), Ok(())),
        (skein_io::Addr::Ip(([0, 0, 0, 0, 0, 0, 0, 1], 7200).into()), Ok(())),
        (skein_io::Addr::Ip(([192, 0, 2, 1], 7200).into()), Err(Unusable::Listener)),
        (skein_io::Addr::InProcess(skein_io::Name::EMPTY), Err(Unusable::Listener)),
    ] {
        let mut settings = fixtures::hook_settings();
        settings.webhooks.as_mut().expect("hooks").listener = listener;
        fixtures::admission(settings, &fixtures::limits(), expected, b"listener");
    }
    for (length, expected) in [
        (0, Err(Unusable::Secret { forge: fixtures::FORGE })),
        (fixtures::SECRET_BOUND, Ok(())),
        (fixtures::SECRET_BOUND + 1, Err(Unusable::Secret { forge: fixtures::FORGE })),
    ] {
        let mut settings = fixtures::hook_settings();
        settings.forges.get_mut(0).expect("forge").access.as_mut().expect("fixture access").webhook_secret =
            Some(Secret::new(fixtures::filled(length, b'a')));
        fixtures::admission(settings, &fixtures::limits(), expected, b"secret");
    }
    let mut settings = fixtures::settings();
    settings.forges.get_mut(0).expect("forge").access.as_mut().expect("fixture access").webhook_secret =
        Some(Secret::new(Box::from(fixtures::HOOK_SECRET)));
    fixtures::admission(
        settings,
        &fixtures::limits(),
        Err(Unusable::Secret { forge: fixtures::FORGE }),
        b"no listener",
    );
}

fn numeric_boundaries() {
    let l = fixtures::limits();
    for invalid in [
        Limits { forges: 0, ..l },
        Limits { token_bytes: 0, ..l },
        Limits { key_bytes: 0, ..l },
        Limits { connections: 0, ..l },
        Limits { head_bytes: 0, ..l },
        Limits { headers: 0, ..l },
        Limits { body_bytes_in: 0, ..l },
        Limits { depth: 0, ..l },
        Limits { chunk: 0, ..l },
        Limits { string_bytes: 0, ..l },
        Limits { body_bytes: 0, ..l },
        Limits { inner: 0, ..l },
        Limits { target_bytes: 0, ..l },
        Limits { repositories: 0, ..l },
        Limits { users: 0, ..l },
        Limits { installations: 0, ..l },
        Limits { hooks: 0, ..l },
        Limits { hook_head_bytes: 0, ..l },
        Limits { hook_body_bytes: 0, ..l },
        Limits { hook_secret_bytes: 0, ..l },
        Limits { tls: skein_tls::client::Limits { read: 0, ..l.tls }, ..l },
        Limits { tls: skein_tls::client::Limits { send: 0, ..l.tls }, ..l },
        Limits { tls: skein_tls::client::Limits { records: 0, ..l.tls }, ..l },
        Limits { deadline: Duration::ZERO, ..l },
        Limits { idle: Duration::ZERO, ..l },
        Limits { start_backoff: Duration::ZERO, ..l },
        Limits { start_backoff_max: Duration::ZERO, ..l },
        Limits { renew_before: Duration::ZERO, ..l },
        Limits { hook_deadline: Duration::ZERO, ..l },
    ] {
        assert_eq!(
            limits::check(&invalid, &fixtures::shared(), &fixtures::settings()),
            Err(Unusable::Zero),
            "{invalid:?}"
        );
    }
    for (limits, expected) in [
        (l, Ok(())),
        (Limits { hook_secret_bytes: 31, ..l }, Err(Unusable::SecretBytes { bytes: 31 })),
        (Limits { hook_secret_bytes: 32, ..l }, Ok(())),
        (Limits { depth: 7, ..l }, Err(Unusable::Documents(skein_forge::forgejo::limits::Problem::Depth))),
        (Limits { depth: 8, ..l }, Ok(())),
    ] {
        assert_eq!(limits::check(&limits, &fixtures::shared(), &fixtures::settings()), expected, "{limits:?}");
    }
    let overflow = crate::Shared { op_bytes: u32::MAX, ..fixtures::shared() };
    assert_eq!(limits::check(&l, &overflow, &fixtures::settings()), Err(Unusable::Zero));
}
fn neighbour_boundaries() {
    let l = fixtures::limits();
    let s = fixtures::shared();
    for (limits, expected) in [
        (
            Limits { connections: s.calls.checked_sub(1).expect("fixture calls"), ..l },
            Err(Unusable::Connections { connections: s.calls.checked_sub(1).expect("fixture calls"), calls: s.calls }),
        ),
        (Limits { connections: s.calls, ..l }, Ok(())),
        (
            Limits { connections: s.calls.checked_add(1).expect("fixture calls"), ..l },
            Err(Unusable::Connections { connections: s.calls.checked_add(1).expect("fixture calls"), calls: s.calls }),
        ),
        (Limits { deadline: s.lifetime, ..l }, Ok(())),
        (
            Limits { deadline: s.lifetime.saturating_add(Duration::from_nanos(1)), ..l },
            Err(Unusable::Deadline {
                deadline: s.lifetime.saturating_add(Duration::from_nanos(1)),
                lifetime: s.lifetime,
            }),
        ),
        (
            Limits { string_bytes: s.name_bytes.checked_sub(1).expect("fixture name"), ..l },
            Err(Unusable::StringBytes {
                string_bytes: s.name_bytes.checked_sub(1).expect("fixture name"),
                name_bytes: s.name_bytes,
            }),
        ),
        (Limits { string_bytes: s.name_bytes, ..l }, Ok(())),
        (Limits { repositories: s.repositories, ..l }, Ok(())),
        (
            Limits { repositories: s.repositories.checked_add(1).expect("fixture repositories"), ..l },
            Err(Unusable::Repositories {
                repositories: s.repositories.checked_add(1).expect("fixture repositories"),
                forge: s.repositories,
            }),
        ),
    ] {
        assert_eq!(limits::check(&limits, &s, &fixtures::settings()), expected, "{limits:?}");
    }
}
fn stack_boundaries() {
    // The JSON caps are derived from the same chunk demand; exercise their
    // comparison directly as well as the public agreement checks.
    const DEMAND: u32 = 1024; // fixtures::limits().chunk
    for stacked in [
        limits::Stacked::HttpOverTls,
        limits::Stacked::HttpOverIo,
        limits::Stacked::TlsOverIo,
        limits::Stacked::JsonOverHttp,
        limits::Stacked::ServerOverIo,
        limits::Stacked::JsonOverServer,
    ] {
        for (cap, expected) in
            [(DEMAND, Ok(())), (DEMAND - 1, Err(Unusable::Stack { stacked, demand: DEMAND, cap: DEMAND - 1 }))]
        {
            assert_eq!(limits::stack(stacked, DEMAND, cap), expected, "{stacked:?}/{cap}");
        }
    }
}

#[test]
fn vocabulary_app_credential_is_unusable_in_settings() {
    let mut settings = fixtures::settings();
    settings.forges.get_mut(0).expect("one forge").access.as_mut().expect("fixture access").credential =
        skein_forge::credential::Credential::App { id: 1, key: Secret::new(Box::new([])) };
    assert_eq!(
        limits::check(&fixtures::limits(), &fixtures::shared(), &settings),
        Err(Unusable::Credential { forge: fixtures::FORGE })
    );
}

#[test]
fn github_limits_are_checked_only_with_a_github_forge() {
    let l = Limits {
        key_bytes: 1,
        renew_before: Duration::from_secs(1),
        deadline: Duration::from_secs(300),
        ..fixtures::limits()
    };
    let shared = crate::Shared { lifetime: l.deadline, ..fixtures::shared() };
    assert_eq!(limits::check(&l, &shared, &fixtures::settings()), Ok(()));
    assert_eq!(
        limits::check(&l, &shared, &fixtures::github_settings()),
        Err(Unusable::AppDeadline { deadline: l.deadline })
    );
}

#[test]
fn a_github_forge_bounds_the_deadline_at_270_seconds() {
    let l = Limits { deadline: Duration::from_secs(270), renew_before: Duration::from_secs(600), ..fixtures::limits() };
    let past = l.deadline.saturating_add(Duration::from_nanos(1));
    let shared = crate::Shared { lifetime: past, ..fixtures::shared() };
    let settings = fixtures::github_settings();
    assert_eq!(limits::check(&l, &shared, &settings), Ok(()));
    assert_eq!(
        limits::check(&Limits { deadline: past, ..l }, &shared, &settings),
        Err(Unusable::AppDeadline { deadline: past })
    );
}

#[test]
fn renew_before_is_between_twice_the_deadline_and_half_an_hour() {
    let l = fixtures::limits();
    let least = l.deadline.saturating_mul(2);
    let most = Duration::from_secs(1800);
    let below = Duration::from_nanos(least.as_nanos().checked_sub(1).expect("one before"));
    let above = most.saturating_add(Duration::from_nanos(1));
    let settings = fixtures::github_settings();
    for (renew_before, expected) in [
        (least, Ok(())),
        (most, Ok(())),
        (below, Err(Unusable::RenewBefore { renew_before: below, deadline: l.deadline })),
        (above, Err(Unusable::RenewBefore { renew_before: above, deadline: l.deadline })),
    ] {
        assert_eq!(limits::check(&Limits { renew_before, ..l }, &fixtures::shared(), &settings), expected);
    }
}

#[test]
fn key_bytes_below_the_signer_bound_is_refused() {
    let l = fixtures::limits();
    let settings = fixtures::github_settings();
    for (key_bytes, expected) in [(1196, Ok(())), (1195, Err(Unusable::KeyBytes { key_bytes: 1195 }))] {
        assert_eq!(limits::check(&Limits { key_bytes, ..l }, &fixtures::shared(), &settings), expected);
    }
}

#[test]
fn a_github_forge_reserves_the_jwt_in_the_request_head() {
    let l = fixtures::limits();
    let shared = fixtures::shared();
    let mut settings = fixtures::github_settings();
    let forge = settings.forges.get(0).expect("forge");
    let request = limits::http(&l, false, &shared.io).request;
    let host = crate::settings::host(forge).expect("host");
    let agent_bytes = usize::try_from(request)
        .expect("request fits")
        .checked_sub(usize::try_from(l.target_bytes).expect("target fits"))
        .expect("target room")
        .checked_sub(host.len())
        .expect("host room")
        .checked_sub(skein_forge::github::jwt::JWT_BYTES)
        .expect("JWT room")
        .checked_sub(fixtures::HEAD_FRAMING)
        .expect("framing room");
    settings.user_agent = fixtures::filled(agent_bytes, b'a');
    assert_eq!(limits::check(&l, &shared, &settings), Ok(()));
    settings.user_agent = fixtures::filled(agent_bytes.checked_add(1).expect("one past"), b'a');
    assert_eq!(limits::check(&l, &shared, &settings), Err(Unusable::Request { forge: fixtures::FORGE }));
    let forge = settings.forges.get_mut(0).expect("forge");
    forge.kind = crate::settings::Kind::Forgejo;
    forge.access = fixtures::settings().forges.get_mut(0).expect("forge").access.take();
    assert_eq!(limits::check(&l, &shared, &settings), Ok(()));
}

#[test]
fn unusable_github_settings_name_the_forge() {
    let mut secret = fixtures::github_settings().forges.get_mut(0).expect("forge").access.take().expect("access");
    secret.webhook_secret = Some(Secret::new(Box::from(fixtures::HOOK_SECRET)));
    for (label, access, expected) in [
        (
            "token",
            fixtures::settings().forges.get_mut(0).expect("forge").access.take(),
            Err(Unusable::Credential { forge: fixtures::FORGE }),
        ),
        ("bad key", Some(fixtures::app_access(&[0; 64], 7, 9)), Err(Unusable::Key { forge: fixtures::FORGE })),
        ("secret", Some(secret), Err(Unusable::Secret { forge: fixtures::FORGE })),
        ("no access", None, Ok(())),
    ] {
        let mut settings = fixtures::github_settings();
        settings.forges.get_mut(0).expect("forge").access = access;
        assert_eq!(limits::check(&fixtures::limits(), &fixtures::shared(), &settings), expected, "{label}");
    }
}

#[test]
fn the_component_admits_a_github_forge() {
    Component::new(fixtures::github_settings(), &fixtures::limits(), &fixtures::shared()).expect("GitHub admitted");
}

#[test]
fn a_github_forge_without_access_is_inert() {
    let mut settings = fixtures::github_settings();
    settings.forges.get_mut(0).expect("forge").access = None;
    let mut h = fixtures::Harness::new(settings);
    h.run(Duration::from_secs(20));
    assert_eq!(h.start(fixtures::FORGE), Start::Unconnected);
    assert_eq!(h.connections_opened(), 0);
}
