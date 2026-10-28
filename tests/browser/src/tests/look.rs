#![expect(clippy::disallowed_types, reason = "the kit's own tests of the look's reads (browser.md, 5.1)")]
use crate::world::{Fault, World};
use skein_browser::boundary::{Below, Event, Look, Looked, Motion, Rect, Refusal, Request, Scheme};
use skein_browser::{Limits, up};
use skein_lib::{List, Time, Token, stream};
const PAGE: Token = Token::new(3);
const READ: Token = Token::new(4);
fn world(limits: Limits) -> World {
    let mut world = World::with_limits(limits);
    world.start();
    world.open();
    world.sent.clear();
    world
}
fn ask(world: &mut World, look: Look) -> Event {
    world.ask(Request::Look { page: PAGE, op: READ, look });
    world.above.pop().expect("look terminal")
}
fn looked(event: Event) -> Looked {
    match event {
        Event::Looked { op: READ, looked } => looked,
        other => panic!("expected Looked: {other:?}"),
    }
}
fn refused(event: Event, why: Refusal) {
    assert!(matches!(event, Event::Refused { op: READ, why: got } if got == why), "{event:?}");
}
fn commands(world: &World) -> Vec<(String, String)> {
    world
        .sent
        .iter()
        .map(|bytes| {
            let doc = skein_browser::wire::decode::Document::parse(
                bytes.strip_suffix(&[0]).unwrap(),
                world.env.limits.command,
            )
            .unwrap();
            (
                String::from_utf8(doc.root().get(b"method").unwrap().text().unwrap().into_vec()).unwrap(),
                String::from_utf8(doc.root().get(b"params").unwrap().raw().into()).unwrap(),
            )
        })
        .collect()
}
fn properties(names: &[&[u8]]) -> List<Box<[u8]>> {
    let mut out = List::with_capacity(u32::try_from(names.len()).unwrap());
    for name in names {
        out.push(Box::from(*name)).unwrap();
    }
    out
}
fn style(names: &[&[u8]]) -> Look {
    Look::Style { node: 7, properties: properties(names) }
}
fn viewport(width: u32, height: u32) -> Look {
    Look::Viewport { width, height, mobile: true, scheme: Scheme::Dark, motion: Motion::Reduce }
}
#[test]
fn viewport_sets_metrics_then_media_and_survives_a_navigation() {
    let mut world = world(Limits::default());
    assert_eq!(looked(ask(&mut world, viewport(390, 844))), Looked::Viewport);
    assert_eq!(commands(&world), [
        ("Emulation.setDeviceMetricsOverride".into(), r#"{"width":390,"height":844,"deviceScaleFactor":1,"mobile":true}"#.into()),
        ("Emulation.setEmulatedMedia".into(), r#"{"features":[{"name":"prefers-color-scheme","value":"dark"},{"name":"prefers-reduced-motion","value":"reduce"}]}"#.into()),
    ]);
    world.sent.clear();
    world.ask(Request::Go { page: PAGE, op: Token::new(5), to: skein_browser::boundary::Go::Reload });
    assert!(!commands(&world).iter().any(|(method, _)| method.starts_with("Emulation.")));
}
const TREE: &str = r#"{"nodes":[
{"nodeId":"root","backendDOMNodeId":1,"role":{"value":"RootWebArea"},"childIds":["ignored","second"]},
{"nodeId":"second","backendDOMNodeId":9,"role":{"value":"heading"},"name":{"value":"Second"},"properties":[{"name":"level","value":{"value":2}}]},
{"nodeId":"ignored","ignored":true,"childIds":["first"]},
{"nodeId":"first","role":{"value":"heading"},"name":{"value":"First heading"},"properties":[{"name":"level","value":{"value":1}}],"childIds":["third"]},
{"nodeId":"third","role":{"value":"heading"},"name":{"value":"Third"},"properties":[{"name":"level","value":{"value":3}}]}
]}"#;
#[test]
fn tree_follows_children_lifts_ignored_nodes_and_reads_levels() {
    let mut world = world(Limits { text: 8, entries: 4, ..Limits::default() });
    world.tree = TREE.into();
    let Looked::Tree(entries) = looked(ask(&mut world, Look::Tree)) else { panic!("tree") };
    let entries = entries.as_slice();
    assert_eq!(
        entries.iter().map(|e| (e.node, e.depth, e.level)).collect::<Vec<_>>(),
        [(Some(1), 0, 0), (None, 1, 1), (None, 2, 3), (Some(9), 1, 2)]
    );
    assert_eq!(entries[1].name.as_ref(), b"First he");
    assert_eq!(commands(&world)[0].0, "Accessibility.getFullAXTree");
}
#[test]
fn tree_past_entries_is_refused_whole() {
    let mut world = world(Limits { entries: 3, ..Limits::default() });
    world.tree = TREE.into();
    refused(ask(&mut world, Look::Tree), Refusal::Limit);
}
#[test]
fn tree_cycles_are_bounded() {
    let mut world = world(Limits::default());
    world.tree = r#"{"nodes":[{"nodeId":"1","ignored":true,"childIds":["1"]}]}"#.into();
    refused(ask(&mut world, Look::Tree), Refusal::Protocol);
}
#[test]
fn select_returns_backend_node_and_none() {
    let mut world = world(Limits::default());
    world.selected = 42;
    assert_eq!(
        looked(ask(&mut world, Look::Select { selector: b"#part".as_slice().into() })),
        Looked::Selected(Some(7))
    );
    assert_eq!(
        commands(&world),
        [
            ("DOM.getDocument".into(), r#"{"depth":0,"pierce":false}"#.into()),
            ("DOM.querySelector".into(), r##"{"nodeId":1,"selector":"#part"}"##.into()),
            ("DOM.describeNode".into(), r#"{"nodeId":42}"#.into()),
        ]
    );
    world.selected = 0;
    assert_eq!(
        looked(ask(&mut world, Look::Select { selector: b".absent".as_slice().into() })),
        Looked::Selected(None)
    );
}
#[test]
fn select_rejects_an_unparseable_selector() {
    let mut world = world(Limits::default());
    world.fault(b"DOM.querySelector", Fault::InvalidSelector);
    refused(ask(&mut world, Look::Select { selector: b"[".as_slice().into() }), Refusal::Protocol);
}
#[test]
fn style_keeps_request_order_missing_values_faces_and_document_border_box() {
    let mut world = world(Limits::default());
    world.frontend = 42;
    world.metrics = r#"{"cssVisualViewport":{"pageX":5.75,"pageY":30.25}}"#.into();
    let Looked::Style { values, faces, rect } = looked(ask(&mut world, style(&[b"color", b"absent", b"font-size"])))
    else {
        panic!("style")
    };
    assert_eq!(
        values.iter().map(|p| (p.name.as_ref(), p.value.as_ref())).collect::<Vec<_>>(),
        [(b"color".as_slice(), b"rgb(17, 17, 17)".as_slice()), (b"absent", b""), (b"font-size", b"16px")]
    );
    assert_eq!(
        faces.iter().map(|f| (f.family.as_ref(), f.custom, f.glyphs)).collect::<Vec<_>>(),
        [(b"Fixture".as_slice(), true, 5), (b"Fallback", false, 2)]
    );
    assert_eq!(rect, Some(Rect { x: 15, y: 50, width: 100, height: 40 }));
    assert_eq!(
        commands(&world).iter().map(|(m, _)| m.as_str()).collect::<Vec<_>>(),
        [
            "CSS.enable",
            "DOM.getDocument",
            "DOM.pushNodesByBackendIdsToFrontend",
            "CSS.getComputedStyleForNode",
            "DOM.requestChildNodes",
            "DOM.describeNode",
            "CSS.getPlatformFontsForNode",
            "DOM.getBoxModel",
            "Page.getLayoutMetrics",
            "DOM.describeNode"
        ]
    );
    assert_eq!(commands(&world)[2].1, r#"{"backendNodeIds":[7]}"#);
    assert_eq!(commands(&world)[3].1, r#"{"nodeId":42}"#);
    assert_eq!(commands(&world)[4].1, r#"{"nodeId":42,"depth":-1,"pierce":true}"#);
    assert_eq!(commands(&world)[5].1, r#"{"backendNodeId":7,"depth":-1,"pierce":true}"#);
    assert_eq!(commands(&world)[6].1, r#"{"nodeId":8}"#);
    world.sent.clear();
    looked(ask(&mut world, style(&[])));
    assert!(!commands(&world).iter().any(|(m, _)| m == "CSS.enable"));
}
#[test]
fn style_values_at_text_are_whole_and_past_text_are_refused() {
    let mut world = world(Limits { text: 6, ..Limits::default() });
    world.styles = r#"{"computedStyle":[{"name":"color","value":"123456"}]}"#.into();
    let Looked::Style { values, .. } = looked(ask(&mut world, style(&[b"color"]))) else { panic!("style") };
    assert_eq!(values.get(0).unwrap().value.as_ref(), b"123456");
    world.styles = r#"{"computedStyle":[{"name":"color","value":"1234567"}]}"#.into();
    refused(ask(&mut world, style(&[b"color"])), Refusal::Limit);
}
#[test]
fn faces_at_entries_are_kept_and_past_entries_refused() {
    let mut world = world(Limits { entries: 2, text: 6, ..Limits::default() });
    let Looked::Style { faces, .. } = looked(ask(&mut world, style(&[]))) else { panic!("style") };
    assert_eq!(faces.get(1).unwrap().family.as_ref(), b"Fallba");
    world.env.limits.entries = 1;
    refused(ask(&mut world, style(&[])), Refusal::Limit);
}
#[test]
fn style_a_gone_backend_or_a_node_removed_between_steps_is_gone() {
    let mut world = world(Limits::default());
    world.gone = true;
    refused(ask(&mut world, style(&[])), Refusal::Gone);
    world.gone = false;
    for method in [
        b"DOM.pushNodesByBackendIdsToFrontend".as_slice(),
        b"CSS.getComputedStyleForNode",
        b"DOM.requestChildNodes",
        b"CSS.getPlatformFontsForNode",
        b"DOM.getBoxModel",
        b"DOM.describeNode",
    ] {
        world.fault(method, Fault::Error);
        refused(ask(&mut world, style(&[])), Refusal::Gone);
    }
}
#[test]
fn style_a_node_without_a_box_has_no_rect() {
    let mut world = world(Limits::default());
    world.fault(b"DOM.getBoxModel", Fault::NoBox);
    let Looked::Style { rect, .. } = looked(ask(&mut world, style(&[]))) else { panic!("style") };
    assert_eq!(rect, None);
    assert_eq!(commands(&world).last().unwrap().0, "DOM.describeNode");
}
#[test]
fn shot_sends_the_document_clip_scale_and_capture_beyond_viewport() {
    let mut world = world(Limits::default());
    let clip = Rect { x: 10, y: 2000, width: 80, height: 40 };
    let Looked::Shot(png) = looked(ask(&mut world, Look::Shot { clip: Some(clip), scale: 2 })) else { panic!("shot") };
    assert_eq!(png.as_ref(), b"\x89PNG\r\n\x1a\n");
    assert_eq!(commands(&world), [
        ("CSS.enable".into(), "{}".into()),
        ("Page.getFrameTree".into(), "{}".into()),
        ("CSS.createStyleSheet".into(), r#"{"frameId":"f1"}"#.into()),
        ("CSS.setStyleSheetText".into(), r#"{"styleSheetId":"sh1","text":"* { caret-animation: manual !important }"}"#.into()),
        ("Page.captureScreenshot".into(), r#"{"format":"png","clip":{"x":10,"y":2000,"width":80,"height":40,"scale":2},"captureBeyondViewport":true}"#.into()),
    ]);
}
#[test]
fn shot_without_clip_reads_the_visual_viewport_first() {
    let mut world = world(Limits::default());
    world.metrics = r#"{"cssVisualViewport":{"pageX":5.5,"pageY":30.5,"clientWidth":390,"clientHeight":844}}"#.into();
    looked(ask(&mut world, Look::Shot { clip: None, scale: 1 }));
    assert_eq!(
        commands(&world).iter().map(|(m, _)| m.as_str()).collect::<Vec<_>>(),
        [
            "CSS.enable",
            "Page.getFrameTree",
            "CSS.createStyleSheet",
            "CSS.setStyleSheetText",
            "Page.getLayoutMetrics",
            "Page.captureScreenshot",
        ]
    );
    assert_eq!(
        commands(&world)[5].1,
        r#"{"format":"png","clip":{"x":5,"y":30,"width":390,"height":844,"scale":1},"captureBeyondViewport":true}"#
    );
}
#[test]
fn second_shot_in_the_same_document_reuses_the_caret_sheet() {
    let mut world = world(Limits::default());
    looked(ask(&mut world, Look::Shot { clip: None, scale: 1 }));
    notify(
        &mut world,
        b"{\"method\":\"Page.navigatedWithinDocument\",\"sessionId\":\"s1\",\"params\":{\"frameId\":\"f1\"}}\0",
    );
    world.sent.clear();
    looked(ask(&mut world, Look::Shot { clip: None, scale: 1 }));
    assert_eq!(
        commands(&world).iter().map(|(m, _)| m.as_str()).collect::<Vec<_>>(),
        ["Page.getFrameTree", "Page.getLayoutMetrics", "Page.captureScreenshot"]
    );
    world.assert_commands_settled();
}
#[test]
fn shot_in_a_new_document_installs_a_new_caret_sheet() {
    let mut world = world(Limits::default());
    looked(ask(&mut world, Look::Shot { clip: None, scale: 1 }));
    let previous = world.loader.clone();
    world.ask(Request::Go { page: PAGE, op: Token::new(5), to: skein_browser::boundary::Go::Reload });
    assert!(matches!(world.above.pop(), Some(Event::Done { op }) if op == Token::new(5)));
    assert_ne!(world.loader, previous);
    world.sent.clear();
    looked(ask(&mut world, Look::Shot { clip: None, scale: 1 }));
    assert_eq!(
        commands(&world).iter().map(|(m, _)| m.as_str()).collect::<Vec<_>>(),
        [
            "Page.getFrameTree",
            "CSS.createStyleSheet",
            "CSS.setStyleSheetText",
            "Page.getLayoutMetrics",
            "Page.captureScreenshot",
        ]
    );
    world.assert_commands_settled();
}
#[test]
fn shot_setup_faults_settle_once_and_allow_a_retry() {
    for method in [b"CSS.enable".as_slice(), b"Page.getFrameTree", b"CSS.createStyleSheet", b"CSS.setStyleSheetText"] {
        for (fault, late) in [(Fault::Error, false), (Fault::Drop, false), (Fault::Delay, false), (Fault::Delay, true)]
        {
            let mut world = world(Limits::default());
            world.fault(method, fault);
            world.ask(Request::Look { page: PAGE, op: READ, look: Look::Shot { clip: None, scale: 1 } });
            assert_eq!(commands(&world).last().unwrap().0.as_bytes(), method);
            if fault != Fault::Error {
                assert!(world.above.is_empty());
                if fault == Fault::Drop || late {
                    world.tick(Time::from_nanos(world.env.limits.answer.as_nanos()));
                }
                world.release_delayed();
            }
            let event = world.above.pop().expect("shot terminal after fault");
            if fault == Fault::Delay && !late {
                looked(event);
            } else {
                refused(event, if fault == Fault::Error { Refusal::Protocol } else { Refusal::Timeout });
                assert!(!commands(&world).iter().any(|(m, _)| m == "Page.captureScreenshot"));
            }
            assert!(world.above.is_empty());
            world.assert_commands_settled();
            looked(ask(&mut world, Look::Shot { clip: None, scale: 1 }));
            assert!(world.above.is_empty());
            world.assert_commands_settled();
        }
    }
}
#[test]
fn shot_at_screenshot_is_kept_and_past_screenshot_refused() {
    let mut world = world(Limits { screenshot: 8, ..Limits::default() });
    looked(ask(&mut world, Look::Shot { clip: None, scale: 1 }));
    world.env.limits.screenshot = 7;
    refused(ask(&mut world, Look::Shot { clip: None, scale: 1 }), Refusal::Limit);
}
#[test]
fn look_admission_checks_both_sides_of_each_bound_without_commands() {
    let mut world = world(Limits { text: 6, properties: 2, ..Limits::default() });
    world.styles = r#"{"computedStyle":[]}"#.into();
    for admitted in [
        viewport(1, 1),
        Look::Shot { clip: Some(Rect { x: 0, y: 0, width: 1, height: 1 }), scale: 1 },
        Look::Shot { clip: None, scale: 2 },
        Look::Select { selector: b"123456".as_slice().into() },
        style(&[b"123456", b"color"]),
    ] {
        looked(ask(&mut world, admitted));
    }
    let cases = [
        (viewport(0, 1), Refusal::Protocol),
        (viewport(1, 0), Refusal::Protocol),
        (Look::Shot { clip: None, scale: 0 }, Refusal::Protocol),
        (Look::Shot { clip: None, scale: 3 }, Refusal::Protocol),
        (Look::Shot { clip: Some(Rect { x: 0, y: 0, width: 0, height: 1 }), scale: 1 }, Refusal::Protocol),
        (Look::Shot { clip: Some(Rect { x: 0, y: 0, width: 1, height: 0 }), scale: 1 }, Refusal::Protocol),
        (Look::Select { selector: b"1234567".as_slice().into() }, Refusal::Limit),
        (style(&[b"1234567"]), Refusal::Limit),
        (style(&[b"a", b"b", b"c"]), Refusal::Limit),
    ];
    for (look, why) in cases {
        world.sent.clear();
        refused(ask(&mut world, look), why);
        assert!(world.sent.is_empty());
    }
}
fn notify(world: &mut World, message: &[u8]) {
    up(
        &mut world.browser,
        &world.env,
        Below::Replies(stream::Up::Bytes(message.into())),
        &mut world.above,
        &mut world.below,
    );
    world.tick(world.env.now);
}
const NAVIGATING: &[u8] =
    b"{\"method\":\"Page.frameStartedLoading\",\"sessionId\":\"s1\",\"params\":{\"frameId\":\"f1\"}}\0";
const LOADED: &[u8] = b"{\"method\":\"Page.loadEventFired\",\"sessionId\":\"s1\",\"params\":{}}\0";
#[test]
fn tree_select_and_style_wait_and_restart_across_navigation() {
    for look in [Look::Tree, Look::Select { selector: b"#part".as_slice().into() }, style(&[])] {
        let mut world = world(Limits::default());
        let method = match look {
            Look::Tree => b"Accessibility.getFullAXTree".as_slice(),
            Look::Select { .. } => b"DOM.querySelector",
            _ => b"CSS.getComputedStyleForNode",
        };
        world.fault(method, Fault::Delay);
        world.ask(Request::Look { page: PAGE, op: READ, look });
        notify(&mut world, NAVIGATING);
        world.release_delayed();
        assert!(world.above.is_empty());
        world.sent.clear();
        notify(&mut world, LOADED);
        world.tick(Time::from_nanos(world.env.limits.poll.as_nanos()));
        looked(world.above.pop().expect("restarted terminal"));
        assert_eq!(
            commands(&world)[0].0,
            if method == b"Accessibility.getFullAXTree" { "Accessibility.getFullAXTree" } else { "DOM.getDocument" }
        );
    }
}
#[test]
fn tree_started_during_navigation_waits_for_commit() {
    let mut world = world(Limits::default());
    notify(&mut world, NAVIGATING);
    world.ask(Request::Look { page: PAGE, op: READ, look: Look::Tree });
    assert!(world.sent.is_empty());
    notify(&mut world, LOADED);
    world.tick(Time::from_nanos(world.env.limits.poll.as_nanos()));
    looked(world.above.pop().expect("tree after load"));
}
#[test]
fn looks_have_command_deadlines_and_crash_terminals() {
    for fault in [Fault::Drop, Fault::Crash, Fault::PageCrash, Fault::InvalidSelector] {
        let mut world = world(Limits::default());
        world.fault(b"Accessibility.getFullAXTree", fault);
        world.ask(Request::Look { page: PAGE, op: READ, look: Look::Tree });
        if fault == Fault::Drop {
            world.tick(Time::from_nanos(world.env.limits.answer.as_nanos()));
        }
        let events: Vec<_> = std::iter::from_fn(|| world.above.pop()).collect();
        let why = match fault {
            Fault::Drop => Refusal::Timeout,
            Fault::InvalidSelector => Refusal::Protocol,
            _ => Refusal::Crashed,
        };
        assert!(
            events.iter().any(|event| matches!(event, Event::Refused {op:READ,why: got} if *got == why)),
            "{events:?}"
        );
    }
}

#[test]
fn concurrent_styles_share_one_css_enable() {
    let mut world = world(Limits::default());
    world.fault(b"CSS.enable", Fault::Delay);
    world.ask(Request::Look { page: PAGE, op: READ, look: style(&[]) });
    world.ask(Request::Look { page: PAGE, op: Token::new(5), look: style(&[]) });
    assert_eq!(commands(&world).iter().filter(|(m, _)| m == "CSS.enable").count(), 1);
    world.release_delayed();
    world.tick(Time::from_nanos(world.env.limits.poll.as_nanos()));
    assert!(matches!(world.above.pop(), Some(Event::Looked { .. })));
    assert!(matches!(world.above.pop(), Some(Event::Looked { .. })));
    assert_eq!(commands(&world).iter().filter(|(m, _)| m == "CSS.enable").count(), 1);
}
#[test]
fn viewport_and_shot_proceed_during_navigation() {
    let mut world = world(Limits::default());
    notify(&mut world, NAVIGATING);
    looked(ask(&mut world, viewport(390, 844)));
    looked(ask(&mut world, Look::Shot { clip: None, scale: 1 }));
}
#[test]
fn unrelated_style_command_errors_are_protocol_refusals() {
    for method in [
        b"CSS.enable".as_slice(),
        b"DOM.getDocument",
        b"CSS.getComputedStyleForNode",
        b"DOM.requestChildNodes",
        b"CSS.getPlatformFontsForNode",
        b"DOM.getBoxModel",
        b"Page.getLayoutMetrics",
        b"DOM.describeNode",
    ] {
        let mut world = world(Limits::default());
        world.fault(method, Fault::InvalidSelector);
        refused(ask(&mut world, style(&[])), Refusal::Protocol);
    }
}
#[test]
fn browser_limits_require_finite_framing_and_advancing_deadlines() {
    let defaults = Limits::default();
    assert!(skein_browser::valid(&defaults));
    for limits in [
        Limits { commands: 0, ..defaults },
        Limits { command: 0, ..defaults },
        Limits { message: 0, ..defaults },
        Limits { message: u32::MAX, ..defaults },
        Limits { command: u32::MAX, ..defaults },
        Limits { answer: skein_lib::Duration::from_nanos(0), ..defaults },
        Limits { poll: skein_lib::Duration::from_nanos(0), ..defaults },
    ] {
        assert!(!skein_browser::valid(&limits), "unusable limits: {limits:?}");
    }
}

#[test]
#[should_panic(expected = "browser limits must be representable")]
fn browser_refuses_a_read_without_room_for_its_nul() {
    let limits = Limits { message: u32::MAX, ..Limits::default() };
    assert_eq!(skein_browser::largest_read(&limits), None);
    assert!(!skein_browser::valid(&limits));
    let _browser = skein_browser::Browser::new(Token::new(1), &limits);
}

#[test]
#[should_panic(expected = "browser limits must be representable")]
fn browser_refuses_a_command_without_room_for_its_nul() {
    let limits = Limits { command: u32::MAX, ..Limits::default() };
    assert_eq!(skein_browser::largest_room(&limits), None);
    assert!(!skein_browser::valid(&limits));
    let _browser = skein_browser::Browser::new(Token::new(1), &limits);
}

#[test]
fn style_merges_nested_text_once_by_family_and_custom() {
    let mut world = world(Limits { entries: 3, ..Limits::default() });
    world.children = r#"[
      {"nodeId":8,"nodeType":3,"nodeValue":"Own"},
      {"nodeId":9,"nodeType":1,"nodeName":"DIV","children":[
        {"nodeId":10,"nodeType":1,"nodeName":"P","children":[
          {"nodeId":11,"nodeType":3,"nodeValue":"Nested"},
          {"nodeId":12,"nodeType":3,"nodeValue":""},
          {"nodeId":13,"nodeType":3,"nodeValue":"Again"}]}]}]"#
        .into();
    world.fonts = [
        (8, r#"{"fonts":[{"familyName":"Family one","isCustomFont":false,"glyphCount":3}]}"#.into()),
        (11, r#"{"fonts":[{"familyName":"Family two","isCustomFont":true,"glyphCount":6},{"familyName":"Family one","isCustomFont":true,"glyphCount":1}]}"#.into()),
        (13, r#"{"fonts":[{"familyName":"Family one","isCustomFont":false,"glyphCount":5}]}"#.into()),
    ].into();
    let Looked::Style { faces, .. } = looked(ask(&mut world, style(&[]))) else { panic!("style") };
    assert_eq!(faces.len(), 3);
    for (family, custom, glyphs) in [("Family one", false, 8), ("Family two", true, 6), ("Family one", true, 1)] {
        assert!(
            faces.iter().any(|f| f.family.as_ref() == family.as_bytes() && f.custom == custom && f.glyphs == glyphs),
            "{faces:?}"
        );
    }
    let calls: Vec<_> =
        commands(&world).into_iter().filter(|(m, _)| m == "CSS.getPlatformFontsForNode").map(|(_, p)| p).collect();
    assert_eq!(calls, [r#"{"nodeId":8}"#, r#"{"nodeId":11}"#, r#"{"nodeId":13}"#]);
    world.env.limits.entries = 2;
    refused(ask(&mut world, style(&[])), Refusal::Limit);
}

#[test]
fn style_walk_reaches_deep_text_without_an_entries_walk_cap() {
    let mut world = world(Limits { entries: 2, ..Limits::default() });
    let mut node = r#"{"nodeId":8,"nodeType":3,"nodeValue":"Deep"}"#.to_owned();
    for id in 20..40 {
        node = format!(r#"{{"nodeId":{id},"nodeType":1,"children":[{node}]}}"#);
    }
    world.children = format!("[{node}]");
    let Looked::Style { faces, .. } = looked(ask(&mut world, style(&[]))) else { panic!("style") };
    assert_eq!(faces.len(), 2);
    assert_eq!(commands(&world).iter().filter(|(m, _)| m == "CSS.getPlatformFontsForNode").count(), 1);
}

#[test]
fn style_without_text_has_no_faces_or_font_calls() {
    let mut world = world(Limits { entries: 0, ..Limits::default() });
    world.children = r#"[{"nodeId":8,"nodeType":3,"nodeValue":""}]"#.into();
    let Looked::Style { faces, .. } = looked(ask(&mut world, style(&[]))) else { panic!("style") };
    assert!(faces.is_empty());
    assert!(!commands(&world).iter().any(|(m, _)| m == "CSS.getPlatformFontsForNode"));
}

#[test]
fn style_glyph_sum_overflow_is_refused() {
    let mut world = world(Limits::default());
    world.children = r#"[{"nodeId":8,"nodeType":3,"nodeValue":"A"},{"nodeId":9,"nodeType":3,"nodeValue":"B"}]"#.into();
    world.fonts.insert(8, r#"{"fonts":[{"familyName":"Fixture","isCustomFont":true,"glyphCount":4294967295}]}"#.into());
    world.fonts.insert(9, r#"{"fonts":[{"familyName":"Fixture","isCustomFont":true,"glyphCount":1}]}"#.into());
    refused(ask(&mut world, style(&[])), Refusal::Protocol);
}

#[test]
fn style_walk_reply_keeps_the_existing_message_bound() {
    let mut world = world(Limits::default());
    world.fault(b"DOM.requestChildNodes", Fault::Oversize);
    world.ask(Request::Look { page: PAGE, op: READ, look: style(&[]) });
    assert!(
        std::iter::from_fn(|| world.above.pop())
            .any(|e| matches!(e, Event::Refused { op: READ, why: Refusal::Crashed })),
        "oversized CDP replies close the browser"
    );
}

#[test]
fn style_family_names_merge_before_boundary_text_is_cut() {
    let mut world = world(Limits { entries: 2, text: 6, ..Limits::default() });
    world.fonts.insert(8, r#"{"fonts":[{"familyName":"Family one","isCustomFont":true,"glyphCount":3},{"familyName":"Family two","isCustomFont":true,"glyphCount":5}]}"#.into());
    let Looked::Style { faces, .. } = looked(ask(&mut world, style(&[]))) else { panic!("style") };
    assert_eq!(
        faces.iter().map(|f| (f.family.as_ref(), f.glyphs)).collect::<Vec<_>>(),
        [(b"Family".as_slice(), 3), (b"Family".as_slice(), 5)]
    );
}

#[test]
fn style_pushes_text_that_has_only_a_backend_id() {
    let mut world = world(Limits::default());
    world.children = r#"[{"nodeId":0,"backendNodeId":80,"nodeType":3,"nodeValue":"Control"}]"#.into();
    world.fonts.insert(80, r#"{"fonts":[{"familyName":"Control","isCustomFont":false,"glyphCount":7}]}"#.into());
    let Looked::Style { faces, .. } = looked(ask(&mut world, style(&[]))) else { panic!("style") };
    assert_eq!((faces.len(), faces.get(0).unwrap().glyphs), (1, 7));
    let calls = commands(&world);
    let push = calls
        .iter()
        .position(|(m, p)| m == "DOM.pushNodesByBackendIdsToFrontend" && p == r#"{"backendNodeIds":[80]}"#)
        .expect("text pushed");
    assert_eq!(calls[push + 1], ("CSS.getPlatformFontsForNode".into(), r#"{"nodeId":80}"#.into()));
}

#[test]
fn tree_reads_cut_values_all_states_and_multiline() {
    let mut world = world(Limits { text: 4, ..Limits::default() });
    world.tree = r#"{"nodes":[{"nodeId":"field","backendDOMNodeId":7,"role":{"value":"textbox"},"name":{"value":"Title"},"value":{"value":"Words to cut"},"properties":[{"name":"focused","value":{"value":true}},{"name":"disabled","value":{"value":true}},{"name":"checked","value":{"value":"true"}},{"name":"expanded","value":{"value":true}},{"name":"selected","value":{"value":true}},{"name":"multiline","value":{"value":true}}]}]}"#.into();
    let Looked::Tree(entries) = looked(ask(&mut world, Look::Tree)) else { panic!("tree") };
    let entry = entries.get(0).unwrap();
    assert_eq!(entry.value.as_ref(), b"Word");
    assert_eq!(entry.name.as_ref(), b"Titl");
    assert_eq!(
        entry.states,
        skein_browser::boundary::States {
            focused: true,
            disabled: true,
            checked: true,
            expanded: true,
            selected: true
        }
    );
    assert!(entry.multiline);
}
