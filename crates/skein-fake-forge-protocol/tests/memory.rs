//! The hosted git codec and two-parent walk under the counting allocator.
extern crate alloc;
use skein_fake_forge_domain as domain;
use skein_fake_forge_protocol::{
    Config, Event, Faults, Hosted, Limits, Repository, Server, Service, closed, delivery, down, resume,
    resume_with_domain, start, up, up_with_domain,
};
#[cfg(test)]
#[path = "../src/tests/fixtures.rs"]
#[expect(dead_code, reason = "the protocol's shared fixture also serves route and document tests")]
mod fixtures;
#[cfg(test)]
mod tests {
    use super::*;
    use skein_fake_forge_protocol::git;
    use skein_heap::{Counting, Meter};
    use skein_lib::{List, Writer};
    #[global_allocator]
    static HEAP: Counting = Counting;

    fn bytes(count: u32, byte: u8) -> Box<[u8]> {
        let mut writer = Writer::new(count.try_into().expect("test bytes"));
        for _ in 0..count {
            writer.put(&[byte]).expect("sized bytes");
        }
        writer.finish()
    }
    fn tree(limits: &domain::Limits) -> Box<[domain::api::File]> {
        let mut files = List::with_capacity(limits.files);
        for index in 0..limits.files {
            files
                .push(domain::api::File {
                    path: bytes(limits.name_bytes, u8::try_from(index).expect("tiny files")),
                    content: bytes(limits.content_bytes, 0xff),
                })
                .expect("bounded tree");
        }
        files.into_boxed()
    }
    // Bounds the hosted git scenario, including its driver and shared domain.
    const FAKE_FORGEJO_GIT_SCENARIO_PEAK_CEILING: u64 = 560_000;

    #[test]
    fn maximal_trees_and_merge_history_stay_under_the_scenario_peak_ceiling() {
        let meter = Meter::new();
        meter.start();
        let mut config = fixtures::config();
        let domain_limits = fixtures::fake_config().limits;
        config.max_response_items = domain_limits.commits;
        let bound = FAKE_FORGEJO_GIT_SCENARIO_PEAK_CEILING
            .checked_add(domain::worst_case(&domain_limits).expect("domain bound"))
            .expect("combined bound");
        // Driver buffers and the shared domain are included in the scenario peak.
        let mut h = fixtures::Harness::new(config);
        let mut tip = 1_u64;
        for _ in 1..domain_limits.commits {
            let call = git::Call::CommitTree {
                parent: tip,
                merging: Some(1),
                tree: tree(&domain_limits),
                message: bytes(
                    domain_limits.title_bytes.saturating_add(domain_limits.body_bytes).saturating_add(2),
                    0xfe,
                ),
            };
            let encoded = git::client::encode(&call, &h.env.limits).expect("largest request fits");
            h.ask(b"POST", &encoded.target, &encoded.body);
            assert_eq!(h.status(), 200);
            tip = tip.checked_add(1).expect("tiny history");
        }
        let call = git::Call::Objects { commits: Box::new([tip]) };
        let encoded = git::client::encode(&call, &h.env.limits).expect("objects request");
        h.ask(b"POST", &encoded.target, &encoded.body);
        assert_eq!(h.status(), 200);
        assert!(skein_lib::bytes::find(&h.body(), b"\"graph\":[{\"commit\":15,").is_some());
        let measured = meter.end();
        let peak = meter.check(measured, bound, &"maximal git trees and both-parent frontier");
        assert!(peak <= FAKE_FORGEJO_GIT_SCENARIO_PEAK_CEILING);
    }
    #[test]
    fn client_objects_decoder_fits_its_priced_scratch() {
        use skein_json::tokenizer::{self as json, Tokenizer};
        use skein_lib::{
            Env, Queue, Time, Wall,
            stream::{Down, Held},
        };
        let limits = fixtures::limits();
        let domain = fixtures::fake_config().limits;
        let mut objects = List::with_capacity(5);
        for commit in 1..=5 {
            objects
                .push(git::Object {
                    commit,
                    parent: Some(1),
                    merge_parent: Some(1),
                    tree: tree(&domain),
                    message: bytes(domain.title_bytes.saturating_add(domain.body_bytes).saturating_add(2), 0xff),
                })
                .expect("whole objects");
        }
        let body =
            git::server::encode(&git::Answer::Objects { objects: objects.into_boxed(), graph: Box::new([]) }, &limits)
                .expect("five largest trees fit the wire");
        let bound = git::decoder_worst_case(&limits)
            .expect("decoder bound")
            .checked_add(json::worst_case(&limits.documents.json).expect("tokenizer bound"))
            .expect("combined bound")
            .checked_add(Queue::<json::Event>::worst_case(1).expect("event slot"))
            .expect("combined bound")
            .checked_add(Queue::<Down>::worst_case(1).expect("demand slot"))
            .expect("combined bound");
        let meter = Meter::new();
        meter.start();
        let mut decoder = git::client::decoder(git::Name::Objects, 200, Time::ZERO, &limits);
        let mut tokenizer = Tokenizer::new(&limits.documents.json);
        let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: limits.documents.json };
        let mut above = Queue::with_capacity(1);
        let mut below = Queue::with_capacity(1);
        let mut input = Held::new(body);
        json::down(&mut tokenizer, &env, git::client::first(&decoder), &mut above, &mut below);
        let answer = loop {
            if let Some(event) = above.pop() {
                match git::client::step(&mut decoder, &limits, &event) {
                    git::client::Step::Demand(request) => {
                        json::down(&mut tokenizer, &env, request, &mut above, &mut below);
                    }
                    git::client::Step::Decoded(answer) => break answer.expect("objects answer"),
                    git::client::Step::Malformed => panic!("objects grammar"),
                }
            } else {
                let Some(Down::Demand { read, .. }) = below.pop() else { panic!("tokenizer demand") };
                json::up(&mut tokenizer, &env, input.answer(read).expect("whole body"), &mut above, &mut below);
            }
        };
        let git::Answer::Objects { objects, graph } = answer else { panic!("objects") };
        assert_eq!(objects.len(), 5);
        assert!(graph.is_empty());
        let measured = meter.end();
        meter.check(measured, bound, &"client's compact document, decoded trees and tokenizer");
    }
}
