//! The list, addresses and capacities (examples.md, 9.5).
mod adds;
mod chart;
mod drift;
mod fixtures;
mod form;
mod person;
mod stream;
mod text;
use crate::{Place, State, limits, places};
use skein_lib::{Queue, Writer};
use skein_ui::{Role, View};

const PATHS: &[(&[u8], Place, &[u8])] = &[
    (b"/", Place::List, b"/"),
    #[cfg(debug_assertions)]
    (b"/test/panic", Place::Panic, b"/test/panic"),
    #[cfg(not(debug_assertions))]
    (b"/test/panic", Place::Missing, b"/missing"),
    #[cfg(debug_assertions)]
    (b"/test/small-store", Place::List, b"/"),
    #[cfg(not(debug_assertions))]
    (b"/test/small-store", Place::Missing, b"/missing"),
    (b"/notes/7", Place::Note { number: 7 }, b"/notes/7"),
    (b"/notes/07", Place::Missing, b"/missing"),
    (b"/x", Place::Missing, b"/missing"),
    (b"/?x=1", Place::List, b"/"),
    (b"/notes/0", Place::Note { number: 0 }, b"/notes/0"),
    (b"/notes/18446744073709551615", Place::Note { number: u64::MAX }, b"/notes/18446744073709551615"),
    (b"/notes/18446744073709551616", Place::Missing, b"/missing"),
    (b"/notes/7/", Place::Missing, b"/missing"),
    (b"/notes/", Place::Missing, b"/missing"),
];
#[test]
fn places_print_and_parse_back() {
    for &(path, expected, printed) in PATHS {
        assert_eq!(places::parse(path), expected, "path {path:?}");
        let mut out = Writer::new(printed.len());
        places::print(expected, &mut out);
        let bytes = out.finish();
        assert_eq!(bytes.as_ref(), printed, "path {path:?}");
        assert_eq!(places::parse(&bytes), expected, "printed {path:?}");
    }
}

#[test]
fn a_paths_box_holds_exactly_its_printed_bytes() {
    for &(_, place, printed) in PATHS {
        assert_eq!(places::bytes(place).as_ref(), printed, "place {place:?}");
    }
}
#[test]
fn the_list_shows_each_note_by_its_key() {
    let state = fixtures::state();
    let mut view = View::new(&limits::limits());
    state.build(&mut view, &mut Queue::with_capacity(1200));
    let mut keys = skein_lib::List::with_capacity(2);
    for node in view.tree().nodes() {
        if node.role == Role::Item
            && let Some(key) = node.key
        {
            keys.push(key.a).expect("two items fit");
        }
    }
    assert_eq!(keys.as_slice(), &[7, 9]);
}
#[test]
fn the_empty_list_says_no_notes_yet() {
    let state = State::start(1, b"/", &[], 0, b"en");
    let mut view = View::new(&limits::limits());
    state.build(&mut view, &mut Queue::with_capacity(1200));
    let mut found = false;
    for node in view.tree().nodes() {
        for run in &node.text {
            if run.text.as_ref() == b"No notes yet." {
                found = true;
            }
        }
    }
    assert!(found, "empty page explains itself");
}
#[test]
fn sixty_four_notes_at_the_record_ceilings_fit_the_view() {
    let mut state = fixtures::state();
    let mut notes = skein_lib::List::with_capacity(64);
    for number in 0..64 {
        let mut parts = fixtures::notes().notes().get(0).expect("fixture note").clone().into_parts();
        parts.number = number;
        parts.title = Box::from([b't'; 256]);
        parts.body = Box::from([b'b'; 4096]);
        parts.due = Box::from([b'd'; 32]);
        notes
            .push(crate::records::Note::new(&crate::records::CEILINGS, parts).expect("ceiling fits"))
            .expect("64 notes fit");
    }
    state.draft = crate::state::DraftMode::Discarding;
    state.notes = crate::records::Notes::new(&crate::records::CEILINGS, crate::records::NotesParts { notes })
        .expect("64 notes fit");
    let mut view = View::new(&limits::limits());
    state.build(&mut view, &mut Queue::with_capacity(1200));
    assert_eq!(view.tree().len(), 487);
}

fn heading(view: &View) -> &[u8] {
    let mut in_heading = false;
    for node in view.tree().nodes() {
        if in_heading {
            return &node.text.get(0).expect("heading text run").text;
        }
        in_heading = node.role == Role::Heading { level: 1 };
    }
    unreachable!("fixture has a heading")
}
#[test]
fn a_notes_page_shows_its_title_as_the_heading() {
    let mut state = fixtures::state();
    state.place = Place::Note { number: 7 };
    let mut view = View::new(&limits::limits());
    state.build(&mut view, &mut Queue::with_capacity(1200));
    assert_eq!(heading(&view), b"Groceries");
}
#[test]
fn an_unknown_note_says_there_is_none() {
    let state = State::start(1, b"/notes/55", &[], 0, b"en");
    let mut view = View::new(&limits::limits());
    state.build(&mut view, &mut Queue::with_capacity(1200));
    assert_eq!(heading(&view), b"No such note.");
}

#[test]
fn a_notes_page_keeps_the_bodys_line_breaks() {
    let mut state = fixtures::state();
    let mut parts = fixtures::notes().notes().get(0).expect("fixture note").clone().into_parts();
    parts.body = b"Milk\nBread\nButter".as_slice().into();
    let mut notes = skein_lib::List::with_capacity(64);
    notes.push(crate::records::Note::new(&crate::records::CEILINGS, parts).unwrap()).unwrap();
    state.notes = crate::records::Notes::new(&crate::records::CEILINGS, crate::records::NotesParts { notes }).unwrap();
    state.place = Place::Note { number: 7 };
    let mut view = View::new(&limits::limits());
    state.build(&mut view, &mut Queue::with_capacity(1200));
    let mut paragraph = false;
    let mut body = Box::<[u8]>::from([]);
    for node in view.tree().nodes() {
        if paragraph {
            body = node.text.get(0).expect("body text").text.clone();
        }
        paragraph = node.role == Role::Paragraph;
    }
    assert_eq!(body.as_ref(), b"Milk\nBread\nButter");
}
