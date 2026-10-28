//! The page's facts and platform reports (examples.md, 9.2).
use crate::{Place, places, records};
mod events;
use alloc::boxed::Box;
use skein_lib::{List, Queue, Reader, Time, Wall, Writer};
use skein_ui::{Act, Binding, BoundAct, Patch, View};

#[derive(Debug)]
pub struct Record {
    pub key: Box<[u8]>,
    pub value: Box<[u8]>,
}
#[derive(Debug)]
pub enum Report {
    Act(BoundAct),
    Place(Place),
    Answered { exchange: u32, status: u16, body: Box<[u8]> },
    Failed { exchange: u32 },
    Due,
    StreamOpened,
    StreamPiece { bytes: Box<[u8]> },
    StreamEnded,
}
#[derive(Debug)]
pub struct Exchange {
    pub id: u32,
    pub post: bool,
    pub path: Box<[u8]>,
    pub body: Box<[u8]>,
    pub bound: u32,
}
pub type Header = (Box<[u8]>, Box<[u8]>);
#[derive(Debug)]
pub enum StreamDuty {
    Open { path: Box<[u8]>, headers: Box<[Header]>, bound: u32 },
    Close,
}
#[derive(Debug)]
pub struct Duties {
    pub writes: Queue<Record>,
    pub removes: Queue<Box<[u8]>>,
    pub exchanges: Queue<Exchange>,
    pub show: Option<(Place, bool)>,
    pub stream: Option<StreamDuty>,
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DraftMode {
    Editing,
    Discarding,
}
#[derive(Debug)]
pub struct State {
    pub(crate) notes: records::Notes,
    pub(crate) place: Place,
    pub(crate) live: bool,
    pub(crate) words: skein_ui::phrases::Phrasebook,
    shown: u64,
    pub(crate) title: Box<[u8]>,
    pub(crate) title_written: u64,
    pub(crate) title_focus: u64,
    pub(crate) draft: DraftMode,
    pub(crate) body: Box<[u8]>,
    pub(crate) due: Box<[u8]>,
    pub(crate) minutes: Box<[u8]>,
    pub(crate) label: records::Label,
    pub(crate) filter: Option<records::Label>,
    pub(crate) title_error: Option<u16>,
    seed: u64,
    serial: u64,
    pending: List<Option<records::Add>>,
    events: events::Events,
    stream_open: bool,
    starting: bool,
    retry: Option<Time>,
}
impl State {
    #[must_use]
    pub fn start(seed: u64, path: &[u8], records: &[Record], _offset: i32, _languages: &[u8]) -> State {
        #[cfg(debug_assertions)]
        assert!(places::parse(path) != Place::Panic, "the test address panics at start");
        let notes = records::Notes::new(&records::CEILINGS, records::NotesParts { notes: List::with_capacity(64) })
            .expect("empty notes fit");
        let mut pending = List::with_capacity(8);
        for record in records {
            if let Ok(add) = records::Add::decode(&records::CEILINGS, &mut Reader::new(&record.value))
                && record.key == add_key(add.key())
                && pending.room() > 0
            {
                pending.push(Some(add)).expect("pending capacity checked");
            }
        }
        State {
            notes,
            place: places::parse(path),
            live: false,
            words: crate::view::words(),
            shown: 0,
            title: Box::from([]),
            title_written: 0,
            title_focus: 0,
            draft: DraftMode::Editing,
            body: Box::from([]),
            due: Box::from([]),
            minutes: Box::from([]),
            label: records::Label::Idea,
            filter: None,
            title_error: None,
            seed,
            serial: 0,
            pending,
            events: events::Events::new(),
            stream_open: false,
            starting: true,
            retry: None,
        }
    }
    pub fn step(&mut self, report: Report, now: Time, _wall: Wall, duties: &mut Duties) {
        match report {
            Report::Place(place) => self.place = place,
            Report::Act(bound) => match bound.act {
                Act::Press => self.press(bound.binding, duties),
                Act::Input { value } => {
                    self.input(bound.binding, value);
                }
                Act::Submit => {
                    if bound.binding == crate::view::TITLE {
                        self.add(duties);
                    }
                }
                Act::Toggle => {
                    for (filter, binding) in crate::view::FILTERS {
                        if bound.binding == binding {
                            self.filter = filter;
                        }
                    }
                }
                Act::Drawing { data } => {
                    if bound.binding == crate::view::CHART {
                        self.filter = match data.as_ref() {
                            [0] => Some(records::Label::Idea),
                            [1] => Some(records::Label::Task),
                            [2] => Some(records::Label::Reference),
                            _ => self.filter.clone(),
                        };
                    }
                }
            },
            Report::Answered { exchange, status, body } => {
                if exchange == 0 && status == 200 {
                    let mut reader = Reader::new(&body);
                    if let Ok(notes) = records::Notes::decode(&records::CEILINGS, &mut reader) {
                        for note in notes.notes() {
                            self.accept(note.clone());
                        }
                        self.live = self.stream_open;
                    }
                } else if status == 200 {
                    self.add_answered(exchange, &body, duties);
                }
            }
            Report::Failed { .. } => {}
            Report::Due => {
                if self.starting {
                    for (index, add) in self.pending.iter().enumerate() {
                        if let Some(add) = add {
                            duties.exchanges.push(add_exchange(
                                u32::try_from(index).expect("eight slots").checked_add(1).expect("eight slots"),
                                add,
                            ));
                        }
                    }
                    self.starting = false;
                }
                self.retry = None;
                duties.stream = Some(StreamDuty::Open {
                    path: Box::from(&b"/api/events"[..]),
                    headers: Box::from([]),
                    bound: 4096,
                });
            }
            Report::StreamOpened => {
                self.retry = None;
                self.stream_open = true;
                self.events = events::Events::new();
                let bound =
                    u32::try_from(records::Notes::worst_case_bytes(&records::CEILINGS).expect("record bound fits"))
                        .expect("record bound fits u32");
                duties.exchanges.push(Exchange {
                    id: 0,
                    post: false,
                    path: Box::from(&b"/api/notes"[..]),
                    body: Box::from([]),
                    bound,
                });
            }
            Report::StreamPiece { bytes } => {
                for note in &self.events.piece(&bytes) {
                    self.accept(note.clone());
                }
                if self.events.failed() {
                    duties.stream = Some(StreamDuty::Close);
                }
            }
            Report::StreamEnded => {
                self.stream_open = false;
                self.live = false;
                self.retry = Some(now.saturating_add(skein_lib::Duration::from_secs(1)));
            }
        }
        self.shown = self.shown.checked_add(1).expect("shown count cannot exhaust");
    }
    pub fn build(&self, view: &mut View, out: &mut Queue<Patch>) {
        if !view.stale(self.shown) {
            return;
        }
        let mut builder = view.builder();
        crate::view::page(self, &mut builder);
        view.finish(builder, self.shown, out);
    }
    #[must_use]
    pub fn address(&self, binding: Binding) -> Option<Place> {
        if binding.owner != crate::view::OWNER {
            return None;
        }
        if binding == crate::view::BACK {
            return Some(Place::List);
        }
        for note in self.notes.notes() {
            if note.number() == binding.token.raw() {
                return Some(Place::Note { number: note.number() });
            }
        }
        None
    }
    #[must_use]
    pub fn deadline(&self) -> Option<Time> {
        self.retry
    }
    #[must_use]
    pub fn closing_loses(&self) -> bool {
        for add in &self.pending {
            if add.is_some() {
                return true;
            }
        }
        false
    }
    pub(crate) fn shows(&self, label: &records::Label) -> bool {
        match &self.filter {
            Some(filter) => label == filter,
            None => true,
        }
    }
    fn press(&mut self, binding: Binding, duties: &mut Duties) {
        if binding == crate::view::ADD {
            self.add(duties);
        }
        if binding == crate::view::CLEAR {
            self.ask_clear();
        }
        if self.draft == DraftMode::Discarding && binding == crate::view::DISCARD {
            self.clear();
        }
        if self.draft == DraftMode::Discarding && binding == crate::view::KEEP {
            self.draft = DraftMode::Editing;
            self.focus_title();
        }
        for (label, option) in crate::view::LABELS {
            if binding == option {
                self.label = label;
            }
        }
        if let Some(place) = self.address(binding) {
            self.place = place;
            duties.show = Some((place, false));
        }
    }
    fn ask_clear(&mut self) {
        for value in [&self.title, &self.body, &self.due, &self.minutes] {
            if !value.is_empty() {
                self.draft = DraftMode::Discarding;
                return;
            }
        }
        self.clear();
    }
    fn input(&mut self, binding: Binding, value: Box<[u8]>) {
        use crate::view::{BODY, DUE, MINUTES, TITLE};
        let (field, bound) = if binding == TITLE {
            (&mut self.title, 256)
        } else if binding == BODY {
            (&mut self.body, 480)
        } else if binding == DUE {
            (&mut self.due, 32)
        } else if binding == MINUTES {
            (&mut self.minutes, 32)
        } else {
            return;
        };
        if value.len() <= bound {
            *field = value;
        }
        if binding == TITLE && !self.title.is_empty() {
            self.title_error = None;
        }
    }
    pub(crate) fn minute_count(&self) -> Option<u32> {
        if self.minutes.is_empty() {
            return Some(0);
        }
        u32::try_from(skein_ui::number(&self.minutes)?).ok()
    }
    pub(crate) fn due_valid(&self) -> bool {
        let date = &self.due;
        if date.is_empty() {
            return true;
        }
        if date.len() != 10 || date.get(4) != Some(&b'-') || date.get(7) != Some(&b'-') {
            return false;
        }
        let year = skein_ui::number(date.get(..4).expect("date length checked"));
        let month = skein_ui::number(date.get(5..7).expect("date length checked"));
        let day = skein_ui::number(date.get(8..).expect("date length checked"));
        let year = match year {
            Some(year) if year > 0 => year,
            _ => return false,
        };
        let days = match month {
            Some(4 | 6 | 9 | 11) => 30,
            Some(2) if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
            Some(2) => 28,
            Some(1 | 3 | 5 | 7 | 8 | 10 | 12) => 31,
            _ => return false,
        };
        match day {
            Some(day) => day > 0 && day <= days,
            None => false,
        }
    }
    fn focus_title(&mut self) {
        self.title_focus = self.title_focus.checked_add(1).expect("focus count cannot exhaust");
    }
    fn clear(&mut self) {
        self.draft = DraftMode::Editing;
        self.focus_title();
        self.title = Box::from([]);
        self.body = Box::from([]);
        self.due = Box::from([]);
        self.minutes = Box::from([]);
        self.label = records::Label::Idea;
        self.title_error = None;
        self.title_written = self.title_written.checked_add(1).expect("written count cannot exhaust");
    }
    fn add(&mut self, duties: &mut Duties) {
        self.title_error = if self.title.is_empty() { Some(16) } else { None };
        let Some(minutes) = self.minute_count() else { return };
        if self.title_error.is_some() || !self.due_valid() {
            return;
        }
        let mut slot = None;
        for (index, add) in self.pending.iter().enumerate() {
            if add.is_none() {
                slot = Some(index);
                break;
            }
        }
        let index = match slot {
            Some(index) => u32::try_from(index).expect("eight pending slots"),
            None if self.pending.room() > 0 => {
                self.pending.push(None).expect("pending capacity checked");
                self.pending.len().checked_sub(1).expect("slot added")
            }
            None => return,
        };
        let key = self.seed ^ self.serial;
        self.serial = self.serial.checked_add(1).expect("add serial cannot exhaust");
        let add = records::Add::new(
            &records::CEILINGS,
            records::AddParts {
                key,
                title: self.title.clone(),
                body: self.body.clone(),
                due: self.due.clone(),
                minutes,
                label: self.label.clone(),
            },
        )
        .expect("title within its ceiling");
        let exchange = add_exchange(index.checked_add(1).expect("eight pending slots"), &add);
        duties.writes.push(Record { key: add_key(key), value: exchange.body.clone() });
        duties.exchanges.push(exchange);
        *self.pending.get_mut(index).expect("pending slot exists") = Some(add);
        self.clear();
    }
    fn add_answered(&mut self, exchange: u32, body: &[u8], duties: &mut Duties) {
        let Some(index) = exchange.checked_sub(1) else {
            return;
        };
        let Some(slot) = self.pending.get_mut(index) else {
            return;
        };
        if slot.is_none() {
            return;
        }
        let Ok(note) = records::Note::decode(&records::CEILINGS, &mut Reader::new(body)) else {
            return;
        };
        let add = slot.take().expect("pending add exists");
        duties.removes.push(add_key(add.key()));
        self.accept(note);
    }
    fn accept(&mut self, note: records::Note) {
        let mut parts = self.notes.clone().into_parts();
        for held in &parts.notes {
            if held.number() == note.number() {
                return;
            }
        }
        if parts.notes.room() > 0 {
            parts.notes.push(note).expect("note capacity checked");
        }
        self.notes = records::Notes::new(&records::CEILINGS, parts).expect("bounded notes");
    }
}
fn add_key(key: u64) -> Box<[u8]> {
    let mut out = Writer::new(20);
    out.put(b"add-").expect("key fits");
    for digit in key.to_be_bytes() {
        for nibble in [digit >> 4_u32, digit & 15] {
            out.put(&[*b"0123456789abcdef".get(usize::from(nibble)).expect("hex digit")]).expect("key fits");
        }
    }
    out.finish()
}
fn add_exchange(id: u32, add: &records::Add) -> Exchange {
    let mut writer = Writer::new(usize::try_from(add.measure()).expect("record fits"));
    add.encode(&mut writer).expect("measured record fits");
    Exchange {
        id,
        post: true,
        path: Box::from(&b"/api/notes"[..]),
        body: writer.finish(),
        bound: u32::try_from(records::Note::worst_case_bytes(&records::CEILINGS).expect("record bound fits"))
            .expect("record bound fits u32"),
    }
}
