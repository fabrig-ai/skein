//! Attaining caller-menu, script-wrapper and delayed-slot ownership controls.

use std::mem::size_of;

use skein_fake_llm_domain::api::{
    Error, Finish, InvalidInput, Line, Menu, Message, Part, Query, Role, Script, ToolSpec, Turn,
};
use skein_fake_llm_domain::{Config, Domain, Event, Request, fire, step, worst_case};
use skein_heap::{Counting, Meter};
use skein_lib::{Duration, Env, Queue, ReplyTo, Time, Token, Wall};

#[global_allocator]
static HEAP: Counting = Counting;

const ITEMS: usize = 128;

fn scripts() -> Box<[Script]> {
    Box::new([Script {
        cue: b"cue".as_slice().into(),
        turns: Box::new([Turn {
            lines: (0..ITEMS).map(|_| Line::Text { text: Box::new([]) }).collect(),
            finish: Finish::Stop,
            tokens: 1,
        }]),
    }])
}

fn menu() -> Menu {
    Menu {
        arguments: (0..ITEMS).map(|_| Box::<[u8]>::from(b"{}".as_slice())).collect(),
        invalid: (0..ITEMS)
            .map(|_| InvalidInput { name: Some(b"unknown".as_slice().into()), arguments: b"{".as_slice().into() })
            .collect(),
    }
}

fn query() -> Query {
    Query {
        credential: None,
        caching: skein_fake_llm_domain::api::Caching::Unscoped,
        model: Box::new([]),
        system: b"cue".as_slice().into(),
        tools: Box::new([]),
        messages: Box::new([Message { role: Role::User, parts: Box::new([Part::Text { text: Box::new([]) }]) }]),
        max_tokens: 10,
        choice: skein_fake_llm_domain::api::ToolChoice::Auto,
    }
}

fn caps() -> skein_fake_llm_domain::Config {
    let scripts = size_of::<Script>() + 3 + size_of::<Turn>() + ITEMS * size_of::<Line>();
    let menu = ITEMS * (size_of::<Box<[u8]>>() + 2 + size_of::<InvalidInput>() + 7 + 1);
    skein_fake_llm_domain::Config {
        calls: 4,
        script_bytes: u32::try_from(scripts + menu).expect("bounded exact aggregate"),
        answer_bytes: u32::try_from(ITEMS * size_of::<Part>()).expect("bounded exact wrapper answer"),
        query_bytes: u32::try_from(3 + size_of::<Message>() + size_of::<Part>()).expect("bounded exact query"),
        latency_min: Duration::from_secs(1),
        latency_max: Duration::from_secs(1),
        ..skein_llm_world::fake::config()
    }
}

