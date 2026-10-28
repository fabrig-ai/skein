//! Domain fixtures and piece transitions.
use crate::Config;
use crate::api::{Finish, Line, Message, Part, Query, Role, Script, ToolSpec, Turn};
use alloc::boxed::Box;
use skein_lib::Duration;
use skein_lib::bytes::copy_of;

pub(super) const CONFIG: Config = Config {
    calls: 4,
    query_bytes: 1 << 20,
    script_bytes: 1 << 20,
    answer_bytes: 1 << 20,
    latency_min: Duration::ZERO,
    latency_max: Duration::ZERO,
    overloaded: 0,
    rate_limited: 0,
    retry_after: Duration::ZERO,
    unavailable: 0,
    too_long: 0,
    unauthorized: 0,
    refused: 0,
    no_calls: 0,
    answer_tokens: 1,
    calls_per_answer: 1,
    malformed: 0,
    outside_choice: 0,
    tool_rounds: 1,
    cache_lifetime: Duration::from_secs(300),
    cache_entries: 8,
    unscoped_reads: 0,
};

pub(super) fn user(parts: Box<[Part]>) -> Message {
    Message { role: Role::User, parts }
}

pub(super) fn query(messages: Box<[Message]>) -> Query {
    let tool = ToolSpec { name: copy_of(b"ls"), description: copy_of(b""), parameters: copy_of(b"{}") };
    Query {
        credential: None,
        caching: crate::api::Caching::Unscoped,
        model: copy_of(b"fake"),
        system: copy_of(b""),
        tools: Box::new([tool]),
        messages,
        max_tokens: 100,
        choice: crate::api::ToolChoice::Auto,
    }
}

pub(super) fn script(cue: &[u8], turns: Box<[Turn]>) -> Script {
    Script { cue: copy_of(cue), turns }
}

pub(super) fn says(text: &[u8]) -> Turn {
    Turn { lines: Box::new([Line::Text { text: copy_of(text) }]), finish: Finish::Stop, tokens: 2 }
}

use crate::api::Piece;
use crate::{Domain, Event, Request, fire, step};
use skein_lib::{Env, Queue, ReplyTo, Time, Token, Wall};

const CALL: Token = Token::new(7);
const OTHER_CALL: Token = Token::new(8);
const MAX_HOLD_BYTES: usize = 64; // fake-llm.md 2.2: admitted hold names.
const PAST_HOLD_BYTES: usize = MAX_HOLD_BYTES + 1;
const INVALID_SCRIPT_CASES: usize = 5; // Placement, count, text, empty hold and long hold.
const LATENCY: Duration = Duration::from_secs(1);
const STREAM_CONFIG: Config = Config { latency_min: LATENCY, latency_max: LATENCY, ..CONFIG };

fn piece(text: &[u8], hold: Option<&[u8]>) -> Piece {
    let text = copy_of(text);
    let Some(name) = hold else {
        return Piece { text, hold: None };
    };
    Piece { text, hold: Some(copy_of(name)) }
}

fn pieces_turn(pieces: Box<[Piece]>) -> Turn {
    Turn { lines: Box::new([Line::Pieces { pieces }]), finish: Finish::Stop, tokens: 2 }
}

fn request() -> Query {
    let mut query = query(Box::new([user(Box::new([Part::Text { text: copy_of(b"hi") }]))]));
    query.system = copy_of(b"@main");
    query
}

