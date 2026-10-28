//! UI storage and full pages measured on the counting allocator.
use skein_heap::{Counting, Meter};
use skein_lib::Queue;
use skein_ui::scene::{Anchor, Point, Scene};
use skein_ui::{Builder, Limits, Node, Owner, Patch, Role, SlotRef, Style, View, markdown, worst_case};
use skein_ui_tests::full_page;

#[global_allocator]
static HEAP: Counting = Counting;
const LIMITS: Limits = Limits {
    nodes: 32,
    depth: 8,
    text_per_node: 8,
    text_bytes: 256,
    drawing_bytes: 0,
    styles_per_node: 2,
    patches: 64,
    bindings: 0,
    slots: 0,
    markdown_depth: 2,
    scene_marks: 32,
    scene_bytes: 256,
};
fn full_scene(limits: &Limits) -> Scene {
    let mut scene = Scene::new(limits);
    let tokens = [Style { owner: Owner { number: 1 }, token: 1 }, Style { owner: Owner { number: 1 }, token: 2 }];
    for _ in 0..limits.scene_marks {
        assert!(scene.text(Point { x: 0, y: 0 }, Anchor::Start, b"abcdefgh", &tokens));
    }
    scene
}
fn peak() -> (u64, u64) {
    let bound = worst_case(&LIMITS).expect("fixture capacities fit");
    let meter = Meter::new();
    meter.start();
    let mut builder = Builder::new(&LIMITS);
    full_page(&LIMITS, &mut builder);
    let mut view = View::new(&LIMITS);
    let mut patches = Queue::with_capacity(LIMITS.patches);
    let scene = full_scene(&LIMITS);
    let mut first = view.builder();
    full_page(&LIMITS, &mut first);
    view.finish(first, 1, &mut patches);
    while patches.pop().is_some() {}
    let mut second = view.builder();
    full_page(&LIMITS, &mut second);
    view.finish(second, 2, &mut patches);
    let mut words = Builder::new(&LIMITS);
    words.open(Role::Page);
    markdown::read(b"- x\n  - y\n", 0, &mut words);
    words.close();
    let words = words.finish();
    let measured = meter.end();
    assert_eq!(view.tree().len(), LIMITS.nodes);
    let mut text = 0;
    for node in view.tree().nodes() {
        let bytes = node.name.as_ref().map_or(0, |name| name.len())
            + node.text.as_slice().iter().map(|run| run.text.len()).sum::<usize>();
        assert_eq!(bytes, usize::try_from(LIMITS.text_per_node).expect("text limit"));
        text += bytes;
    }
    assert_eq!(text, usize::try_from(LIMITS.text_bytes).expect("text budget"));
    assert_eq!(scene.marks().len(), usize::try_from(LIMITS.scene_marks).expect("mark limit"));
    let scene_bytes: usize = (0..LIMITS.scene_marks).map(|index| scene.data(index).len()).sum();
    assert_eq!(scene_bytes, usize::try_from(LIMITS.scene_bytes).expect("scene budget"));
    assert!(!scene.refused());
    drop((words, scene, patches, view, builder));
    (measured.peak(), bound)
}
#[test]
fn full_pages_at_their_node_and_text_limits_stay_within_the_worst_case() {
    let (peak, bound) = peak();
    assert!(peak <= bound, "full pages, diff, Markdown and scene: {peak} <= {bound}");
}
#[test]
fn a_full_page_fills_its_node_text_and_style_limits() {
    let mut builder = Builder::new(&LIMITS);
    full_page(&LIMITS, &mut builder);
    let tree = builder.finish();
    assert_eq!(tree.nodes().len(), usize::try_from(LIMITS.nodes).expect("node count fits"));
    let mut text_bytes = 0;
    for node in tree.nodes() {
        let text = node.name.as_ref().map_or(0, |name| name.len())
            + node.text.as_slice().iter().map(|run| run.text.len()).sum::<usize>();
        assert_eq!(text, usize::try_from(LIMITS.text_per_node).expect("text count fits"));
        text_bytes += text;
        if node.role == Role::Text {
            assert_eq!(node.styles.len(), LIMITS.styles_per_node);
        }
    }
    assert_eq!(text_bytes, usize::try_from(LIMITS.text_bytes).expect("text budget fits"));
}

#[test]
fn a_full_patch_queue_holds_at_most_four_words_a_patch() {
    // Four words cover every patch variant (ui.md, 7).
    const WORDS: u64 = 4;
    let bound = u64::from(LIMITS.patches) * WORDS * u64::try_from(size_of::<usize>()).expect("word size fits");
    let meter = Meter::new();
    meter.start();
    let mut patches = Queue::<Patch>::with_capacity(LIMITS.patches);
    for node in 0..LIMITS.patches {
        patches.push(Patch::Remove { node: skein_ui::NodeId(node) });
    }
    let measured = meter.end();
    assert_eq!(patches.len(), LIMITS.patches);
    assert!(measured.held() <= bound, "patch queue storage: {} <= {bound}", measured.held());
    // A growing ring holds both its old and new buffers at a realloc peak.
    assert!(measured.peak() <= 2 * bound, "patch queue growth: {} <= 2 * {bound}", measured.peak());
}

#[test]
fn a_build_without_slots_reserves_no_splice_room() {
    let mut view = View::new(&LIMITS);
    let meter = Meter::new();
    meter.start();
    let mut builder = view.builder();
    let peak = meter.end().peak();
    let splice = u64::from(LIMITS.nodes) * u64::try_from(size_of::<Node>()).expect("node size fits");
    full_page(&LIMITS, &mut builder);
    let mut patches = Queue::with_capacity(LIMITS.patches);
    view.finish(builder, 1, &mut patches);
    assert!(peak < splice, "builder reservation without slots: {peak} < {splice}");
}

#[test]
fn a_view_reserves_its_splice_room_once() {
    let limits = Limits { slots: 2, ..LIMITS };
    let mut view = View::new(&limits);
    let mut patches = Queue::with_capacity(limits.patches);
    for shown in 1..=2 {
        // Keep the consumed fragment within the base; measure only the build interval.
        let meter = Meter::new();
        let mut fragment = Builder::new(&limits);
        fragment.open(Role::Region);
        fragment.close();
        let baseline = meter.held();
        meter.start();
        let mut builder = view.builder();
        builder.open(Role::Page);
        builder.slot(SlotRef { owner: Owner { number: 1 }, token: skein_lib::Token::new(1) }, b"Loading");
        builder.close();
        let slots = builder.slots(0);
        builder.splice(slots[0].id, fragment);
        view.finish(builder, shown, &mut patches);
        let peak = meter.end().peak().checked_sub(baseline).expect("peak includes the fragment baseline");
        while patches.pop().is_some() {}
        let splice = u64::from(limits.nodes) * u64::try_from(size_of::<Node>()).expect("node size fits");
        assert!(peak < splice, "view build {shown} reservation with slots: {peak} < {splice}");
    }
}