#[test]
fn exact_joint_menu_script_cap_and_all_delayed_slots_include_empty_part_wrappers() {
    let config = caps();
    let bound = worst_case(&config).expect("checked bound");
    let mut out = Queue::uncapped();
    let meter = Meter::new();
    meter.start();
    let mut domain = Domain::configured(&config, 7, scripts(), menu()).expect("exact joint script/menu cap");
    let measured = meter.end();
    meter.check(measured, bound, &"all retained script/menu wrappers");
    let mut env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: config };
    for token in 1..=u64::from(config.calls) {
        meter.start();
        step(
            &mut domain,
            &env,
            Event::Call { reply_to: ReplyTo::new(Token::new(token)), call: Token::new(token), query: query() },
            &mut out,
        );
        let measured = meter.end();
        assert!(out.pop().is_none(), "every admitted answer waits for its actual timer");
        meter.check(measured, bound, &"empty-part wrapper scratch plus all delayed slots");
    }
    assert_eq!(domain.calls(), config.calls, "every slab and alarm slot is live");
    meter.start();
    step(
        &mut domain,
        &env,
        Event::Call { reply_to: ReplyTo::new(Token::new(99)), call: Token::new(99), query: query() },
        &mut out,
    );
    let measured = meter.end();
    let Request::Reply { result, .. } = out.pop().expect("full slab refuses once") else {
        panic!("the call ends in one reply")
    };
    assert_eq!(result, Err(Error::Overloaded.into()));
    meter.check(measured, bound, &"one-over live-slot refusal");
    env.now = Time::from_nanos(1_000_000_000);
    let mut terminals = 0_u32;
    while domain.is_due(env.now) {
        meter.start();
        fire(&mut domain, &env, &mut out);
        let measured = meter.end();
        let Request::Reply { result, .. } = out.pop().expect("one actual delayed terminal") else {
            panic!("the call ends in one reply")
        };
        let answer = result.expect("exact maximum answer cap admitted");
        assert_eq!(answer.parts.len(), ITEMS, "every empty Part wrapper reached output");
        assert!(answer.parts.iter().all(|part| matches!(part, Part::Text { text } if text.is_empty())));
        drop(answer);
        meter.check(measured, bound, &"actual delayed terminal output");
        terminals += 1;
    }
    assert_eq!(terminals, config.calls);
    domain.reclaim();
    assert_eq!(domain.calls(), 0);
    drop(core::mem::replace(&mut out, Queue::uncapped()));
    drop(domain);
    assert_eq!(meter.held(), 0, "all owned wrappers reclaimed after actual terminals");

    let tight = skein_fake_llm_domain::Config { script_bytes: config.script_bytes - 1, ..config };
    assert!(Domain::configured(&tight, 7, scripts(), menu()).is_err(), "one-byte tighter aggregate rejects");
    let mut env = Env {
        now: Time::ZERO,
        wall: Wall::EPOCH,
        limits: skein_fake_llm_domain::Config { answer_bytes: config.answer_bytes - 1, ..config },
    };
    let mut domain = Domain::configured(&env.limits, 7, scripts(), menu()).expect("same admitted input");
    step(
        &mut domain,
        &env,
        Event::Call { reply_to: ReplyTo::new(Token::new(1)), call: Token::new(1), query: query() },
        &mut out,
    );
    env.now = Time::from_nanos(1_000_000_000);
    fire(&mut domain, &env, &mut out);
    let Request::Reply { result, .. } = out.pop().expect("actual refusal terminal") else {
        panic!("the call ends in one reply")
    };
    assert_eq!(result, Err(Error::ContextTooLong.into()), "empty wrappers count toward answer admission");
}

#[test]
fn exact_query_cap_is_accepted_and_one_byte_less_returns_one_actual_refusal() {
    for fits in [true, false] {
        let config = caps();
        let config = Config { query_bytes: config.query_bytes - u32::from(!fits), ..config };
        let bound = worst_case(&config).expect("checked query-bound control");
        let mut out = Queue::uncapped();
        let meter = Meter::new();
        meter.start();
        let mut domain = Domain::try_scripted(&config, 7, scripts()).expect("unchanged admitted script");
        let measured = meter.end();
        meter.check(measured, bound, &"query-cap control startup");
        let mut env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: config };
        meter.start();
        step(
            &mut domain,
            &env,
            Event::Call { reply_to: ReplyTo::new(Token::new(17)), call: Token::new(17), query: query() },
            &mut out,
        );
        let measured = meter.end();
        assert!(out.pop().is_none(), "query admission preserves actual delayed settlement");
        meter.check(measured, bound, &"exact or one-byte-over query admission");
        assert_eq!(domain.calls(), 1);
        assert_eq!(domain.next_deadline(), Some(Time::from_nanos(1_000_000_000)));
        env.now = Time::from_nanos(1_000_000_000);
        meter.start();
        fire(&mut domain, &env, &mut out);
        let measured = meter.end();
        let Request::Reply { to, result } = out.pop().expect("one actual query terminal") else {
            panic!("the call ends in one reply")
        };
        assert_eq!(to.into_token(), Token::new(17));
        if fits {
            let answer = result.expect("exact query cap admits the same query");
            assert_eq!(answer.finish, Finish::Stop);
            assert_eq!(answer.parts.len(), ITEMS);
            drop(answer);
        } else {
            assert_eq!(result, Err(Error::ContextTooLong.into()), "only the query allowance changed");
        }
        assert!(out.pop().is_none(), "one terminal consumes the original reply right");
        meter.check(measured, bound, &"actual query success or refusal terminal");
        assert_eq!(domain.next_deadline(), None);
        domain.reclaim();
        assert_eq!(domain.calls(), 0);
        drop(core::mem::replace(&mut out, Queue::uncapped()));
        drop(domain);
        assert_eq!(meter.held(), 0, "query control leaves no retained or transferred ownership");
    }
}

#[test]
fn extreme_call_and_answer_configuration_has_no_representable_memory_bound() {
    assert!(worst_case(&caps()).is_some(), "ordinary configuration is representable");
    let config = Config { calls: u32::MAX, answer_bytes: u32::MAX, ..caps() };
    assert!(worst_case(&config).is_none(), "container and payload accounting must not wrap");
}