struct Playback {
    domain: Domain,
    env: Env<Config>,
    out: Queue<Request>,
}
impl Playback {
    fn new(config: Config, pieces: Box<[Piece]>) -> Self {
        Self::from_query(config, pieces, request())
    }
    fn from_query(config: Config, pieces: Box<[Piece]>, query: Query) -> Self {
        Self::from_turn(config, pieces_turn(pieces), query)
    }
    fn from_turn(config: Config, turn: Turn, query: Query) -> Self {
        let domain = Domain::scripted(&config, 1, Box::new([script(b"@main", Box::new([turn]))]));
        Self::from_domain(config, domain, query)
    }
    fn from_domain(config: Config, domain: Domain, query: Query) -> Self {
        let mut playback =
            Self { domain, env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: config }, out: Queue::uncapped() };
        playback.call(CALL, query);
        playback
    }
    fn call(&mut self, call: Token, query: Query) {
        step(&mut self.domain, &self.env, Event::Call { reply_to: ReplyTo::new(call), call, query }, &mut self.out);
    }
    fn fire(&mut self, seconds: u64) -> Option<Request> {
        self.env.now = Time::ZERO.saturating_add(Duration::from_secs(seconds));
        fire(&mut self.domain, &self.env, &mut self.out);
        self.out.pop()
    }
    fn release(&mut self, name: &[u8]) {
        step(&mut self.domain, &self.env, Event::Release { name: copy_of(name) }, &mut self.out);
    }
}

#[test]
fn a_held_piece_waits_with_no_timer_until_released() {
    let mut play = Playback::new(STREAM_CONFIG, Box::new([piece(b"Hel", None), piece(b"lo", Some(b"h"))]));
    play.fire(1);
    let held = play.fire(2);
    let Some(Request::Hold { call, .. }) = held else { panic!("the held piece emits a hold") };
    assert_eq!(call, CALL);
    assert_eq!(play.domain.next_deadline(), None, "a held call has no timer");
    play.release(b"h");
    assert!(play.out.is_empty(), "a release sends nothing");
    let Some(Request::Reply { result: Ok(answer), .. }) = play.fire(3) else { panic!("the reply ends the call") };
    assert_eq!(&*answer.parts, &[Part::Text { text: copy_of(b"Hello") }]);
}

#[test]
fn each_piece_is_sent_at_its_own_latency() {
    let mut play = Playback::new(STREAM_CONFIG, Box::new([piece(b"a", None), piece(b"b", None), piece(b"c", None)]));
    for (seconds, text) in [(1, b"a"), (2, b"b")] {
        assert_eq!(play.domain.next_deadline(), Some(Time::ZERO.saturating_add(Duration::from_secs(seconds))));
        let Some(Request::Piece { call, text: sent, .. }) = play.fire(seconds) else {
            panic!("a text piece at its latency")
        };
        assert_eq!((call, sent.as_ref()), (CALL, text.as_slice()));
    }
    let Some(Request::Reply { result: Ok(answer), .. }) = play.fire(3) else { panic!("the last piece ends the call") };
    assert_eq!(&*answer.parts, &[Part::Text { text: copy_of(b"abc") }]);
}

#[test]
fn a_release_before_its_hold_changes_nothing() {
    let mut play = Playback::new(STREAM_CONFIG, Box::new([piece(b"a", Some(b"h"))]));
    play.release(b"h");
    play.fire(1);
    assert_eq!(play.domain.next_deadline(), None);
    assert_eq!(play.fire(2), None);
}

#[test]
fn one_release_frees_every_call_held_at_its_name() {
    let mut play = Playback::new(STREAM_CONFIG, Box::new([piece(b"a", Some(b"h"))]));
    play.call(OTHER_CALL, request());
    play.fire(1);
    play.fire(1);
    play.release(b"h");
    let mut calls = [None, None];
    for call in &mut calls {
        let Some(Request::Reply { to, .. }) = play.fire(2) else { panic!("both held calls resume") };
        *call = Some(to.into_token());
    }
    calls.sort();
    assert_eq!(calls, [Some(CALL), Some(OTHER_CALL)]);
}

#[test]
fn a_faulted_reply_in_pieces_goes_whole_in_one_reply() {
    let mut play = Playback::new(Config { overloaded: 1000, ..STREAM_CONFIG }, Box::new([piece(b"a", Some(b"h"))]));
    let Some(Request::Reply { result, .. }) = play.fire(1) else { panic!("a fault is a terminal") };
    assert_eq!(result, Err(crate::api::Error::Overloaded.into()));
}

