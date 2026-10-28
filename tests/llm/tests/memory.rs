//! Counting allocator verifies preparation and every driven call. The world
//! retains emitted values, so its measured total also conservatively includes
//! request/response bytes and the test's trace instead of subtracting them.
use skein_heap::{Counting, Meter, Span};
use skein_llm::client;
use skein_llm_world::{World, call, limits, text_response};

#[global_allocator]
static HEAP: Counting = Counting;

#[test]
fn codex_dense_request_encoding_uses_only_reserved_bytes_and_fixed_scratch() {
    dense_request_encoding(skein_llm::Provider::OpenAiCodex);
}

#[test]
fn anthropic_dense_request_encoding_uses_only_reserved_bytes_and_fixed_scratch() {
    dense_request_encoding(skein_llm::Provider::Anthropic);
}

fn dense_request_encoding(provider: skein_llm::Provider) {
    use skein_llm::{Block, Json, Message, Provider, Replay, Role, Tool};
    for count in [32, 4096] {
        let mut bounds = skein_llm_world::limits_for(provider);
        bounds.request = 131_072;
        bounds.reasoning = 131_072;
        let value = format!("{{\"values\":[{}]}}", vec!["null"; count].join(","));
        let document = skein_llm::DocumentLimits {
            bytes: bounds.request,
            strings: bounds.request,
            tokens: bounds.request,
            depth: bounds.depth,
        };
        let mut input = call(1);
        if provider == Provider::Anthropic {
            input.endpoint = skein_llm::Endpoint::anthropic();
            input.credential.as_mut().expect("fixture credential").account_id = Box::new([]);
        }
        input.prompt.tools = Box::new([Tool {
            name: b"read".to_vec().into(),
            description: Box::new([]),
            schema: Json::from_bytes(value.as_bytes(), &document).expect("dense schema fits owner admission"),
        }]);
        let mut blocks = vec![Block::ToolCall {
            id: b"call".to_vec().into(),
            name: b"read".to_vec().into(),
            arguments: value.as_bytes().to_vec().into(),
            replay: None,
        }];
        if provider == Provider::OpenAiCodex {
            let opaque = format!("{{\"type\":\"extension\",\"values\":[{}]}}", vec!["null"; count].join(","));
            blocks.push(Block::Reasoning {
                replay: Replay {
                    provider,
                    value: Json::from_bytes(opaque.as_bytes(), &document).expect("dense replay fits owner admission"),
                },
            });
        }
        input.prompt.messages = Box::new([Message { role: Role::Assistant, content: blocks.into_boxed_slice() }]);
        let original = input.prompt.clone();
        let scratch = client::encoding_worst_case(&bounds).expect("fixed scratch is representable");
        let measuring = Span::start();
        let measured =
            client::measure(&input.prompt, Some(skein_llm_world::credential_of(&input)), &input.endpoint, &bounds)
                .expect("dense request fits the whole window");
        let growth = measuring.end();
        let measuring_peak = growth.peak;
        assert!(
            growth.peak <= i64::try_from(scratch).expect("scratch fits i64"),
            "{provider:?}/{count}: measure peak {} exceeds fixed scratch {scratch}",
            growth.peak
        );
        assert_eq!(input.prompt, original, "the borrowed owner retains every document and byte");
        let bound = scratch
            + client::worst_case(&bounds).expect("client bound is representable")
            + u64::from(measured.head)
            + u64::from(measured.body);
        let preparing = Span::start();
        let prepared = client::Client::prepare(input, &bounds).expect("dense request prepares");
        let growth = preparing.end();
        assert!(
            growth.peak <= i64::try_from(bound).expect("prepared bound fits i64"),
            "{provider:?}/{count}: preparation peak {} exceeds reserved body and fixed state {bound}",
            growth.peak
        );
        assert_eq!(prepared.measured(), measured);
        eprintln!(
            "{provider:?}/{count}: scratch={scratch}, measurement peak={}, prepared peak={}, bound={bound}",
            measuring_peak, growth.peak
        );
        drop(prepared);
    }
}

// Bounds the largest preparation and streaming peak across both HTTP framings at seed 37.
const PREPARATION_STREAMING_PEAK_CEILING: u64 = 610_000;

#[test]
fn preparation_and_streaming_stay_under_the_peak_ceiling() {
    for chunked in [false, true] {
        let bounds = limits();
        let input = call(1);
        let span = Span::start();
        let prepared = client::Client::prepare(input, &bounds).ok().unwrap();
        let grown = span.end();
        assert!(
            grown.peak <= i64::try_from(PREPARATION_STREAMING_PEAK_CEILING).unwrap(),
            "preparation peak {} exceeds {PREPARATION_STREAMING_PEAK_CEILING}",
            grown.peak
        );
        drop(prepared);

        let meter = Meter::new();
        let wire = text_response(chunked);
        let mut world = World::new(call(1), bounds, wire, 37);
        world.fragmentation(1, 5);
        world.request(client::Request::Start);
        for _ in 0..100_000 {
            meter.start();
            let progress = world.tick(true);
            let measured = meter.end();
            assert!(
                measured.peak() <= PREPARATION_STREAMING_PEAK_CEILING,
                "total peak {} exceeds {PREPARATION_STREAMING_PEAK_CEILING}",
                measured.peak()
            );
            assert!(
                meter.held() <= PREPARATION_STREAMING_PEAK_CEILING,
                "including the world's small retained trace, live heap stays within {PREPARATION_STREAMING_PEAK_CEILING}"
            );
            if !progress {
                break;
            }
        }
        world.assert_once();
        world.request(client::Request::Close);
        world.settle();
        world.settle();
        world.assert_once();
        drop(world);
        assert_eq!(meter.held(), 0, "all client/world allocations released after settlement");
    }
}