fn tool_scripts() -> Box<[Script]> {
    Box::new([Script {
        cue: b"cue".as_slice().into(),
        turns: Box::new([Turn {
            lines: (0..2)
                .map(|_| Line::Call {
                    name: vec![b'n'; 64].into_boxed_slice(),
                    arguments: vec![b'a'; 256].into_boxed_slice(),
                })
                .collect(),
            finish: Finish::ToolCalls,
            tokens: 500,
        }]),
    }])
}

fn tool_menu() -> Menu {
    Menu { arguments: Box::new([vec![b'a'; 256].into_boxed_slice()]), invalid: Box::new([]) }
}

#[test]
fn random_and_scripted_full_and_truncated_tool_scratch_stays_within_the_bound() {
    for scripted in [false, true] {
        let config = Config {
            calls: 2,
            query_bytes: 4096,
            script_bytes: 4096,
            answer_bytes: 2048,
            tool_rounds: 1,
            calls_per_answer: 2,
            ..caps()
        };
        let bound = worst_case(&config).expect("checked tool scratch bound");
        let mut out = Queue::uncapped();
        let meter = Meter::new();
        meter.start();
        let scripts = if scripted { tool_scripts() } else { Box::new([]) };
        let mut domain = Domain::configured(&config, 7, scripts, tool_menu()).expect("bounded caller tool data");
        let measured = meter.end();
        meter.check(measured, bound, &"caller tool script and menu startup");
        let mut env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: config };
        for (token, max_tokens) in [(1, 1000), (2, 0)] {
            meter.start();
            let mut query = query();
            query.tools = Box::new([ToolSpec {
                name: vec![b'n'; 64].into_boxed_slice(),
                description: Box::new([]),
                parameters: Box::new([]),
            }]);
            query.max_tokens = max_tokens;
            step(
                &mut domain,
                &env,
                Event::Call { reply_to: ReplyTo::new(Token::new(token)), call: Token::new(token), query },
                &mut out,
            );
            let measured = meter.end();
            assert!(out.pop().is_none(), "full and truncated tools both await actual fire");
            meter.check(measured, bound, &"full tool generation and simultaneous truncation scratch");
        }
        assert_eq!(domain.calls(), config.calls, "both delayed tool slots are held");
        env.now = Time::from_nanos(1_000_000_000);
        for token in 1..=2 {
            meter.start();
            fire(&mut domain, &env, &mut out);
            let measured = meter.end();
            let Request::Reply { to, result } = out.pop().expect("one actual tool terminal") else {
                panic!("the call ends in one reply")
            };
            assert_eq!(to.into_token(), Token::new(token), "actual output preserves its reply right");
            let answer = result.expect("both full and truncated caller tools are admitted");
            if token == 1 {
                assert_eq!(answer.finish, Finish::ToolCalls);
                assert!((1..=2).contains(&answer.parts.len()));
                if scripted {
                    assert_eq!(answer.parts.len(), 2, "both complete scripted calls were generated");
                }
                assert!(answer.usage.output.is_some_and(|count| count > 0));
            } else {
                assert_eq!(answer.finish, Finish::Length);
                assert_eq!(answer.parts.len(), 1, "the cut answer retains just its first call");
                assert_eq!(answer.usage.output, Some(0));
            }
            for part in &answer.parts {
                let Part::ToolCall { id, name, arguments } = part else {
                    panic!("actual tool-generation path must emit a tool call");
                };
                assert!(!id.is_empty(), "generated calls retain their actual identities");
                assert_eq!(name.as_ref(), &[b'n'; 64]);
                if token == 1 {
                    assert_eq!(arguments.as_ref(), &[b'a'; 256], "whole caller bytes reach the full answer");
                } else {
                    assert_eq!(arguments.as_ref(), b"aaa", "the cut retains the literal caller prefix");
                }
            }
            drop(answer);
            assert!(out.pop().is_none(), "each fired call emits only one terminal");
            meter.check(measured, bound, &"tool output ownership handed to its receiver");
        }
        assert_eq!(domain.next_deadline(), None);
        meter.start();
        domain.reclaim();
        let measured = meter.end();
        meter.check(measured, bound, &"tool slots reclaimed after actual outputs");
        assert_eq!(domain.calls(), 0);
        drop(core::mem::replace(&mut out, Queue::uncapped()));
        drop(domain);
        assert_eq!(meter.held(), 0, "caller scripts, menus, scratch and transferred tools are all released");
    }
}