#[test]
fn a_reply_cut_by_max_tokens_goes_whole() {
    let mut short = request();
    short.max_tokens = 1; // The scripted turn takes two tokens.
    let mut play = Playback::from_query(STREAM_CONFIG, Box::new([piece(b"abc", Some(b"h"))]), short);
    let Some(Request::Reply { result: Ok(answer), .. }) = play.fire(1) else { panic!("the cut answer goes whole") };
    assert_eq!(answer.finish, Finish::Length);
    assert_eq!(&*answer.parts, &[Part::Text { text: copy_of(b"do") }]);
}

#[test]
fn a_script_breaking_the_pieces_rule_is_refused() {
    let cases: [(&str, Box<[Line]>); INVALID_SCRIPT_CASES] = [
        (
            "second",
            Box::new([Line::Text { text: copy_of(b"a") }, Line::Pieces { pieces: Box::new([piece(b"b", None)]) }]),
        ),
        ("empty list", Box::new([Line::Pieces { pieces: Box::new([]) }])),
        ("empty text", Box::new([Line::Pieces { pieces: Box::new([piece(b"", None)]) }])),
        ("empty hold", Box::new([Line::Pieces { pieces: Box::new([piece(b"a", Some(b""))]) }])),
        ("long hold", Box::new([Line::Pieces { pieces: Box::new([piece(b"a", Some(&[b'h'; PAST_HOLD_BYTES]))]) }])),
    ];
    for (label, lines) in cases {
        let turn = Turn { lines, finish: Finish::Stop, tokens: 2 };
        let result = Domain::try_scripted(&CONFIG, 1, Box::new([script(b"@main", Box::new([turn]))]));
        assert_eq!(result.err(), Some(crate::api::Error::InvalidRequest), "{label}");
    }
}

#[test]
fn a_hold_name_at_64_bytes_is_admitted() {
    let turn = pieces_turn(Box::new([piece(b"a", Some(&[b'h'; MAX_HOLD_BYTES]))]));
    Domain::try_scripted(&CONFIG, 1, Box::new([script(b"@main", Box::new([turn]))]))
        .expect("a hold at the bound is admitted");
}

#[test]
fn a_line_after_a_failing_line_naming_none_is_refused() {
    let fail =
        Line::Fail { error: crate::api::Error::Refused, report: crate::api::Report::none(), credentials: Box::new([]) };
    let turn = Turn { lines: Box::new([fail, Line::Text { text: copy_of(b"later") }]), ..says(b"") };
    let got = Domain::try_scripted(&CONFIG, 1, Box::new([script(b"@main", Box::new([turn]))]));
    assert_eq!(got.err(), Some(crate::api::Error::InvalidRequest));
}

#[test]
fn a_failing_lines_bytes_count_against_the_script_cap() {
    use crate::api::{Error, Report};
    // Stored wrappers, the cue, the three report values, and two u32 indices.
    const CAP: usize = size_of::<Script>()
        + size_of::<Turn>()
        + size_of::<Line>()
        + b"@main".len()
        + b"spent".len()
        + b"req_42".len()
        + b"top_p".len()
        + 2 * size_of::<u32>();
    let scripts = Box::new([script(
        b"@main",
        Box::new([Turn {
            lines: Box::new([Line::Fail {
                error: Error::Refused,
                report: Report { code: copy_of(b"spent"), request: copy_of(b"req_42"), field: copy_of(b"top_p") },
                credentials: Box::new([0, 1]),
            }]),
            ..says(b"")
        }]),
    )]);
    for (cap, expected) in [(CAP, None), (CAP - 1, Some(Error::ContextTooLong))] {
        let got = Domain::try_scripted(
            &Config { script_bytes: u32::try_from(cap).expect("fixture fits"), ..CONFIG },
            1,
            scripts.clone(),
        );
        assert_eq!(got.err(), expected, "{cap}");
    }
}