// Bounds the largest construction and cancellation peak across all three stages at seed 19.
const CANCELLATION_PEAK_CEILING: u64 = 540_000;

#[test]
fn cancellation_releases_owned_buffers() {
    for stage in 0..3 {
        let bounds = limits();
        let meter = Meter::new();
        meter.start();
        let mut world = World::new(call(1), bounds, text_response(true), 19);
        if stage > 0 {
            world.request(client::Request::Start);
        }
        if stage == 2 {
            for _ in 0..50 {
                world.tick(true);
            }
        }
        world.request(client::Request::Cancel);
        world.settle();
        let measured = meter.end();
        assert!(measured.peak() <= CANCELLATION_PEAK_CEILING);
        assert!(meter.held() <= CANCELLATION_PEAK_CEILING);
        drop(world);
        assert_eq!(meter.held(), 0);
    }
}

// Bounds the largest error-body peak across JSON and non-JSON replies at seed 13.
const ERROR_BODY_PEAK_CEILING: u64 = 820_000;

#[test]
fn bounded_error_body_and_protocol_failure_release_all_storage() {
    let bounds = limits();
    for body in [br#"{"error":{"code":"rate_limit_exceeded","message":"later"}}"#.as_slice(), b"not json".as_slice()] {
        let meter = Meter::new();
        let wire = skein_llm_world::response(429, "Content-Type: application/json\r\nRetry-After: 9\r\n", body, false);
        let mut world = World::new(call(1), bounds, wire, 13);
        world.request(client::Request::Start);
        for _ in 0..100_000 {
            meter.start();
            let progress = world.tick(true);
            let measured = meter.end();
            assert!(measured.peak() <= ERROR_BODY_PEAK_CEILING);
            assert!(meter.held() <= ERROR_BODY_PEAK_CEILING);
            if !progress {
                break;
            }
        }
        world.settle();
        world.assert_once();
        drop(world);
        assert_eq!(meter.held(), 0);
    }
}

// Bounds the large schema, escaped request, reasoning and answer scenario at seed 17.
const LARGE_SCHEMA_PEAK_CEILING: u64 = 1_700_000;

