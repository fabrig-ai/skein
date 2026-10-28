//! The list page and its English phrasebook (examples.md, 9.1).
use crate::{Place, State, records::Label};
use alloc::boxed::Box;
use skein_lib::{List, Token};
use skein_ui::phrases::{
    Form, Language, Phrase, PhraseId, PhraseLimits, PhraseShape, Phrasebook, PluralClass, PluralRule, Segment,
    SpanUnit, WallFormat,
};
use skein_ui::{Binding, Builder, EntryKind, Key, Owner, Role, Run, Shortcut, Style};
pub(crate) const OWNER: Owner = Owner { number: 1 };
pub(crate) const TITLE: Binding = Binding { owner: OWNER, token: Token::new(u64::MAX) };
pub(crate) const ADD: Binding = Binding { owner: OWNER, token: Token::new(u64::MAX - 1) };
pub(crate) const BACK: Binding = Binding { owner: OWNER, token: Token::new(u64::MAX - 2) };
pub(crate) const BODY: Binding = Binding { owner: OWNER, token: Token::new(u64::MAX - 3) };
pub(crate) const DUE: Binding = Binding { owner: OWNER, token: Token::new(u64::MAX - 4) };
pub(crate) const MINUTES: Binding = Binding { owner: OWNER, token: Token::new(u64::MAX - 5) };
pub(crate) const CLEAR: Binding = Binding { owner: OWNER, token: Token::new(u64::MAX - 6) };
pub(crate) const DISCARD: Binding = Binding { owner: OWNER, token: Token::new(u64::MAX - 10) };
pub(crate) const KEEP: Binding = Binding { owner: OWNER, token: Token::new(u64::MAX - 11) };
pub(crate) const LABELS: [(Label, Binding); 3] = [
    (Label::Idea, Binding { owner: OWNER, token: Token::new(u64::MAX - 7) }),
    (Label::Task, Binding { owner: OWNER, token: Token::new(u64::MAX - 8) }),
    (Label::Reference, Binding { owner: OWNER, token: Token::new(u64::MAX - 9) }),
];
pub(crate) const CHART: Binding = Binding { owner: OWNER, token: Token::new(u64::MAX - 12) };
pub(crate) const FILTERS: [(Option<Label>, Binding); 4] = [
    (None, Binding { owner: OWNER, token: Token::new(u64::MAX - 13) }),
    (Some(Label::Idea), Binding { owner: OWNER, token: Token::new(u64::MAX - 14) }),
    (Some(Label::Task), Binding { owner: OWNER, token: Token::new(u64::MAX - 15) }),
    (Some(Label::Reference), Binding { owner: OWNER, token: Token::new(u64::MAX - 16) }),
];
pub(crate) const CARD: Style = Style { owner: OWNER, token: 0 };
const WORDS: [&[u8]; 27] = [
    b"Notes",
    b"Live",
    b"Offline",
    b"No notes yet.",
    b"idea",
    b"task",
    b"reference",
    b"No such note.",
    b"Title",
    b"Add",
    b"Back to notes",
    b"Body",
    b"Due date",
    b"Minutes",
    b"Label",
    b"Clear",
    b"Enter a title.",
    b"Use YYYY-MM-DD for the due date.",
    b"Enter a whole number of minutes.",
    b"Discard draft?",
    b"Discard",
    b"Keep",
    b"All notes",
    b"Ideas",
    b"Tasks",
    b"References",
    b"Notes by label",
];
const WORD_LIMITS: PhraseLimits =
    PhraseLimits { languages: 1, phrases: 27, holes: 0, segments: 27, text_bytes: 384, hole_bytes: 0 };

pub(crate) fn words() -> Phrasebook {
    let mut shapes = List::with_capacity(27);
    let mut phrases = List::with_capacity(27);
    for (number, text) in WORDS.iter().enumerate() {
        let id = PhraseId { owner: OWNER, number: u16::try_from(number).expect("declared phrases fit") };
        shapes.push(PhraseShape { id, holes: Box::from([]), plural: None }).expect("declared shapes fit");
        let segment = Segment::Text(Box::from(*text));
        let form = Form { class: PluralClass::Other, segments: Box::from([segment]) };
        phrases.push(Phrase { id, forms: Box::from([form]) }).expect("declared phrases fit");
    }
    let language = Language {
        plural: PluralRule::English,
        span_unit: SpanUnit::Minutes,
        wall: WallFormat::Seconds,
        phrases: phrases.into_boxed(),
    };
    Phrasebook::load(shapes.into_boxed(), Box::from([language]), &WORD_LIMITS)
        .expect("English phrases cover every shape")
}