#[test]
fn a_failing_line_after_pieces_ends_the_walk_with_its_failure() {
    let failure = crate::api::Failure {
        error: crate::api::Error::Refused,
        report: crate::api::Report::none(),
        parts: Box::new([Part::Text { text: copy_of(b"before") }]),
    };
    let turn = Turn {
        lines: Box::new([
            Line::Pieces { pieces: Box::new([piece(b"be", None), piece(b"fore", Some(b"h"))]) },
            Line::Fail { error: failure.error, report: failure.report.clone(), credentials: Box::new([]) },
        ]),
        finish: Finish::Stop,
        tokens: 2,
    };
    let mut play = Playback::from_turn(STREAM_CONFIG, turn, request());
    let first = match play.fire(1) {
        Some(Request::Piece { text, .. }) => Some(text),
        Some(Request::Reply { .. } | Request::Hold { .. }) | None => None,
    };
    assert_eq!(first.as_deref(), Some(b"be".as_slice()));
    let held = match play.fire(2) {
        Some(Request::Hold { call, .. }) => Some(call),
        Some(Request::Piece { .. } | Request::Reply { .. }) | None => None,
    };
    assert_eq!(held, Some(CALL));
    assert_eq!(play.fire(3), None);
    play.release(b"h");
    assert_eq!(play.fire(4), Some(Request::Reply { to: ReplyTo::new(CALL), result: Err(failure) }));
}

/// System-only playback for response fixtures.
pub(crate) fn played(scripts: &[Script]) -> crate::domain::Played {
    crate::domain::Played {
        scripts: scripts.into(),
        cued: scripts.len(),
        asked: 0,
        resets: skein_lib::List::with_capacity(crate::respond::reset_count(scripts).expect("fixture reset count")),
    }
}

pub(crate) fn empty_menu() -> crate::api::Menu {
    crate::api::Menu { arguments: Box::new([]), invalid: Box::new([]) }
}

#[test]
fn played_groups_and_menu_share_one_byte_cap() {
    use crate::api::{Error, Menu, Scripts};
    // Three Script/Turn/Line wrappers, cues and replies, plus the menu box/body.
    const CAP: usize = 3 * (size_of::<Script>() + size_of::<Turn>() + size_of::<Line>())
        + b"system".len()
        + b"ask".len()
        + b"cued".len()
        + b"asked".len()
        + b"otherwise".len()
        + size_of::<Box<[u8]>>()
        + b"{}".len();
    for (cap, expected) in [(CAP, None), (CAP - 1, Some(Error::ContextTooLong))] {
        let scripts = Scripts {
            cued: Box::new([script(b"system", Box::new([says(b"cued")]))]),
            asked: Box::new([script(b"ask", Box::new([says(b"asked")]))]),
            otherwise: Box::new([says(b"otherwise")]),
        };
        let menu = Menu { arguments: Box::new([copy_of(b"{}")]), invalid: Box::new([]) };
        let config = Config { script_bytes: u32::try_from(cap).expect("fixture cap fits"), ..CONFIG };
        assert_eq!(Domain::played(&config, 1, scripts, menu).err(), expected, "cap {cap}");
    }
}

#[test]
fn played_refuses_bad_asked_and_fallback_turns() {
    use crate::api::{Error, Report, Scripts};
    let broken_pieces = pieces_turn(Box::new([piece(b"", None)]));
    let broken_failure = Turn {
        lines: Box::new([
            Line::Fail { error: Error::Refused, report: Report::none(), credentials: Box::new([]) },
            Line::Text { text: copy_of(b"after") },
        ]),
        ..says(b"")
    };
    for (label, turn) in [("pieces", broken_pieces), ("failure", broken_failure)] {
        let rows = [
            (
                "asked",
                Scripts {
                    cued: Box::new([]),
                    asked: Box::new([script(b"ask", Box::new([turn.clone()]))]),
                    otherwise: Box::new([]),
                },
            ),
            ("otherwise", Scripts { cued: Box::new([]), asked: Box::new([]), otherwise: Box::new([turn]) }),
        ];
        for (group, scripts) in rows {
            assert_eq!(
                Domain::played(&CONFIG, 1, scripts, empty_menu()).err(),
                Some(Error::InvalidRequest),
                "{label} {group}"
            );
        }
    }
}

