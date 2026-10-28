//! Replay storage is charged at the bytes the completion holds (llm.md, 4.2).
use skein_llm::{Block, Cap, Declared, Failure, Limits, Provider, client};
use skein_llm_world::{TERMINAL, World, call, events, limits, response};

#[test]
fn standard_client_bound_excludes_per_item_replay_capacity() {
    // 64 MiB leaves headroom above the standard client's 51 MiB fixed state;
    // multiplying replay capacity by every output item exceeds this ceiling.
    const STANDARD_CLIENT_CEILING: u64 = 64 * 1024 * 1024;
    let declared = Declared {
        window: 272_000,
        output: 32_000,
        reasoning_item: 65_536,
        tool_payload: 40_960,
        calls_per_response: 16,
        conversations: 6,
        tools: 0,
        history_items: 1,
    };
    let derived = Limits::derive(&declared).unwrap();
    for (provider, receiving) in [(Provider::OpenAiCodex, 3_527_212), (Provider::Anthropic, 3_527_206)] {
        let mut bounds = derived.dialect(provider);
        bounds.http.request = 4096;
        bounds.http.read = 256;
        bounds.http.send = 31;
        bounds.sse.chunk = 128;
        let bound = client::worst_case(&bounds).unwrap();
        assert!(bound <= STANDARD_CLIENT_CEILING);
        assert_eq!(bounds.output_items, 34);
        assert_eq!(bounds.reasoning, 65_536);
        assert_eq!(bounds.receiving, receiving);
    }
}

fn replay_completion(payload: usize) -> World {
    let mut bounds = limits();
    bounds.answer = 64;
    bounds.reasoning = 256;
    bounds.receiving = 256;
    let done = format!(
        r#"{{"type":"response.output_item.done","output_index":0,"item":{{"id":"r","type":"reasoning","encrypted_content":"{}","summary":[]}}}}"#,
        "x".repeat(payload)
    );
    let documents = [
        r#"{"type":"response.output_item.added","output_index":0,"item":{"id":"r","type":"reasoning"}}"#,
        &done,
        r#"{"type":"response.output_item.added","output_index":1,"item":{"id":"m","type":"message"}}"#,
        r#"{"type":"response.output_item.done","output_index":1,"item":{"id":"m","type":"message","content":[{"type":"output_text","text":""}]}}"#,
        TERMINAL,
    ];
    let mut world = World::new(
        call(1),
        bounds,
        response(200, "Content-Type: text/event-stream\r\n", &events(&documents), false),
        1,
    );
    world.request(client::Request::Start);
    world.run();
    world.assert_once();
    world
}

#[test]
fn replay_at_the_receiving_share_is_held() {
    let world = replay_completion(33);
    let completion = world
        .seen
        .iter()
        .find_map(|event| match event {
            client::Event::Completed { completion, .. } => Some(completion),
            client::Event::Delta { .. }
            | client::Event::Block { .. }
            | client::Event::Failed { .. }
            | client::Event::Cancelled { .. }
            | client::Event::Reusable
            | client::Event::Close
            | client::Event::Closed => None,
        })
        .expect("replay fits the receiving reservation");
    let [Block::Reasoning { replay }, Block::Text { text, replay: Some(metadata) }] = completion.content.as_ref()
    else {
        panic!("reasoning followed by text")
    };
    assert_eq!(replay.value.document().len(), 11);
    assert_eq!(replay.value.document().text_len(), 73);
    assert!(text.is_empty());
    assert_eq!(metadata.value.document().len(), 4);
    assert_eq!(metadata.value.document().text_len(), 3);
    assert_eq!(world.machine.waiting(), client::Waiting::Idle);
}

#[test]
fn replay_one_byte_past_receiving_is_refused() {
    let world = replay_completion(34);
    assert!(world.seen.iter().any(|event| matches!(
        event,
        client::Event::Failed { failure: Failure::Limit { which: Cap::Answer, bound: 256 }, .. }
    )));
    assert!(!world.seen.iter().any(|event| matches!(event, client::Event::Completed { .. })));
}