fn phrase(state: &State, number: u16) -> Box<[u8]> {
    state.words.say(0, PhraseId { owner: OWNER, number }, &[]).expect("declared English phrase").text
}
fn text(builder: &mut Builder, role: Role, bytes: &[u8]) {
    builder.open(role);
    builder.text(Run::Plain, bytes);
    builder.close();
}
pub(crate) fn page(state: &State, builder: &mut Builder) {
    builder.open(Role::Page);
    builder.open(Role::Banner);
    text(builder, Role::Text, &phrase(state, 0));
    text(builder, Role::Status, &phrase(state, if state.live { 1 } else { 2 }));
    builder.close();
    builder.open(Role::Main);
    match state.place {
        Place::List => {
            text(builder, Role::Heading { level: 1 }, &phrase(state, 0));
            list(state, builder);
            form(state, builder);
        }
        #[cfg(debug_assertions)]
        Place::Panic => text(builder, Role::Paragraph, &phrase(state, 7)),
        Place::Note { number } => note(state, number, builder),
        Place::Missing => text(builder, Role::Heading { level: 1 }, &phrase(state, 7)),
    }
    builder.close();
    if state.place == Place::List {
        chart(state, builder);
    }
    if state.draft == crate::state::DraftMode::Discarding {
        dialog(state, builder);
    }
    builder.close();
}
fn dialog(state: &State, builder: &mut Builder) {
    builder.open(Role::Dialog);
    builder.name(&phrase(state, 19));
    for (binding, name) in [(DISCARD, 20), (KEEP, 21)] {
        builder.open(Role::Button);
        builder.name(&phrase(state, name));
        builder.bind(binding);
        if binding == KEEP {
            builder.shortcut(Shortcut::ESCAPE);
        }
        builder.close();
    }
    builder.close();
}
fn list(state: &State, builder: &mut Builder) {
    let mut any = false;
    for note in state.notes.notes() {
        if state.shows(note.label()) {
            any = true;
        }
    }
    if !any {
        text(builder, Role::Paragraph, &phrase(state, 3));
        return;
    }
    builder.open(Role::List);
    for note in state.notes.notes() {
        if !state.shows(note.label()) {
            continue;
        }
        builder.open(Role::Item);
        builder.key(Key { owner: OWNER, kind: 0, a: note.number(), b: 0 });
        builder.open(Role::Link);
        builder.text(Run::Plain, note.title());
        builder.link(Binding { owner: OWNER, token: Token::new(note.number()) });
        builder.close();
        text(builder, Role::Time, note.due());
        let label = match note.label() {
            Label::Idea => 4,
            Label::Task => 5,
            Label::Reference => 6,
        };
        text(builder, Role::Text, &phrase(state, label));
        builder.close();
    }
    builder.close();
}

