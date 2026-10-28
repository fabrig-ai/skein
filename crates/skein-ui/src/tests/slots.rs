use crate::Limits;
use crate::{Builder, Owner, Role, Run, SlotRef, State};
use skein_lib::Token;

const LIMITS: Limits = Limits {
    nodes: 24,
    depth: 10,
    text_per_node: 32,
    text_bytes: 256,
    drawing_bytes: 8,
    styles_per_node: 2,
    patches: 144,
    bindings: 8,
    slots: 8,
    markdown_depth: 2,
    scene_marks: 16,
    scene_bytes: 256,
};

fn reference(number: u64) -> SlotRef {
    SlotRef { owner: Owner { number: 1 }, token: Token::new(number) }
}

fn page() -> Builder {
    let mut builder = Builder::new(&LIMITS);
    builder.open(Role::Page);
    builder.slot(reference(1), b"Loading");
    builder.slot(reference(2), b"Loading");
    builder.close();
    builder
}

fn fragment(number: u64) -> Builder {
    fragment_with_limits(number, &LIMITS)
}

fn fragment_with_limits(number: u64, limits: &Limits) -> Builder {
    let mut builder = Builder::new(limits);
    builder.open(Role::Region);
    builder.text(Run::Plain, b"words");
    builder.slot(reference(number), b"Loading");
    builder.close();
    builder
}

#[test]
fn slots_fill_two_deep_in_preorder_and_later_ids_survive_insertions() {
    let mut builder = page();
    let first = builder.slots(0);
    builder.splice(first[0].id, fragment(3));
    builder.splice(first[1].id, fragment(4));
    let second = builder.slots(1);
    assert_eq!(second.len(), 2);
    assert_eq!(second[0].reference, reference(3));
    assert_eq!(second[1].reference, reference(4));
    builder.splice(second[0].id, fragment(5));
    builder.unshowable(second[1].id, b"Unavailable");
    assert_eq!(builder.slots(2).len(), 1);
    assert!(builder.slots(0).is_empty());
    assert!(builder.slots(1).is_empty());
    let tree = builder.finish();
    assert_eq!(tree.get(0).expect("page").size, tree.len());
    for index in 0..tree.len() {
        let node = tree.get(index).expect("preorder node");
        if let Some(slot) = node.slot {
            assert!(node.name.is_none() && node.states.is_empty() && node.binding.is_none());
            if slot == reference(5) {
                assert_eq!(node.size, 2);
                let fallback = tree.get(index.checked_add(1).expect("child fits")).expect("loading child");
                assert_eq!(fallback.name.as_deref(), Some(b"Loading".as_slice()));
                assert_eq!(fallback.states.as_slice(), [State::Busy]);
            }
        }
    }
}

#[test]
fn loading_and_unshowable_are_named_statuses_without_breaking_the_page() {
    let mut builder = page();
    let slots = builder.slots(0);
    builder.unshowable(slots[1].id, b"Cannot show this part");
    let tree = builder.finish();
    let loading = tree.get(2).expect("first fallback");
    assert_eq!(loading.role, Role::Status);
    assert_eq!(loading.name.as_deref(), Some(b"Loading".as_slice()));
    assert_eq!(loading.states.as_slice(), [State::Busy]);
    let unavailable = tree.get(4).expect("second fallback");
    assert_eq!(unavailable.name.as_deref(), Some(b"Cannot show this part".as_slice()));
    assert!(unavailable.states.is_empty());
}

#[test]
#[should_panic(expected = "slots nest at most two deep")]
fn a_third_fill_is_refused_and_its_slot_stays_loading_until_then() {
    let mut builder = page();
    let first = builder.slots(0);
    builder.splice(first[0].id, fragment(3));
    let second = builder.slots(1);
    builder.splice(second[0].id, fragment(4));
    let third = builder.slots(2);
    builder.splice(third[0].id, fragment(5));
}

#[test]
#[should_panic(expected = "spliced tree fits its node capacity")]
fn all_fragments_count_against_the_same_node_capacity() {
    let limits = Limits { nodes: 8, ..LIMITS };
    let mut builder = Builder::new(&limits);
    builder.open(Role::Page);
    builder.slot(reference(1), b"Loading");
    builder.close();
    let first = builder.slots(0);
    builder.splice(first[0].id, fragment_with_limits(2, &limits));
    let second = builder.slots(1);
    builder.splice(second[0].id, fragment_with_limits(3, &limits));
}

#[test]
#[should_panic(expected = "spliced slots fit their capacity")]
fn filled_slots_still_count_against_the_slots_limit() {
    let limits = Limits { slots: 1, ..LIMITS };
    let mut builder = Builder::new(&limits);
    builder.slot(reference(1), b"Loading");
    let slots = builder.slots(0);
    let mut fragment = Builder::new(&limits);
    fragment.slot(reference(2), b"Loading");
    builder.splice(slots[0].id, fragment);
}