#[test]
fn configured_codex_echo_request_and_entry_scratch_fit_the_peer_price() {
    let mut bounds = skein_llm_world::limits();
    bounds.output_items = 32;
    bounds.retained = 32768;
    bounds.skip = 32768;
    let observations = skein_llm_world::fake::ObservationLimits {
        heads: 1,
        head_bytes: 8192,
        events: 64,
        event_bytes: 65536,
        queries: 1,
        query_bytes: 65536,
        pending: 0,
        request: 32768,
        response_bytes: 65536,
    };
    let echo = skein_llm::openai::Echo { instructions: true, tools: true, attribution_bytes: 256 };
    let meter = Meter::new();
    meter.start();
    let mut input = skein_llm_world::call(1);
    input.prompt.instructions = vec![b'i'; 2048].into_boxed_slice();
    input.prompt.messages = (0..16)
        .map(|_| skein_llm::Message {
            role: skein_llm::Role::User,
            content: Box::new([skein_llm::Block::Text { text: vec![b'h'; 64].into_boxed_slice(), replay: None }]),
        })
        .collect();
    let extra = skein_llm_world::fake::extra_worst_case(
        &bounds,
        &observations,
        &input.endpoint,
        core::slice::from_ref(skein_llm_world::credential_of(&input)),
    )
    .unwrap();
    let bound = extra + skein_llm::client::call_worst_case(&bounds).unwrap() + 32768;
    let scripts = Box::new([Script {
        cue: input.prompt.instructions.clone(),
        turns: Box::new([Turn {
            lines: Box::new([Line::Text { text: b"done".as_slice().into() }]),
            finish: Finish::Stop,
            tokens: 1,
        }]),
    }]);
    let mut world = skein_llm_world::fake::Exchange::new_with_codex_echo(input, bounds, scripts, echo);
    world.observe(observations);
    let constructed = meter.end();
    meter.check(constructed, bound, &"bounded echo peer construction");
    meter.start();
    world.start();
    world.run();
    let receiving = meter.end();
    meter.check(receiving, bound, &"actual retained request and repeated native echo scratch");
    assert!(world.seen.iter().any(|event| matches!(event, skein_llm::client::Event::Completed { .. })));
    assert!(world.responses.len() > 8192, "configured attribution reaches the actual wire");
    meter.start();
    world.request(skein_llm::client::Request::Close);
    world.settle();
    drop(world);
    let settled = meter.end();
    meter.check(settled, bound, &"closed echo peer reclamation");
    assert_eq!(meter.held(), 0, "every echo/request owner releases its bounded storage");
}

#[test]
fn the_cache_table_reaches_its_bound_under_the_counted_domain_footprint() {
    use skein_fake_llm_domain::api::Caching;
    let config = Config { cache_entries: 3, query_bytes: caps().query_bytes + 4, ..caps() };
    let bound = worst_case(&config).unwrap();
    let mut out = Queue::uncapped();
    let meter = Meter::new();
    meter.start();
    let mut domain = Domain::new(&config, 5);
    let measured = meter.end();
    meter.check(measured, bound, &"fixed cache capacity at construction");
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: config };
    for scope in 1..=4 {
        let mut query = query();
        query.caching = Caching::Scope([scope; 16]);
        query.messages[0].parts = Box::new([Part::Text { text: b"abcd".as_slice().into() }]);
        let env = Env { now: Time::from_nanos(u64::from(scope) * 2_000_000_000), ..env };
        meter.start();
        step(
            &mut domain,
            &env,
            Event::Call {
                reply_to: ReplyTo::new(Token::new(u64::from(scope))),
                call: Token::new(u64::from(scope)),
                query,
            },
            &mut out,
        );
        let measured = meter.end();
        meter.check(measured, bound, &"full prefix table and oldest replacement");
        let later = Env { now: env.now.saturating_add(Duration::from_secs(1)), ..env };
        meter.start();
        fire(&mut domain, &later, &mut out);
        let measured = meter.end();
        let Request::Reply { result, .. } = out.pop().unwrap() else { panic!("the call ends in one reply") };
        let answer = result.unwrap();
        assert_eq!(answer.usage.cache_read, Some(0));
        drop(answer);
        domain.reclaim();
        meter.check(measured, bound, &"real delayed answer and retained full cache");
    }
    meter.start();
    drop(core::mem::replace(&mut out, Queue::uncapped()));
    drop(domain);
    let measured = meter.end();
    assert_eq!(measured.held(), 0, "the cache retains no ownership after its domain is dropped");
}