fn form(state: &State, builder: &mut Builder) {
    builder.open(Role::Form);
    builder.name(&phrase(state, 9));
    builder.open(Role::TextField);
    builder.name(&phrase(state, 8));
    builder.value(&state.title, state.title_written);
    if state.title_focus > 0 {
        builder.focus(state.title_focus);
    }
    builder.state(skein_ui::State::SubmitsOnEnter);
    builder.state(skein_ui::State::Required);
    invalid(state, builder, state.title_error);
    builder.bind(TITLE);
    builder.close();
    builder.open(Role::TextArea);
    builder.style(Style { owner: OWNER, token: 2 });
    builder.name(&phrase(state, 11));
    builder.value(&state.body, state.title_written);
    builder.bind(BODY);
    builder.close();
    for (binding, name, kind, value, bad, why) in [
        (DUE, 12, EntryKind::Date, &state.due, !state.due_valid(), 17),
        (MINUTES, 13, EntryKind::Number, &state.minutes, state.minute_count().is_none(), 18),
    ] {
        builder.open(Role::TextField);
        builder.name(&phrase(state, name));
        builder.entry(kind);
        builder.value(value, state.title_written);
        invalid(state, builder, if bad { Some(why) } else { None });
        builder.bind(binding);
        builder.close();
    }
    builder.open(Role::Select);
    builder.name(&phrase(state, 14));
    for ((label, binding), name) in LABELS.into_iter().zip(4..7) {
        builder.open(Role::Option);
        builder.name(&phrase(state, name));
        if state.label == label {
            builder.state(skein_ui::State::Selected);
        }
        builder.bind(binding);
        builder.close();
    }
    builder.close();
    builder.open(Role::Button);
    builder.bind(CLEAR);
    builder.text(Run::Plain, &phrase(state, 15));
    builder.close();
    builder.open(Role::Button);
    builder.name(&phrase(state, 9));
    builder.bind(ADD);
    builder.close();
    builder.close();
}
fn invalid(state: &State, builder: &mut Builder, why: Option<u16>) {
    if let Some(why) = why {
        builder.state(skein_ui::State::Invalid);
        builder.description(&phrase(state, why));
    }
}

fn note(state: &State, number: u64, builder: &mut Builder) {
    let mut found = false;
    for note in state.notes.notes() {
        if note.number() != number {
            continue;
        }
        found = true;
        builder.open(Role::Region);
        builder.name(note.title());
        builder.style(CARD);
        text(builder, Role::Heading { level: 1 }, note.title());
        builder.open(Role::Paragraph);
        let mut start = 0;
        for (index, byte) in note.body().iter().enumerate() {
            if index.saturating_sub(start) >= 252 && byte & 0xc0 != 0x80 {
                builder.text(Run::Plain, note.body().get(start..index).expect("body chunk"));
                start = index;
            }
        }
        builder.text(Run::Plain, note.body().get(start..).expect("last body chunk"));
        builder.close();
        text(builder, Role::Time, note.due());
        let minutes = skein_lib::Decimal::of(u64::from(note.minutes()));
        text(builder, Role::Text, minutes.as_bytes());
        let label = match note.label() {
            Label::Idea => 4,
            Label::Task => 5,
            Label::Reference => 6,
        };
        text(builder, Role::Text, &phrase(state, label));
        builder.close();
    }
    if !found {
        text(builder, Role::Heading { level: 1 }, &phrase(state, 7));
    }
    builder.open(Role::Link);
    builder.text(Run::Plain, &phrase(state, 10));
    builder.link(BACK);
    builder.close();
}

fn chart(state: &State, builder: &mut Builder) {
    let mut counts = [0_u8; 3];
    for note in state.notes.notes() {
        let index = match note.label() {
            Label::Idea => 0,
            Label::Task => 1,
            Label::Reference => 2,
        };
        let count = counts.get_mut(index).expect("three label indices");
        *count = count.checked_add(1).expect("64 notes");
    }
    let labels = [phrase(state, 4), phrase(state, 5), phrase(state, 6)];
    let mut data = List::with_capacity(64);
    for (count, label) in counts.into_iter().zip(&labels) {
        data.push(count).expect("three label counts fit");
        data.push(u8::try_from(label.len()).expect("label word fits")).expect("label lengths fit");
        for byte in label {
            data.push(*byte).expect("label words fit");
        }
    }
    builder.open(Role::Complementary);
    builder.name(&phrase(state, 26));
    builder.drawing(crate::draw::KIND, &phrase(state, 26), data.as_slice(), 64);
    builder.bind(CHART);
    builder.open(Role::List);
    for (count, label) in counts.into_iter().zip(&labels) {
        builder.open(Role::Item);
        builder.text(Run::Plain, crate::draw::caption(label, count).as_slice());
        builder.close();
    }
    builder.close();
    builder.close();
    builder.open(Role::ChoiceGroup);
    builder.name(&phrase(state, 14));
    for ((filter, binding), name) in FILTERS.into_iter().zip(22..26) {
        builder.open(Role::Choice);
        builder.name(&phrase(state, name));
        builder.bind(binding);
        if state.filter == filter {
            builder.state(skein_ui::State::Selected);
        }
        builder.close();
    }
    builder.close();
    builder.close();
}