#[test]
fn configured_and_system_only_played_answer_the_same_queries() {
    use crate::api::Scripts;
    // Initial request plus an assistant and user message for each of four rounds.
    const HISTORY_MESSAGES: u32 = 9;
    let scripts = Box::new([script(b"@main", Box::new([says(b"first"), says(b"second")]))]);
    let configured = Domain::configured(&CONFIG, 1, scripts.clone(), empty_menu()).expect("configured scripts");
    let played = Domain::played(
        &CONFIG,
        1,
        Scripts { cued: scripts, asked: Box::new([]), otherwise: Box::new([]) },
        empty_menu(),
    )
    .expect("played scripts");
    assert_eq!(configured.scripts(), played.scripts());
    let first = request();
    let mut left = Playback::from_domain(CONFIG, configured, first.clone());
    let mut right = Playback::from_domain(CONFIG, played, first);
    let mut history = skein_lib::List::with_capacity(HISTORY_MESSAGES);
    history.push(user(Box::new([Part::Text { text: copy_of(b"hi") }]))).expect("room for initial request");
    // The two turns, past their end, and a system cue miss.
    for system in [b"@main".as_slice(), b"@main", b"@main", b"missing"] {
        let a = left.fire(0);
        let b = right.fire(0);
        assert_eq!(a, b, "system {system:?}");
        let Some(Request::Reply { result: Ok(answer), .. }) = a else { panic!("a whole answer") };
        history.push(Message { role: Role::Assistant, parts: answer.parts }).expect("room for answer");
        history.push(user(Box::new([Part::Text { text: copy_of(b"next") }]))).expect("room for next request");
        let mut next = query(history.to_boxed());
        next.system = copy_of(system);
        left.domain.reclaim();
        right.domain.reclaim();
        left.call(CALL, next.clone());
        right.call(CALL, next);
    }
    assert_eq!(left.fire(0), right.fire(0), "random answers use the same RNG state");
}

#[test]
fn asked_and_fallback_holds_release_at_their_flat_places() {
    use crate::api::Scripts;
    use crate::domain::Played;
    use crate::pieces::Place;
    let held_turn = pieces_turn(Box::new([piece(b"Hel", None), piece(b"lo", Some(b"h"))]));
    let scripts = Scripts {
        cued: Box::new([script(b"system", Box::new([says(b"cued")])), script(b"other", Box::new([]))]),
        asked: Box::new([script(b"ask", Box::new([held_turn.clone()]))]),
        otherwise: Box::new([held_turn]),
    };
    // Two cued scripts, then the asked script, then the fallback.
    for (text, script_index) in [(b"ask".as_slice(), 2), (b"new".as_slice(), 3)] {
        let query = query(Box::new([user(Box::new([Part::Text { text: copy_of(text) }]))]));
        let mut held = Played::new(&STREAM_CONFIG, scripts.clone(), &empty_menu()).expect("held scripts");
        let response = crate::respond::respond(
            &mut skein_lib::Rng::new(1),
            &mut 0,
            &STREAM_CONFIG,
            &mut held,
            &empty_menu(),
            &query,
            Time::ZERO,
        )
        .expect("scripted pieces");
        assert_eq!(response.1, Some(Place { script: script_index, turn: 0 }));
        let domain = Domain::played(&STREAM_CONFIG, 1, scripts.clone(), empty_menu()).expect("played domain");
        assert_eq!(domain.scripts(), held.scripts.as_ref());
        let mut play = Playback::from_domain(STREAM_CONFIG, domain, query);
        let Some(Request::Piece { text, .. }) = play.fire(1) else { panic!("first piece") };
        assert_eq!(text.as_ref(), b"Hel");
        let Some(Request::Hold { call, .. }) = play.fire(2) else { panic!("held piece") };
        assert_eq!(call, CALL);
        assert_eq!(play.domain.next_deadline(), None);
        assert_eq!(play.fire(3), None);
        play.release(b"unrelated");
        assert_eq!(play.domain.next_deadline(), None);
        play.release(b"h");
        let Some(Request::Reply { result: Ok(answer), .. }) = play.fire(4) else { panic!("released reply") };
        assert_eq!(&*answer.parts, &[Part::Text { text: copy_of(b"Hello") }]);
    }
}

