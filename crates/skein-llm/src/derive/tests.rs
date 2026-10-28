//! Provider headroom and the actual HTTP reader's byte/field boundaries.
#![expect(clippy::disallowed_types, reason = "ordinary unit fixtures build independent HTTP heads")]
#![expect(clippy::disallowed_methods, reason = "ordinary unit fixtures own their wire bytes")]
#![expect(clippy::disallowed_macros, reason = "tests embed archives and format independent HTTP fields")]
#![expect(clippy::arithmetic_side_effects, reason = "trusted fixture sizes use ordinary test arithmetic")]

use core::fmt::Write as _;

use super::{Declared, Limits};
use crate::{DocumentLimits, Json, Provider, client};
use skein_http::client as http;
use skein_lib::stream::{Down, Up};
use skein_lib::{Env, Intake, Queue, Time, Wall};

const DECLARED: Declared = Declared {
    window: 64,
    output: 32,
    reasoning_item: 48,
    tool_payload: 8,
    calls_per_response: 3,
    conversations: 2,
    tools: 8,
    history_items: 16,
};

#[test]
fn every_archived_head_fits_within_a_quarter_of_its_derived_bounds() {
    let captures: &[(Provider, &[u8])] = &[
        (Provider::OpenAiCodex, include_bytes!("../openai/fixtures/single-text/response.headers")),
        (Provider::OpenAiCodex, include_bytes!("../openai/fixtures/tool-call/response.headers")),
        (Provider::OpenAiCodex, include_bytes!("../openai/fixtures/tool-result-final/response.headers")),
        (Provider::Anthropic, include_bytes!("../anthropic/fixtures/single-text/response.headers")),
        (Provider::Anthropic, include_bytes!("../anthropic/fixtures/tool-call/response.headers")),
        (Provider::Anthropic, include_bytes!("../anthropic/fixtures/parallel-tool-calls/response.headers")),
        (Provider::Anthropic, include_bytes!("../anthropic/fixtures/tool-result-final/response.headers")),
    ];
    for &(provider, source) in captures {
        let parsed =
            Json::from_bytes(source, &DocumentLimits { bytes: 16384, strings: 16384, tokens: 4096, depth: 16 })
                .expect("archived header wrapper");
        let document = parsed.document();
        let mut bytes = b"HTTP/1.1 200 OK\r\n\r\n".len();
        let mut fields = 0;
        for index in 0..document.len() {
            let key = document.token(index).unwrap();
            if key.kind != skein_json::Kind::Key {
                continue;
            }
            match document.text(key).unwrap() {
                b"name" => {
                    fields += 1;
                    bytes += document.text(document.token(index + 1).unwrap()).unwrap().len() + 2;
                }
                b"value" => bytes += document.text(document.token(index + 1).unwrap()).unwrap().len() + 2,
                _ => {}
            }
        }
        let limits = Limits::derive(&DECLARED).unwrap().dialect(provider);
        assert!(bytes <= usize::try_from(limits.http.head / 4).unwrap(), "{provider:?}: {bytes}");
        assert!(fields <= limits.http.headers / 4, "{provider:?}: {fields}");
    }
}

fn response_head(bytes: u32, fields: u32) -> Vec<u8> {
    let mut head = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nX-Padding: ".to_vec();
    let mut extensions = String::new();
    for index in 0..fields - 2 {
        write!(extensions, "X-{index}: x\r\n").unwrap();
    }
    let fixed = head.len() + 4 + extensions.len();
    head.resize(usize::try_from(bytes).unwrap() - fixed + head.len(), b'x');
    head.extend_from_slice(b"\r\n");
    head.extend_from_slice(extensions.as_bytes());
    head.extend_from_slice(b"\r\n");
    assert_eq!(head.len(), usize::try_from(bytes).unwrap());
    head
}

fn read_head(limits: http::Limits, wire: &[u8]) -> Result<http::Response, http::Error> {
    let mut machine = http::Client::new(&limits);
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits };
    let mut above = Queue::uncapped();
    let mut below = Queue::uncapped();
    let mut intake = Intake::with_capacity(u32::try_from(wire.len()).unwrap());
    intake.append(wire).unwrap();
    http::down(
        &mut machine,
        &env,
        http::Request::Call(http::Call {
            method: http::Method::Get,
            target: b"/".as_slice().into(),
            headers: Box::new([skein_http::Header {
                name: b"Host".as_slice().into(),
                value: b"fixture".as_slice().into(),
            }]),
            body: http::Body::None,
            close: false,
        }),
        &mut above,
        &mut below,
    );
    for _ in 0..1000_u32 {
        if let Some(event) = above.pop() {
            match event {
                http::Event::Response(response) => return Ok(response),
                http::Event::Failed(error) => return Err(error),
                other
                @ (http::Event::Body(_) | http::Event::Upload(_) | http::Event::Done(_) | http::Event::Closed) => {
                    panic!("unexpected head event {other:?}")
                }
            }
        }
        let request = below.pop().expect("a head demands its next line");
        match request {
            Down::Demand { read, room } => {
                let event = if room > 0 { Up::Room } else { Up::Bytes(intake.meet(read).expect("complete head")) };
                http::up(&mut machine, &env, event, &mut above, &mut below);
            }
            Down::Send(_) => {}
            Down::Finish => panic!("HTTP never finishes its lower stream"),
        }
    }
    panic!("bounded head progresses");
}