fn credential_list(bounds: &skein_llm::client::Limits) -> Vec<skein_llm::Credential> {
    let count = skein_fake_llm_protocol::provider::MAX_CREDENTIALS;
    let head = skein_llm_world::fake::limits(bounds).http.head;
    let length = usize::try_from(head / count).expect("credential fixture length");
    (0..count)
        .map(|index| {
            let mut value = vec![b'x'; length];
            value[0] = b'a' + u8::try_from(index).expect("sixteen fixture values");
            skein_llm::Credential::bearer(value.into())
        })
        .collect()
}

fn full_report_script() -> Box<[Script]> {
    let count = skein_fake_llm_protocol::provider::MAX_CREDENTIALS;
    let cue = b"full-report";
    let wrappers = size_of::<Script>()
        + cue.len()
        + size_of::<Turn>()
        + size_of::<Line>()
        + usize::try_from(count).expect("credential indices") * size_of::<u32>();
    let field = usize::try_from(skein_llm_world::fake::config().script_bytes).expect("script bytes") - wrappers;
    Box::new([Script {
        cue: cue.as_slice().into(),
        turns: Box::new([Turn {
            lines: Box::new([Line::Fail {
                error: Error::Unauthorized,
                report: skein_fake_llm_domain::api::Report {
                    code: Box::new([]),
                    request: Box::new([]),
                    field: vec![b'f'; field].into(),
                },
                credentials: (0..count).collect(),
            }]),
            finish: Finish::Stop,
            tokens: 1,
        }]),
    }])
}

fn full_list_world() -> (skein_llm_world::fake::Exchange, u64) {
    use skein_llm_world::fake::{Exchange, ObservationLimits, extra_worst_case};
    let mut bounds = skein_llm_world::limits();
    bounds.skip = 65536;
    let observations = ObservationLimits {
        heads: 1,
        head_bytes: 8192,
        events: 64,
        event_bytes: 65536,
        queries: 1,
        query_bytes: 65536,
        pending: 0,
        request: 32768,
        response_bytes: 65536,
    };
    let list = credential_list(&bounds);
    let mut input = skein_llm_world::public_call(1);
    input.prompt.instructions = b"full-report".as_slice().into();
    input.credential = Some(skein_llm::Credential::bearer(list[0].value.clone()));
    let endpoint = input.endpoint.clone();
    let price = skein_llm::client::call_worst_case(&bounds).expect("Client price")
        + extra_worst_case(&bounds, &observations, &endpoint, &list).expect("full list peer price");
    let machine = skein_llm::client::Client::prepare(input, &bounds).expect("full list Client admission");
    let mut list = list.into_iter();
    let mut world = Exchange::prepared(machine, endpoint, list.next(), bounds, full_report_script());
    for (index, credential) in list.enumerate() {
        assert_eq!(world.accept(credential), Ok(u32::try_from(index + 1).expect("accepted index")));
    }
    world.observe(observations);
    (world, price)
}

#[test]
fn the_largest_list_and_failing_line_fit_the_peer_price() {
    let meter = Meter::new();
    meter.start();
    let (mut world, price) = full_list_world();
    let construction = meter.end();
    meter.check(construction, price, &"sixteen credentials and full script report");
    meter.start();
    world.start();
    world.run();
    let receiving = meter.end();
    meter.check(receiving, price, &"full failing report through the Client");
    let failures: Vec<_> = world
        .seen
        .iter()
        .filter_map(|event| match event {
            skein_llm::client::Event::Failed { failure, .. } => Some(*failure),
            skein_llm::client::Event::Block { .. }
            | skein_llm::client::Event::Delta { .. }
            | skein_llm::client::Event::Completed { .. }
            | skein_llm::client::Event::Cancelled { .. }
            | skein_llm::client::Event::Reusable
            | skein_llm::client::Event::Close
            | skein_llm::client::Event::Closed => None,
        })
        .collect();
    assert_eq!(failures, [skein_llm::Failure::Unauthorized]);
    drop(failures);
    meter.start();
    world.request(skein_llm::client::Request::Close);
    world.settle();
    drop(world);
    let closed = meter.end();
    meter.check(closed, price, &"closed full-list world");
    assert_eq!(meter.held(), 0);
}
