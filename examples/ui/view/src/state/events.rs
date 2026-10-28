//! HTTP's event reader over bounded browser pieces (examples.md, 9.2).
use crate::records;
use skein_http::sse;
use skein_lib::{
    Env, Intake, List, Queue, Reader, Time, Wall,
    base64::{self, Alphabet},
    stream::{self, Read},
};
const LIMITS: sse::Limits = sse::Limits { line: 8192, event: 8192, field: 32, chunk: 4096 };
#[derive(Debug)]
pub(super) struct Events {
    reader: sse::Reader,
    intake: Intake,
    wanted: Read,
    data: List<u8>,
    above: Queue<sse::Event>,
    below: Queue<stream::Down>,
    failed: bool,
}
impl Events {
    pub(super) fn new() -> Self {
        let mut events = Self {
            reader: sse::Reader::new(&LIMITS),
            intake: Intake::with_capacity(8192),
            wanted: Read::Nothing,
            data: List::with_capacity(8192),
            above: Queue::with_capacity(4),
            below: Queue::with_capacity(4),
            failed: false,
        };
        events.down(sse::Request::Next);
        events
    }
    fn down(&mut self, request: sse::Request) {
        let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: LIMITS };
        sse::down(&mut self.reader, &env, request, &mut self.above, &mut self.below);
    }
    pub(super) fn piece(&mut self, bytes: &[u8]) -> List<records::Note> {
        let mut notes = List::with_capacity(64);
        if self.failed {
            return notes;
        }
        self.intake.append(bytes).expect("one partial scan and one browser piece fit");
        loop {
            while let Some(event) = self.above.pop() {
                match event {
                    sse::Event::Opened => {
                        self.data.clear();
                        self.data_on();
                    }
                    sse::Event::Data(stream::Up::Bytes(bytes)) => {
                        for byte in &bytes {
                            self.data.push(*byte).expect("event bound holds its data");
                        }
                        self.data_on();
                    }
                    sse::Event::Dispatched(_) => {
                        if let Ok(bytes) = base64::decode(Alphabet::Standard, self.data.as_slice(), 8192)
                            && let Ok(note) = records::Note::decode(&records::CEILINGS, &mut Reader::new(&bytes))
                            && notes.room() > 0
                        {
                            notes.push(note).expect("note capacity checked");
                        }
                        self.down(sse::Request::Next);
                    }
                    sse::Event::Data(stream::Up::End) | sse::Event::Closed => {}
                    sse::Event::Data(stream::Up::Failed(_)) | sse::Event::Ended | sse::Event::Failed(_) => {
                        self.failed = true;
                    }
                    sse::Event::Data(stream::Up::Room) => unreachable!("event data reads only"),
                }
            }
            while let Some(down) = self.below.pop() {
                match down {
                    stream::Down::Demand { read, room: _ } => self.wanted = read,
                    stream::Down::Send(_) | stream::Down::Finish => unreachable!("event reader reads only"),
                }
            }
            if self.failed {
                break;
            }
            let Some(bytes) = self.intake.meet(self.wanted) else {
                break;
            };
            self.wanted = Read::Nothing;
            let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: LIMITS };
            sse::up(&mut self.reader, &env, stream::Up::Bytes(bytes), &mut self.above, &mut self.below);
        }
        notes
    }
    fn data_on(&mut self) {
        self.down(sse::Request::Data(stream::Down::Demand { read: Read::Arrived { max: 1024 }, room: 0 }));
    }
    pub(super) fn failed(&self) -> bool {
        self.failed
    }
}