#[test]
fn derived_response_heads_accept_both_bounds_and_refuse_one_byte_or_field_over() {
    for provider in [Provider::OpenAiCodex, Provider::Anthropic] {
        let mut limits = Limits::derive(&DECLARED).unwrap().dialect(provider).http;
        limits.request = 256;
        limits.read = 256;
        limits.send = 256;
        let response = read_head(limits, &response_head(limits.head, limits.headers)).unwrap();
        assert_eq!(response.headers.len(), usize::try_from(limits.headers).unwrap());
        assert_eq!(read_head(limits, &response_head(limits.head + 1, limits.headers)), Err(http::Error::HeadTooLong));
        assert_eq!(
            read_head(limits, &response_head(limits.head, limits.headers + 1)),
            Err(http::Error::TooManyHeaders)
        );
    }
}

#[test]
fn headroom_flows_into_fixed_client_memory_without_changing_the_call_reservation() {
    for (provider, head, fields) in [(Provider::OpenAiCodex, 1935, 42), (Provider::Anthropic, 1435, 31)] {
        let mut limits = Limits::derive(&DECLARED).unwrap().dialect(provider);
        limits.http.request = 256;
        limits.http.read = 256;
        limits.http.send = 256;
        limits.sse.chunk = 128;
        let mut captured = limits;
        captured.http.head = head;
        captured.http.headers = fields;
        let delta = http::worst_case(&limits.http).unwrap() - http::worst_case(&captured.http).unwrap();
        assert!(delta > 0);
        assert_eq!(client::worst_case(&limits).unwrap() - client::worst_case(&captured).unwrap(), delta);
        assert_eq!(client::largest_reservation(&limits), client::largest_reservation(&captured));
    }
}

#[test]
fn owner_declared_counts_accept_the_boundary_and_refuse_one_more_by_name() {
    let declared = Declared { tools: 2, history_items: 3, ..DECLARED };
    let derived = Limits::derive(&declared).unwrap();
    for provider in [Provider::OpenAiCodex, Provider::Anthropic] {
        let mut bounds = derived.dialect(provider);
        bounds.http.request = 4096;
        bounds.http.read = 256;
        bounds.http.send = 256;
        bounds.sse.chunk = 256;
        let endpoint = match provider {
            Provider::OpenAiCodex | Provider::OpenAi => crate::Endpoint::codex(),
            Provider::Anthropic => crate::Endpoint::anthropic(),
        };
        let credential = match provider {
            Provider::OpenAiCodex | Provider::OpenAi => crate::Credential {
                scheme: crate::Scheme::Bearer,
                value: b"fake".as_slice().into(),
                account_id: b"fake".as_slice().into(),
            },
            Provider::Anthropic => crate::Credential::anthropic(b"fake".as_slice().into()),
        };
        let schema = Json::from_bytes(b"{}", &DocumentLimits { bytes: 2, strings: 2, depth: 2, tokens: 2 }).unwrap();
        let tool = crate::Tool { name: b"t".as_slice().into(), description: Box::new([]), schema };
        let block = crate::Block::Text { text: Box::new([]), replay: None };
        let mut prompt = crate::Prompt {
            model: b"fixture".as_slice().into(),
            instructions: Box::new([]),
            tools: alloc::vec![tool.clone(); 2].into(),
            messages: Box::new([crate::Message {
                role: crate::Role::User,
                content: alloc::vec![block.clone(); 3].into(),
            }]),
            reasoning_effort: None,
            affinity: None,
            choice: crate::ToolChoice::Auto,
            max_output_tokens: None,
        };
        client::measure(&prompt, Some(&credential), &endpoint, &bounds).expect("declared counts are accepted");
        prompt.tools = alloc::vec![tool; 3].into();
        assert_eq!(
            client::measure(&prompt, Some(&credential), &endpoint, &bounds),
            Err(crate::Error::Limit { which: crate::Cap::Tools, bound: 2 })
        );
        prompt.tools = Box::new([]);
        prompt.messages[0].content = alloc::vec![block; 4].into();
        assert_eq!(
            client::measure(&prompt, Some(&credential), &endpoint, &bounds),
            Err(crate::Error::Limit { which: crate::Cap::HistoryItems, bound: 3 })
        );
        prompt.messages = alloc::vec![crate::Message { role: crate::Role::User, content: Box::new([]) }; 4].into();
        assert_eq!(
            client::measure(&prompt, Some(&credential), &endpoint, &bounds),
            Err(crate::Error::Limit { which: crate::Cap::HistoryItems, bound: 3 })
        );
    }
}

#[test]
fn the_public_route_takes_the_head_floor() {
    // llm.md 7: public response-head and error-body floors, without a capture.
    const HEAD_BYTES: u32 = 16 * 1024;
    const HEAD_FIELDS: u32 = 128;
    let bounds = Limits::derive(&DECLARED).unwrap().dialect(Provider::OpenAi);
    assert_eq!((bounds.http.head, bounds.http.headers, bounds.error_bytes), (HEAD_BYTES, HEAD_FIELDS, HEAD_BYTES));
}