fn ruled_scripts(asked: bool) -> crate::api::Scripts {
    use crate::api::Scripts;
    let scripts = Box::new([script(b"cue", Box::new([says(b"wait"), says(b"answer")]))]);
    if asked {
        Scripts { cued: Box::new([]), asked: scripts, otherwise: Box::new([]) }
    } else {
        Scripts { cued: scripts, asked: Box::new([]), otherwise: Box::new([]) }
    }
}

fn rule(cue: &[u8], turn: u32) -> crate::api::Rule {
    crate::api::Rule { cue: copy_of(cue), turn }
}

fn rules_in(asked: bool, group: Box<[Box<[crate::api::Rule]>]>) -> crate::api::Rules {
    use crate::api::Rules;
    if asked { Rules { cued: Box::new([]), asked: group } } else { Rules { cued: group, asked: Box::new([]) } }
}

#[test]
fn input_rules_refuse_a_table_longer_than_its_script_group() {
    for asked in [false, true] {
        let rules = rules_in(asked, Box::new([Box::new([]), Box::new([])]));
        assert_eq!(
            Domain::played_by_input(&CONFIG, 1, ruled_scripts(asked), empty_menu(), rules).err(),
            Some(crate::api::Error::InvalidRequest),
            "asked {asked}"
        );
    }
}

#[test]
fn input_rules_refuse_turn_indices_past_the_script() {
    for asked in [false, true] {
        for rules in [Box::<[crate::api::Rule]>::from([rule(b"", 2)]), Box::new([rule(b"expected", 2), rule(b"", 0)])] {
            assert_eq!(
                Domain::played_by_input(
                    &CONFIG,
                    1,
                    ruled_scripts(asked),
                    empty_menu(),
                    rules_in(asked, Box::new([rules]))
                )
                .err(),
                Some(crate::api::Error::InvalidRequest),
                "asked {asked}"
            );
        }
    }
}

#[test]
fn input_rules_require_exactly_one_empty_cue_at_the_end() {
    for asked in [false, true] {
        for (label, rules) in [
            ("missing default", Box::<[crate::api::Rule]>::from([rule(b"expected", 1)])),
            ("early default", Box::new([rule(b"", 0), rule(b"expected", 1)])),
            ("duplicate default", Box::new([rule(b"", 0), rule(b"", 1)])),
        ] {
            assert_eq!(
                Domain::played_by_input(
                    &CONFIG,
                    1,
                    ruled_scripts(asked),
                    empty_menu(),
                    rules_in(asked, Box::new([rules]))
                )
                .err(),
                Some(crate::api::Error::InvalidRequest),
                "{label} asked {asked}"
            );
        }
    }
}

#[test]
fn input_rule_arrays_and_cues_share_the_script_and_menu_byte_cap() {
    use crate::api::Rule;
    const SCRIPT: usize =
        size_of::<Script>() + b"cue".len() + 2 * (size_of::<Turn>() + size_of::<Line>()) + b"waitanswer".len();
    const MENU: usize = size_of::<Box<[u8]>>() + b"{}".len();
    const RULES: usize = size_of::<Box<[Rule]>>() + 2 * size_of::<Rule>() + b"expected".len();
    const CAP: usize = SCRIPT + MENU + RULES;
    for asked in [false, true] {
        for (cap, expected) in [(CAP, None), (CAP - 1, Some(crate::api::Error::ContextTooLong))] {
            let config = Config { script_bytes: u32::try_from(cap).expect("fixture cap"), ..CONFIG };
            let rules = rules_in(asked, Box::new([Box::new([rule(b"expected", 1), rule(b"", 0)])]));
            let menu = crate::api::Menu { arguments: Box::new([copy_of(b"{}")]), invalid: Box::new([]) };
            assert_eq!(
                Domain::played_by_input(&config, 1, ruled_scripts(asked), menu, rules).err(),
                expected,
                "cap {cap} asked {asked}"
            );
        }
    }
}