#[test]
fn large_schema_request_reasoning_and_answer_stay_under_the_peak_ceiling() {
    let mut bounds = limits();
    bounds.output_items = 4;
    bounds.reasoning = 4096;
    let meter = Meter::new();
    let mut request = call(1);
    // Request escaping nearly doubles these instructions; a large schema
    // then takes the encoded request close to its body cap.
    request.prompt.instructions = vec![b'"'; 1400].into();
    let schema_wire =
        format!(r#"{{"type":"object","properties":{{"x":{{"type":"string","description":"{}"}}}}}}"#, "s".repeat(3000));
    request.prompt.tools = Box::new([skein_llm::Tool {
        name: b"large_tool".to_vec().into(),
        description: vec![b'd'; 128].into(),
        schema: skein_llm::openai::Json::from_bytes(schema_wire.as_bytes(), &bounds.native().document()).unwrap(),
    }]);
    let reasoning = format!(
        r#"{{"type":"response.output_item.done","output_index":0,"item":{{"id":"rs_1","type":"reasoning","encrypted_content":"{}","summary":[]}}}}"#,
        "r".repeat(3500)
    );
    let text = format!(
        r#"{{"type":"response.output_item.done","output_index":1,"item":{{"id":"msg_1","type":"message","content":[{{"type":"output_text","text":"{}"}}]}}}}"#,
        "t".repeat(3500)
    );
    let docs = [
        r#"{"type":"response.output_item.added","output_index":0,"item":{"id":"rs_1","type":"reasoning"}}"#,
        &reasoning,
        r#"{"type":"response.output_item.added","output_index":1,"item":{"id":"msg_1","type":"message"}}"#,
        &text,
        skein_llm_world::TERMINAL,
    ];
    let wire =
        skein_llm_world::response(200, "Content-Type: text/event-stream\r\n", &skein_llm_world::events(&docs), true);
    let mut world = World::new(request, bounds, wire, 17);
    world.fragmentation(127, 4);
    world.request(client::Request::Start);
    for _ in 0..100_000 {
        meter.start();
        let progress = world.tick(true);
        let measured = meter.end();
        assert!(measured.peak() <= LARGE_SCHEMA_PEAK_CEILING);
        assert!(meter.held() <= LARGE_SCHEMA_PEAK_CEILING);
        if !progress {
            break;
        }
    }
    world.assert_once();
    assert!(
        world
            .seen
            .iter()
            .any(|e| matches!(e, client::Event::Completed { completion, .. } if completion.content.len() == 2)),
        "{:?}",
        world.seen
    );
    // The cap is on the body as rendered, which the Codex route sends gzip.
    let sent = skein_llm_world::identity(&world.sent);
    let head = sent.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    assert!(sent.len() - head > 6000, "the request is close to its 8192-byte cap");
    drop(sent);
    world.request(client::Request::Close);
    world.settle();
    drop(world);
    drop(schema_wire);
    drop(reasoning);
    drop(text);
    assert_eq!(meter.held(), 0);
}

// Bounds the almost-full error buffer and fragmented request scenario at seed 17.
const PARTIAL_ERROR_PEAK_CEILING: u64 = 850_000;

#[test]
fn almost_full_error_buffer_and_partial_request_stay_under_the_peak_ceiling() {
    let bounds = limits();
    let meter = Meter::new();
    let detail = "x".repeat(3800);
    let body = format!(r#"{{"error":{{"code":"rate_limit_exceeded","message":"{detail}"}}}}"#);
    let wire = skein_llm_world::response(429, "Content-Type: application/json\r\n", body.as_bytes(), true);
    let mut world = World::new(call(1), bounds, wire, 17);
    world.fragmentation(31, 3);
    world.request(client::Request::Start);
    for _ in 0..100_000 {
        meter.start();
        let progress = world.tick(true);
        let measured = meter.end();
        assert!(measured.peak() <= PARTIAL_ERROR_PEAK_CEILING);
        assert!(meter.held() <= PARTIAL_ERROR_PEAK_CEILING);
        if !progress {
            break;
        }
    }
    world.assert_once();
    world.settle();
    drop(world);
    drop(body);
    drop(detail);
    assert_eq!(meter.held(), 0);
}

// Bounds three unknown items with dense empty-token replay storage at seed 3.
const UNKNOWN_ITEMS_PEAK_CEILING: u64 = 1_700_000;

#[test]
fn multiple_unknown_items_with_many_empty_tokens_stay_under_the_peak_ceiling() {
    let mut bounds = limits();
    bounds.output_items = 4;
    bounds.reasoning = 4096;
    let meter = Meter::new();
    // Near 1024 tokens per event and near 8192 bytes across the answer, while
    // Empty strings have no payload allocation; fixed records still cost bytes.
    let tiny_values = vec![r#""""#; 850].join(",");
    let mut documents = Vec::new();
    for index in 0..3 {
        documents.push(format!(r#"{{"type":"response.output_item.added","output_index":{index},"item":{{"id":"rs_{index}","type":"future_reasoning"}}}}"#));
        documents.push(format!(r#"{{"type":"response.output_item.done","output_index":{index},"item":{{"id":"rs_{index}","type":"future_reasoning","encrypted_content":"opaque","future":[{tiny_values}],"summary":[]}}}}"#));
    }
    documents.push(skein_llm_world::TERMINAL.to_owned());
    let refs: Vec<_> = documents.iter().map(String::as_str).collect();
    let wire =
        skein_llm_world::response(200, "Content-Type: text/event-stream\r\n", &skein_llm_world::events(&refs), true);
    let mut world = World::new(call(1), bounds, wire, 3);
    world.fragmentation(31, 3);
    world.request(client::Request::Start);
    for _ in 0..100_000 {
        meter.start();
        let progress = world.tick(true);
        let measured = meter.end();
        assert!(measured.peak() <= UNKNOWN_ITEMS_PEAK_CEILING);
        assert!(meter.held() <= UNKNOWN_ITEMS_PEAK_CEILING);
        if !progress {
            break;
        }
    }
    world.assert_once();
    let completed = world
        .seen
        .iter()
        .find_map(|e| if let client::Event::Completed { completion, .. } = e { Some(completion) } else { None })
        .unwrap();
    assert_eq!(completed.content.len(), 3);
    for block in &completed.content {
        let skein_llm::Block::Reasoning { replay } = block else { panic!("fixture only contains reasoning") };
        assert!(replay.value.document().len() > 850, "many fixed records exercise compact replay storage");
    }
    world.request(client::Request::Close);
    world.settle();
    drop(world);
    drop(refs);
    drop(documents);
    drop(tiny_values);
    assert_eq!(meter.held(), 0);
}

// Bounds thirty-two unknown arrays with dense compact records at seed 33.
const UNKNOWN_ARRAYS_PEAK_CEILING: u64 = 4_100_000;

#[test]
fn thirty_two_unknown_arrays_exercise_the_compact_record_ceiling() {
    let mut bounds = limits();
    bounds.output_items = 32;
    bounds.tokens = 2048;
    bounds.retained = 4096;
    bounds.reasoning = 4096;
    bounds.strings = 64;
    bounds.answer = 140_000;
    bounds.receiving = 1_048_576;
    bounds.request = 1024;
    let meter = Meter::new();
    let tiny_values = vec!["0"; 1900].join(",");
    let mut documents = Vec::new();
    for index in 0..32 {
        documents.push(format!(r#"{{"type":"response.output_item.added","output_index":{index},"item":{{"id":"rs_{index}","type":"future_reasoning"}}}}"#));
        documents.push(format!(r#"{{"type":"response.output_item.done","output_index":{index},"item":{{"id":"rs_{index}","type":"future_reasoning","encrypted_content":"opaque","future":[{tiny_values}],"summary":[]}}}}"#));
    }
    documents.push(skein_llm_world::TERMINAL.to_owned());
    let refs: Vec<_> = documents.iter().map(String::as_str).collect();
    let wire =
        skein_llm_world::response(200, "Content-Type: text/event-stream\r\n", &skein_llm_world::events(&refs), true);
    let mut world = World::new(call(1), bounds, wire, 33);
    world.fragmentation(251, 1);
    world.request(client::Request::Start);
    let mut most = 0;
    for _ in 0..100_000 {
        meter.start();
        let progress = world.tick(true);
        let measured = meter.end();
        most = most.max(measured.peak());
        assert!(
            measured.peak() <= UNKNOWN_ARRAYS_PEAK_CEILING,
            "total peak {} exceeds {UNKNOWN_ARRAYS_PEAK_CEILING}",
            measured.peak()
        );
        if !progress {
            break;
        }
    }
    world.assert_once();
    let completed = world
        .seen
        .iter()
        .find_map(|e| if let client::Event::Completed { completion, .. } = e { Some(completion) } else { None })
        .unwrap();
    assert_eq!(completed.content.len(), 32);
    for block in &completed.content {
        let skein_llm::Block::Reasoning { replay } = block else { panic!("fixture only contains reasoning") };
        assert!(replay.value.document().len() > 1900);
    }
    assert!(most > 2_000_000, "many short values exercise substantial compact record storage: {most}");
    world.request(client::Request::Close);
    world.settle();
    drop(world);
    drop(refs);
    drop(documents);
    drop(tiny_values);
    assert_eq!(meter.held(), 0);
}

// Bounds the largest signed-thinking, tool and replay peak over both framings and fragment sizes at seed 37.
const ANTHROPIC_REPLAY_PEAK_CEILING: u64 = 2_000_000;

#[test]
fn anthropic_signed_thinking_tool_input_and_replay_stay_under_the_peak_ceiling() {
    // Synthetic Messages documents exercise accumulation into signed replay
    // and tool arguments; they are not captured subscription traffic.
    for chunked in [false, true] {
        for fragment in [1, 251] {
            let mut bounds = limits();
            bounds.output_items = 4;
            bounds.reasoning = 4096;
            let input = memory_anthropic_call(1);
            let span = Span::start();
            let prepared = client::Client::prepare(input, &bounds).ok().unwrap();
            let grown = span.end();
            assert!(
                grown.peak <= i64::try_from(ANTHROPIC_REPLAY_PEAK_CEILING).unwrap(),
                "Anthropic preparation exceeds {ANTHROPIC_REPLAY_PEAK_CEILING}"
            );
            drop(prepared);

            let meter = Meter::new();
            let thinking = format!(
                r#"{{"type":"content_block_delta","index":0,"delta":{{"type":"thinking_delta","thinking":"{}"}}}}"#,
                "r".repeat(1500)
            );
            let signature = format!(
                r#"{{"type":"content_block_delta","index":0,"delta":{{"type":"signature_delta","signature":"{}"}}}}"#,
                "s".repeat(1500)
            );
            let tool_input = format!(
                r#"{{"type":"content_block_delta","index":1,"delta":{{"type":"input_json_delta","partial_json":"{{\"value\":\"{}\"}}"}}}}"#,
                "a".repeat(1800)
            );
            let text = format!(
                r#"{{"type":"content_block_delta","index":2,"delta":{{"type":"text_delta","text":"{}"}}}}"#,
                "t".repeat(1500)
            );
            let documents = [
                r#"{"type":"message_start","message":{"type":"message","role":"assistant","content":[],"usage":{"input_tokens":7,"output_tokens":1}}}"#,
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
                &thinking,
                &signature,
                r#"{"type":"content_block_stop","index":0}"#,
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tool_1","name":"read","input":{}}}"#,
                &tool_input,
                r#"{"type":"content_block_stop","index":1}"#,
                r#"{"type":"content_block_start","index":2,"content_block":{"type":"text","text":""}}"#,
                &text,
                r#"{"type":"content_block_stop","index":2}"#,
                r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":100}}"#,
                r#"{"type":"message_stop"}"#,
            ];
            let wire = skein_llm_world::response(
                200,
                "Content-Type: text/event-stream\r\n",
                &skein_llm_world::events(&documents),
                chunked,
            );
            let input = memory_anthropic_call(1);
            let mut world = World::new(input, bounds, wire, 37);
            world.fragmentation(fragment, 5);
            world.request(client::Request::Start);
            drive_anthropic_memory(&mut world, &meter, ANTHROPIC_REPLAY_PEAK_CEILING);
            world.assert_once();
            assert!(
                !world.seen.iter().any(|event| matches!(event, client::Event::Failed { .. })),
                "{:?}",
                world.seen.iter().find(|event| matches!(event, client::Event::Failed { .. }))
            );
            let completed =
                world
                    .seen
                    .iter()
                    .find_map(|event| {
                        if let client::Event::Completed { completion, .. } = event { Some(completion) } else { None }
                    })
                    .unwrap();
            assert_eq!(completed.content.len(), 3);
            assert!(matches!(&completed.content[0], skein_llm::Block::Reasoning { .. }));
            assert!(
                matches!(&completed.content[1], skein_llm::Block::ToolCall { arguments, .. } if arguments.len() > 1800)
            );
            let content = completed.content.clone();
            world.request(client::Request::Close);
            world.settle();
            drop(world);

            let mut replay = memory_anthropic_call(2);
            replay.prompt.messages = Box::new([skein_llm::Message { role: skein_llm::Role::Assistant, content }]);
            let span = Span::start();
            let prepared = client::Client::prepare(replay, &bounds).ok().unwrap();
            let grown = span.end();
            assert!(
                grown.peak <= i64::try_from(ANTHROPIC_REPLAY_PEAK_CEILING).unwrap(),
                "signed replay preparation exceeds {ANTHROPIC_REPLAY_PEAK_CEILING}"
            );
            assert!(
                meter.held() <= ANTHROPIC_REPLAY_PEAK_CEILING,
                "replay and fixture storage stays within {ANTHROPIC_REPLAY_PEAK_CEILING}"
            );
            drop(prepared);
            drop(thinking);
            drop(signature);
            drop(tool_input);
            drop(text);
            assert_eq!(meter.held(), 0, "Anthropic storage released after settlement and replay");
        }
    }
}

fn drive_anthropic_memory(world: &mut World, meter: &Meter, bound: u64) {
    for _ in 0..100_000 {
        meter.start();
        let progress = world.tick(true);
        let measured = meter.end();
        assert!(measured.peak() <= bound, "Anthropic total peak {} exceeds {bound}", measured.peak());
        assert!(meter.held() <= bound, "Anthropic live storage exceeds {bound}");
        if !progress {
            return;
        }
    }
    panic!("bounded Anthropic memory world stalled");
}

fn memory_anthropic_call(owner: u64) -> skein_llm::Call {
    let mut input = call(owner);
    input.endpoint = skein_llm::Endpoint::anthropic();
    input.credential = Some(skein_llm::Credential::anthropic(b"synthetic-oauth-token".to_vec().into()));
    input.prompt.affinity = None;
    input.prompt.max_output_tokens = Some(1024);
    input
}

#[test]
fn opaque_envelope_and_extended_thinking_transit_fit_counted_bounds() {
    for bytes in [32_u32, 2048] {
        let mut bounds = limits();
        bounds.reasoning = bytes;
        bounds.strings = bytes.max(4096);
        bounds.retained = 8192;
        let metadata = format!(
            r#"{{"type":"thinking","thinking":"{}","signature":"s","extension":{{"signed":true}}}}"#,
            "x".repeat(usize::try_from(bytes.saturating_sub(96)).expect("bounded payload"))
        );
        let replay = skein_llm::Replay {
            provider: skein_llm::Provider::Anthropic,
            value: skein_llm::Json::from_bytes(metadata.as_bytes(), &bounds.native().document())
                .expect("bounded extended opaque value"),
        };
        let bound = skein_llm::replay_worst_case(&bounds.native().document()).expect("checked replay transit bound");
        let span = Span::start();
        let encoded = replay.to_bytes(&bounds.native().document());
        match encoded {
            Ok(encoded) => {
                let decoded = skein_llm::Replay::from_bytes(&encoded, &bounds.native().document())
                    .expect("complete envelope restores");
                assert_eq!(decoded, replay, "counted transit preserves every extension");
                let grown = span.end();
                assert!(
                    grown.peak <= i64::try_from(bound).expect("bounded signed comparison"),
                    "replay transit peak {} exceeds {bound}",
                    grown.peak
                );
                drop(decoded);
                drop(encoded);
            }
            Err(skein_llm::Error::Limit { .. }) => {
                let grown = span.end();
                assert!(
                    grown.peak <= i64::try_from(bound).expect("bounded signed comparison"),
                    "refused envelope peak {} exceeds {bound}",
                    grown.peak
                );
            }
            Err(skein_llm::Error::Invalid | skein_llm::Error::Unsupported) => panic!("fixture metadata is valid"),
        }
    }
}

// Bounds the scripted byte peer and client conveying the script text cap.
const SCRIPTED_PEER_PEAK_CEILING: u64 = 2_100_000;

#[test]
fn scripted_byte_peer_and_client_stay_under_their_peak_ceiling() {
    use skein_fake_llm_domain::api::{Finish, Line, Script, Turn};
    let mut bounds = limits();
    bounds.strings = 4096; // This story reaches the fake domain's scripted text cap.
    // A string at its 4096-byte cap is emitted inside a larger JSON event and
    // a `data: ` line. Admit the entire document plus its six framing bytes.
    bounds.skip = bounds.retained.checked_add(6).expect("bounded data-line framing");
    let mut input = call(1);
    input.prompt.instructions = b"maximum-script".as_slice().into();
    let scripts = Box::new([Script {
        cue: b"maximum-script".as_slice().into(),
        turns: Box::new([Turn {
            lines: Box::new([Line::Text {
                text: vec![b'x'; usize::try_from(bounds.strings).expect("bounded maximum answer")].into(),
            }]),
            finish: Finish::Stop,
            tokens: 1000,
        }]),
    }]);
    let span = Span::start();
    let mut world = skein_llm_world::fake::Exchange::new(input, bounds, scripts);
    world.start();
    world.run();
    let grown = span.end();
    let bound = SCRIPTED_PEER_PEAK_CEILING;
    assert!(
        grown.peak <= i64::try_from(bound).expect("bounded comparison"),
        "composed actual Client/peer peak {} exceeds {bound}",
        grown.peak
    );
    assert_eq!(world.queries.len(), 1, "maximum payload reached actual peer");
    assert!(
        world.seen.iter().any(|event| matches!(event, client::Event::Completed { .. })),
        "maximum payload reached actual Client terminal: {:?}",
        world.seen
    );
    let actual: Vec<_> = world
        .seen
        .iter()
        .filter_map(|event| match event {
            client::Event::Completed { completion, .. } => Some(completion),
            client::Event::Block { .. }
            | client::Event::Delta { .. }
            | client::Event::Failed { .. }
            | client::Event::Cancelled { .. }
            | client::Event::Reusable
            | client::Event::Close
            | client::Event::Closed => None,
        })
        .collect();
    let [completion] = actual.as_slice() else {
        panic!("exactly one maximum-payload completion");
    };
    let [skein_llm::Block::Text { text, .. }] = &*completion.content else {
        panic!("whole maximum scripted text");
    };
    assert_eq!(text.len(), usize::try_from(bounds.strings).expect("same maximum text cap"));
    assert!(text.iter().all(|byte| *byte == b'x'), "whole maximum payload was conveyed");
}

// Bounds preparation and fragmented reception of an oversized call at seed 3.
const OVERSIZED_CALL_PEAK_CEILING: u64 = 540_000;

#[test]
fn an_oversized_call_completes_under_the_peak_ceiling() {
    use skein_llm::{Block, openai};
    let mut bounds = limits();
    bounds.input = 3;
    let events = [
        openai::Event::ToolAdded {
            index: 0,
            id: b"i".as_slice().into(),
            call_id: b"c".as_slice().into(),
            name: b"read".as_slice().into(),
            arguments: Box::new([]),
        },
        openai::Event::ArgumentsDelta { index: 0, delta: vec![b'x'; 128].into() },
        openai::Event::Done {
            index: 0,
            item: openai::Item::FunctionCall {
                id: b"i".as_slice().into(),
                call_id: b"c".as_slice().into(),
                name: b"read".as_slice().into(),
                arguments: vec![b'x'; 128].into(),
            },
        },
        openai::Event::Completed { stop: openai::Stop::ToolUse, usage: openai::Usage::NONE },
    ];
    let meter = Meter::new();
    let mut body = Vec::new();
    for event in &events {
        body.extend_from_slice(b"data: ");
        body.extend_from_slice(&openai::encode_event(event, &bounds.native()).expect("bounded provider event"));
        body.extend_from_slice(b"\n\n");
    }
    let wire = skein_llm_world::response(200, "Content-Type: text/event-stream\r\n", &body, false);
    drop(body);
    meter.start();
    let mut world = World::new(call(1), bounds, wire, 3);
    world.request(client::Request::Start);
    let measured = meter.end();
    meter.check(measured, OVERSIZED_CALL_PEAK_CEILING, &"oversize preparation");
    for _ in 0..100_000 {
        meter.start();
        let progress = world.tick(true);
        let measured = meter.end();
        meter.check(measured, OVERSIZED_CALL_PEAK_CEILING, &"oversize actual fragmented event");
        if !progress {
            break;
        }
    }
    world.assert_once();
    let block = world
        .seen
        .iter()
        .find_map(|event| match event {
            client::Event::Completed { completion, .. } => completion.content.first(),
            client::Event::Delta { .. }
            | client::Event::Block { .. }
            | client::Event::Failed { .. }
            | client::Event::Cancelled { .. }
            | client::Event::Reusable
            | client::Event::Close
            | client::Event::Closed => None,
        })
        .unwrap();
    skein_llm_world::assert_tool_stream(&world.seen, 0, block);
    assert!(matches!(block, Block::Oversize { bytes: 128, .. }));
    world.request(client::Request::Close);
    world.settle();
    drop(world);
    assert_eq!(meter.held(), 0, "oversize settlement releases owned buffers");
}

#[test]
fn replay_at_its_cap_owns_only_shared_text_and_fixed_records() {
    let raw =
        format!("{{\"type\":\"reasoning\",\"summary\":[{}],\"encrypted_content\":\"x\"}}", vec!["\"\""; 512].join(","));
    let mut bounded = limits().native();
    bounded.retained = u32::try_from(raw.len()).unwrap();
    bounded.reasoning = bounded.retained;
    bounded.tokens = 1024;
    let admitted = skein_llm::Replay {
        provider: skein_llm::Provider::OpenAiCodex,
        value: skein_llm::Json::from_bytes(raw.as_bytes(), &bounded.document()).unwrap(),
    };
    let wire = admitted.to_bytes(&bounded.document()).unwrap();
    assert_eq!(wire.len(), raw.len() + usize::try_from(skein_llm::REPLAY_HEADER_BYTES).unwrap());
    drop(admitted);
    let meter = Meter::new();
    let replay = skein_llm::Replay::from_bytes(&wire, &bounded.document()).unwrap();
    let document = replay.value.document();
    let owned = u64::from(document.text_len())
        + u64::from(document.len()) * u64::try_from(size_of::<skein_json::Compact>()).unwrap();
    assert_eq!(meter.held(), owned, "one shared text allocation and one fixed-record allocation");
    let cloned = replay.clone();
    assert_eq!(meter.held(), owned * 2, "clone has the same compact storage");
    drop(cloned);
    drop(replay);
    assert_eq!(meter.held(), 0, "compact replay allocations release together");
}

fn selective_peak(size: usize, call_arguments: bool, ceiling: u64) -> (u64, u64) {
    // A quarter MiB across the four independently ignored echo paths keeps
    // the ignored fields far larger than their retained limits.
    let text = "x".repeat(if call_arguments { size } else { size / 4 });
    let documents = if call_arguments {
        vec![
            r#"{"type":"response.output_item.added","output_index":0,"item":{"id":"i","call_id":"c","name":"read","type":"function_call","arguments":""}}"#.to_owned(),
            format!(r#"{{"type":"response.output_item.done","output_index":0,"item":{{"arguments":"{text}","id":"i","call_id":"c","name":"read","type":"function_call"}}}}"#),
            skein_llm_world::TERMINAL.to_owned(),
        ]
    } else {
        vec![format!(
            r#"{{"type":"response.completed","response":{{"output":["{text}"],"instructions":"{text}","tools":[{{"description":"{text}"}}],"status":"completed","usage":{{"input_tokens":1,"output_tokens":1,"attribution":{{"echo":"{text}"}}}}}}}}"#
        )]
    };
    let refs: Vec<_> = documents.iter().map(String::as_str).collect();
    let wire =
        skein_llm_world::response(200, "Content-Type: text/event-stream\r\n", &skein_llm_world::events(&refs), false);
    drop(refs);
    drop(documents);
    drop(text);
    let mut bounds = limits();
    bounds.tokens = 64;
    bounds.retained = 512;
    bounds.strings = 64;
    bounds.input = 3;
    bounds.reasoning = 128;
    bounds.skip = 8_000_000;
    // Only client preparation and calls are metered. Transport, observations,
    // and cloned handoffs grow outside; original outputs are handed out.
    let meter = Meter::new();
    meter.start();
    let machine = client::Client::prepare(call(1), &bounds).expect("fixture admitted");
    let mut peak = meter.check(meter.end(), ceiling, &"selective preparation");
    let outside = Span::start();
    let mut world = World::prepared(machine, bounds, wire, 77);
    world.fragmentation(256, 31);
    let mut outside = outside.end().net;
    selective_step(
        &meter,
        &mut world,
        skein_llm_world::Turn::Down(client::Request::Start),
        ceiling,
        &mut outside,
        &mut peak,
    );
    for _ in 0..200_000 {
        let transport = Span::start();
        let turn = world.next_turn(true);
        outside += transport.end().net;
        match turn {
            skein_llm_world::Turn::Done => break,
            skein_llm_world::Turn::Waiting => {}
            turn @ (skein_llm_world::Turn::Up(_) | skein_llm_world::Turn::Down(_) | skein_llm_world::Turn::Resume) => {
                selective_step(&meter, &mut world, turn, ceiling, &mut outside, &mut peak);
            }
        }
    }
    world.assert_once();
    assert!(world.seen.iter().any(|event| matches!(event, client::Event::Completed { .. })), "{:?}", world.seen);
    if call_arguments {
        assert!(world.seen.iter().any(|event| matches!(event, client::Event::Block { block: skein_llm::Block::Oversize { bytes, .. }, .. } if *bytes == u64::try_from(size).expect("fixture size fits u64"))));
    }
    let held = u64::try_from(i64::try_from(meter.held()).expect("held fits") - outside).expect("client owns heap");
    world.request(client::Request::Close);
    world.settle();
    (peak, held)
}

/// Temporary output queues are handed out after each client call, so their
/// buffers and payloads are removed by `Meter::check`. Copies belong to the
/// world, measured separately, including its growing transport and trace.
fn selective_step(
    meter: &Meter,
    world: &mut World,
    turn: skein_llm_world::Turn,
    ceiling: u64,
    outside: &mut i64,
    peak: &mut u64,
) {
    use skein_lib::Queue;
    use skein_lib::stream::Up;
    use skein_llm_world::Turn;

    if let Turn::Up(Up::Bytes(bytes)) = &turn {
        *outside -= i64::try_from(bytes.len()).expect("input bytes fit");
    }
    let mut above = Queue::with_capacity(client::MAX_OUT.above);
    let mut below = Queue::with_capacity(client::MAX_OUT.below);
    meter.start();
    match turn {
        Turn::Up(event) => client::up(&mut world.machine, &world.env, event, &mut above, &mut below),
        Turn::Down(request) => client::down(&mut world.machine, &world.env, request, &mut above, &mut below),
        Turn::Resume => client::resume(&mut world.machine, &world.env, &mut above, &mut below),
        Turn::Waiting | Turn::Done => unreachable!("only client calls are metered"),
    }
    let measured = meter.end();
    let mut copies_above = Queue::with_capacity(above.capacity());
    let mut copies_below = Queue::with_capacity(below.capacity());
    let mut handed = 0;
    while let Some(event) = above.pop() {
        let copying = Span::start();
        copies_above.push(event.clone());
        handed += copying.end().net;
        drop(event);
    }
    while let Some(request) = below.pop() {
        let copying = Span::start();
        let copy = match &request {
            skein_lib::stream::Down::Demand { read, room } => {
                skein_lib::stream::Down::Demand { read: *read, room: *room }
            }
            skein_lib::stream::Down::Send(bytes) => skein_lib::stream::Down::Send(bytes.clone()),
            skein_lib::stream::Down::Finish => skein_lib::stream::Down::Finish,
        };
        copies_below.push(copy);
        handed += copying.end().net;
        drop(request);
    }
    drop((above, below));
    // The named ceiling covers client growth; world-owned storage stays outside.
    let total_ceiling = i64::try_from(ceiling).expect("ceiling fits") + *outside;
    let own = meter.check(measured, u64::try_from(total_ceiling).expect("positive ceiling"), &"selective client call");
    let own = i64::try_from(own).expect("peak fits") - *outside;
    *peak = (*peak).max(u64::try_from(own).expect("client owns its heap"));
    *outside += handed;
    let observing = Span::start();
    world.outputs(copies_above, copies_below);
    *outside += observing.end().net;
    let held = i64::try_from(meter.held()).expect("held fits") - *outside;
    assert!(held >= 0 && held <= i64::try_from(ceiling).expect("ceiling fits"));
}

// Bounds the largest peak for discarded echoes over all three fixture sizes at seed 77.
const SELECTIVE_ECHO_PEAK_CEILING: u64 = 71_000;

#[test]
fn selective_echo_storage_differs_only_by_bounded_cold_growth() {
    // The SSE reader's data Intake (128-byte cap) keeps its grown, empty buffer.
    // Its growth from 32 to 128 bytes bounds the difference, never the echo's size.
    // A quarter above the measured 96-byte difference, rounded to two digits.
    const COLD_GROWTH: u64 = 120;
    let short = selective_peak(64, false, SELECTIVE_ECHO_PEAK_CEILING);
    let middle = selective_peak(4096, false, SELECTIVE_ECHO_PEAK_CEILING);
    let long = selective_peak(1 << 18, false, SELECTIVE_ECHO_PEAK_CEILING);
    assert!(middle.0 <= short.0 + COLD_GROWTH);
    assert!(long.0 <= short.0 + COLD_GROWTH);
    assert!(middle.1 <= short.1 + COLD_GROWTH, "discarded echoes retain no event buffer");
    assert!(long.1 <= short.1 + COLD_GROWTH, "discarded echoes retain no event buffer");
}

// Bounds the largest peak for discarded tool arguments over both fixture sizes at seed 77.
const SELECTIVE_ARGUMENT_PEAK_CEILING: u64 = 71_000;

#[test]
fn selective_oversize_call_high_water_is_independent_of_the_argument_size() {
    let short = selective_peak(64, true, SELECTIVE_ARGUMENT_PEAK_CEILING);
    let long = selective_peak(1 << 18, true, SELECTIVE_ARGUMENT_PEAK_CEILING);
    assert_eq!(short.0, long.0, "discarded arguments retain only their length");
}

#[test]
fn measurement_refuses_large_starts_without_copying_their_owned_bytes() {
    for anthropic in [false, true] {
        let mut input = call(1);
        if anthropic {
            input.endpoint = skein_llm::Endpoint::anthropic();
            input.credential = Some(skein_llm::Credential::anthropic(b"synthetic-token".as_slice().into()));
        }
        input.prompt.instructions = vec![b'x'; 1024 * 1024].into();
        let bounds = limits();
        let span = Span::start();
        let result =
            client::measure(&input.prompt, Some(skein_llm_world::credential_of(&input)), &input.endpoint, &bounds);
        let measured = span.end();
        assert!(result.is_err());
        assert!(
            measured.peak < i64::try_from(input.prompt.instructions.len()).unwrap(),
            "only bounded filter-planning scratch is allocated; the oversized prompt is never copied or encoded"
        );
    }
}

#[test]
fn twenty_five_thousand_history_items_allocate_only_the_declared_translation_tables() {
    use skein_llm::{Block, Declared, Limits, Message, Provider, Role, openai};
    let declared = Declared { window: 272_000, tools: 8, history_items: 25_000, ..skein_llm_world::TINY_DECL };
    let mut bounds = Limits::derive(&declared).expect("large model with owner counts").dialect(Provider::OpenAiCodex);
    bounds.http.request = 4096;
    bounds.http.read = 256;
    bounds.http.send = 256;
    bounds.sse.chunk = 256;
    let mut input = call(1);
    input.prompt.messages = Box::new([Message {
        role: Role::User,
        content: vec![Block::Text { text: Box::new([]), replay: None }; 25_000].into(),
    }]);
    // Translation owns native table capacity and may overlap its exact boxed
    // result. The owner's prompt is outside this span; empty text allocates no
    // payload. Small fixed writer/header scratch is independently bounded.
    let tables = skein_lib::List::<openai::Input>::worst_case(declared.history_items).unwrap()
        + skein_lib::List::<openai::Tool>::worst_case(declared.tools).unwrap();
    let bound = 2 * tables + 64 * 1024;
    let span = Span::start();
    let measured =
        client::measure(&input.prompt, Some(skein_llm_world::credential_of(&input)), &input.endpoint, &bounds)
            .expect("all declared items fit");
    let grown = span.end();
    assert!(measured.body <= bounds.request);
    assert!(
        grown.peak <= i64::try_from(bound).unwrap(),
        "translation peak {} exceeds owner table bound {bound}",
        grown.peak
    );
    eprintln!(
        "history items={} tools={} translation peak={} bound={bound}",
        declared.history_items, declared.tools, grown.peak
    );
}

// Bounds a full report with each diagnostic field at its 128-byte cap at seed 1.
const FULL_REPORT_PEAK_CEILING: u64 = 820_000;

#[test]
fn a_full_report_stays_under_the_peak_ceiling() {
    // llm.md 4.2: each report value may hold 128 bytes.
    const REPORT_BOUND: usize = 128;
    let bounds = limits();
    let body = format!(
        r#"{{"error":{{"code":"{}","message":"spent","param":"{}"}}}}"#,
        "c".repeat(REPORT_BOUND),
        "f".repeat(REPORT_BOUND)
    );
    let headers = format!("x-oai-request-id: {}\r\n", "r".repeat(REPORT_BOUND));
    let meter = Meter::new();
    let source = skein_llm_world::response(429, &headers, body.as_bytes(), false);
    let mut world = World::new(call(1), bounds, source, 1);
    world.request(client::Request::Start);
    loop {
        meter.start();
        let progress = world.tick(true);
        let measured = meter.end();
        assert!(
            measured.peak() <= FULL_REPORT_PEAK_CEILING,
            "peak {} exceeds {FULL_REPORT_PEAK_CEILING}",
            measured.peak()
        );
        if !progress {
            break;
        }
    }
    world.assert_once();
    drop(world);
    assert_eq!(meter.held(), 0);
}