#[test]
fn empty_rule_tables_charge_nothing_and_empty_entries_charge_their_array() {
    use crate::api::{Rule, Rules, Scripts};
    const SCRIPT: usize =
        size_of::<Script>() + b"cue".len() + 2 * (size_of::<Turn>() + size_of::<Line>()) + b"waitanswer".len();
    const CAP: usize = SCRIPT + size_of::<Box<[Rule]>>();
    let config = Config { script_bytes: 0, ..CONFIG };
    Domain::played_by_input(
        &config,
        1,
        Scripts { cued: Box::new([]), asked: Box::new([]), otherwise: Box::new([]) },
        empty_menu(),
        Rules { cued: Box::new([]), asked: Box::new([]) },
    )
    .expect("empty tables fit a zero byte cap");
    for (cap, expected) in [(CAP, None), (CAP - 1, Some(crate::api::Error::ContextTooLong))] {
        let config = Config { script_bytes: u32::try_from(cap).expect("fixture cap"), ..CONFIG };
        assert_eq!(
            Domain::played_by_input(
                &config,
                1,
                ruled_scripts(false),
                empty_menu(),
                rules_in(false, Box::new([Box::new([])])),
            )
            .err(),
            expected
        );
    }
}

#[test]
fn an_input_selected_turn_keeps_its_holds_failure_reset_and_accounting() {
    use crate::api::{Error, Report, Scripts};
    let turn = Turn {
        lines: Box::new([
            Line::Pieces { pieces: Box::new([piece(b"be", None), piece(b"fore", Some(b"h"))]) },
            Line::Fail {
                error: Error::Exhausted { retry_after: Some(Duration::from_secs(2)) },
                report: Report::none(),
                credentials: Box::new([0]),
            },
            Line::Text { text: copy_of(b"after") },
        ]),
        finish: Finish::Length,
        tokens: 7,
    };
    // One unselected cued script precedes the asked script's selected second turn.
    let scripts = Scripts {
        cued: Box::new([script(b"unselected", Box::new([says(b"other")]))]),
        asked: Box::new([script(b"ask", Box::new([says(b"wait"), turn]))]),
        otherwise: Box::new([]),
    };
    let domain = Domain::played_by_input(
        &STREAM_CONFIG,
        1,
        scripts,
        empty_menu(),
        rules_in(true, Box::new([Box::new([rule(b"expected", 1), rule(b"", 0)])])),
    )
    .expect("ruled held turn");
    let mut query = query(Box::new([user(Box::new([Part::Text { text: copy_of(b"ask expected") }]))]));
    query.credential = Some(0);
    let mut play = Playback::from_domain(STREAM_CONFIG, domain, query.clone());
    let Some(Request::Piece { text, .. }) = play.fire(1) else { panic!("selected turn's first piece") };
    assert_eq!(&*text, b"be");
    assert_eq!(play.fire(2), Some(Request::Hold { call: CALL, usage: crate::api::Usage::NONE }));
    play.release(b"h");
    let Some(Request::Reply { result: Err(failure), .. }) = play.fire(3) else { panic!("selected turn's failure") };
    assert_eq!(failure.error, Error::Exhausted { retry_after: Some(Duration::from_secs(2)) });
    assert_eq!(&*failure.parts, &[Part::Text { text: copy_of(b"before") }]);
    play.domain.reclaim();
    play.call(OTHER_CALL, query);
    let Some(Request::Piece { call, usage, text }) = play.fire(4) else { panic!("retry's first piece") };
    assert_eq!((call, usage.output, &*text), (OTHER_CALL, Some(7), b"be".as_slice()));
    let Some(Request::Hold { call, .. }) = play.fire(5) else { panic!("retry's hold") };
    assert_eq!(call, OTHER_CALL);
    play.release(b"h");
    let Some(Request::Reply { result: Ok(answer), .. }) = play.fire(6) else {
        panic!("reset passes at the selected place")
    };
    assert_eq!(&*answer.parts, &[Part::Text { text: copy_of(b"before") }, Part::Text { text: copy_of(b"after") }]);
    assert_eq!((answer.finish, answer.usage.output), (Finish::Length, Some(7)));
}
