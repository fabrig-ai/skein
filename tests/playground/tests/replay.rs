use skein_io::kernel;
use skein_lib::{Queue, Time, Wall};
use skein_playground::{
    native::Native,
    replay::{Differs, Feed},
    slices::Composition,
};
use skein_playground_world::{
    bare::{self, END, Process, Visit},
    contracts::{Contracts, Input},
    copy_report, duty_bytes,
};
use skein_ui_web::{
    bridge::{
        Duty, Line, Output, Said, Stamp, ToPage, encode_to_page,
        log::{self, End, Log},
    },
    fetch::Report,
    platform::{
        Platform,
        worker::{Bridge, Clock, Heard, Limits, Speed},
    },
    transport::StreamDuty,
};
use skein_world::{Host, Memory, Referee, World};
use std::{cell::RefCell, rc::Rc};

#[global_allocator]
static HEAP: skein_heap::Counting = skein_heap::Counting;
// The bare composition's native slice budget.
const BUDGET: u32 = 16;
const LOG_BYTES: u32 = 16 * 1024;
const CUT_BYTES: u32 = 400; // The header, terminal and first two exchanges fit, the stream open does not.
const DUTIES: u32 = 4; // The visitor fixture's entrance bound.
const REPORTS: usize = 32; // The finite story has fewer reports than inputs plus stream pieces.
const STREAM_PIECE: u32 = 4;
const ID: u32 = 913;

fn send(page: &mut Native<Visit>, id: u32, path: &str) {
    let Duty::Exchange(exchange) = bare::exchange(id, path) else { unreachable!("exchange fixture") };
    page.send(&exchange);
}
fn finish(page: &mut Native<Visit>) {
    page.pace(Speed(3));
    page.run();
    while !page.slices().world().settled() {
        page.advance(page.wake().expect("the fixture's end is due"));
        page.run();
    }
}
fn visit(bound: u32) -> Native<Visit> {
    visit_with_budget(bound, BUDGET)
}
fn visit_with_budget(bound: u32, budget: u32) -> Native<Visit> {
    let mut page = bare::native_with_log(Speed(0), bound, budget);
    send(&mut page, ID, "/echo");
    page.run();
    send(&mut page, ID, "/slow");
    page.run();
    page.pace(Speed(10));
    page.advance(page.wake().expect("slow answer is due"));
    page.run();
    page.pace(Speed(0));
    page.stream(StreamDuty::Open { path: b"/stream/1x8".as_slice().into(), headers: Vec::new(), bound: STREAM_PIECE });
    page.run();
    for _ in 0..3 {
        // Two four-byte pieces, then the stream's terminal.
        page.read_on();
        page.run();
    }
    send(&mut page, ID, "/slow");
    page.abandon(ID);
    page.run();
    send(&mut page, ID, "/print");
    page.run();
    // Check the printed line at an input, rather than only at the end.
    send(&mut page, ID, "/echo");
    page.run();
    finish(&mut page);
    page
}

/// The bare composition, with bounded observations and inputs owned by their process.
struct Recorded {
    process: Process,
    turns: u64,
    inputs: Vec<Option<Duty>>,
    input: usize,
    reports: Vec<Report>,
    bound: u64,
}
impl Recorded {
    fn new(process: Process, inputs: &[log::Input]) -> Self {
        let inputs: Vec<_> = inputs.iter().map(|input| Some(input.duty.clone())).collect();
        let bytes = inputs.iter().flatten().map(duty_bytes).sum::<u64>();
        let bound = process.worst_case()
            + bytes
            + u64::try_from(inputs.capacity() * size_of::<Option<Duty>>() + REPORTS * size_of::<Report>())
                .expect("fixture storage")
            + u64::try_from(REPORTS).expect("report count") * u64::from(skein_playground_world::limits().answer);
        Self { process, turns: 0, inputs, input: 0, reports: Vec::with_capacity(REPORTS), bound }
    }
    fn give(&mut self, duty: &Duty) {
        // Move the process's own bytes at the entrance, keeping the heap meter's ownership.
        let owned = self.inputs[self.input].take().expect("one owned input per logged input");
        assert_eq!(&owned, duty);
        self.input += 1;
        Visit::visitor(&mut self.process).expect("visitor process").duty(owned);
    }
}
impl Host for Recorded {
    fn iterate(&mut self, now: Time, wall: Wall) {
        self.process.iterate(now, wall);
        self.turns += 1;
        if let Some(visitor) = Visit::visitor(&mut self.process) {
            while let Some(report) = visitor.report() {
                assert!(self.reports.len() < REPORTS, "finite report history fits");
                self.reports.push(report);
            }
        }
    }
    fn completions(&mut self) -> &mut Queue<kernel::Complete> {
        self.process.completions()
    }
    fn submissions(&mut self) -> &mut Queue<kernel::Submit> {
        self.process.submissions()
    }
    fn work_pending(&self, now: Time) -> bool {
        self.process.work_pending(now)
    }
    fn next_deadline(&self) -> Option<Time> {
        self.process.next_deadline()
    }
    fn next_policy_deadline(&self) -> Option<Time> {
        self.process.next_policy_deadline()
    }
    fn is_empty(&self) -> bool {
        self.process.is_empty()
    }
    fn worst_case(&self) -> u64 {
        self.bound
    }
    fn operations(&self) -> u32 {
        self.process.operations()
    }
}

type Verdict = Rc<RefCell<Option<Result<(), Differs>>>>;
struct Replay {
    feed: Option<Feed>,
    verdict: Verdict,
    reports: usize,
    lines: usize,
    cut: bool,
    differs: Option<Differs>,
    contracts: Contracts,
}
impl Referee<Recorded> for Replay {
    fn act(&mut self, now: Time, processes: &mut [Recorded]) {
        let step = processes[0].turns;
        let feed = self.feed.as_mut().expect("feed until the world stops");
        while self.differs.is_none() {
            let Some(stamp) = feed.next() else {
                break;
            };
            assert!(stamp.step >= step, "the harness never passes an input's recorded step");
            if stamp.step != step || stamp.instant > now {
                break;
            }
            assert_eq!(stamp.instant, now, "each input enters at its instant");
            match feed.take(Stamp { step, instant: now }) {
                Ok(duty) => {
                    self.contracts.input(Input::of(&duty));
                    processes[1].give(&duty);
                }
                Err(differs) => {
                    self.differs = Some(differs);
                    break;
                }
            }
        }
        if self.differs.is_some() || (self.cut && feed.next().is_none() && self.contracts.is_settled()) {
            for process in processes {
                process.process.close();
            }
        }
    }
    fn observe(&mut self, _now: Time, processes: &[Recorded]) {
        // Observe a whole step after both turns, matching a one-step slice's posts.
        if processes[0].turns != processes[1].turns {
            return;
        }
        let Some(feed) = &mut self.feed else {
            return;
        };
        for report in &processes[1].reports[self.reports..] {
            self.contracts.report(report);
            feed.said(&Said::Report(copy_report(report)));
        }
        self.reports = processes[1].reports.len();
        for _ in self.lines..processes[0].process.printed() {
            feed.said(&Said::Line(Line {
                process: b"scripted server".as_slice().into(),
                stream: Output::Standard,
                line: b"a scripted line".as_slice().into(),
            }));
        }
        self.lines = processes[0].process.printed();
        if processes.iter().all(Host::is_empty) {
            self.contracts.settled();
            feed.said(&Said::Stopped);
            let feed = self.feed.take().expect("finish once");
            *self.verdict.borrow_mut() = Some(match self.differs.take() {
                Some(differs) => Err(differs),
                None => feed.finish(),
            });
        }
    }
    fn next_deadline(&self) -> Option<Time> {
        self.feed.as_ref().and_then(Feed::next).map(|stamp| stamp.instant)
    }
    fn overdue(&self, _now: Time) -> Option<String> {
        None
    }
    fn passed(&self) -> bool {
        self.verdict.borrow().is_some()
    }
}
fn replay(log: Log, different: bool) -> (Result<(), Differs>, u32, Time) {
    let verdict = Rc::new(RefCell::new(None));
    let mut config = skein_sim::Config::calm();
    config.wall = log.start.start;
    let inputs = &log.inputs;
    // The process's owned copy is made inside spawn's metered constructor.
    let process_inputs: Vec<_> = inputs
        .iter()
        .map(|input| log::Input { stamp: input.stamp, duty: input.duty.clone(), digest: input.digest })
        .collect();
    let seed = log.start.seed;
    let referee = Replay {
        cut: log.end == End::Cut,
        feed: Some(Feed::new(log)),
        verdict: verdict.clone(),
        reports: 0,
        lines: 0,
        differs: None,
        contracts: Contracts::new(),
    };
    let mut world = World::new(seed, config, referee, Memory::Checked);
    world.spawn(|| Recorded::new(Process::server(Some(END), different, None), &[]));
    world.spawn(|| Recorded::new(Process::visitor(Some(END), DUTIES), &process_inputs));
    let outcome = world.run();
    let steps = outcome.iterations;
    let end = outcome.end;
    assert!(outcome.procs.iter().all(Host::is_empty));
    assert!(outcome.heap.as_ref().expect("checked heap").iter().all(|(peak, bound)| peak <= bound));
    drop(outcome); // Checks that every process frees exactly the heap it owned.
    let result = verdict.borrow_mut().take().expect("the referee finished");
    (result, steps, end)
}

#[test]
fn a_visit_logged_in_memory_replays_under_the_harness() {
    let mut page = visit(LOG_BYTES);
    let messages = bare::heard(&mut page);
    assert!(messages.iter().any(|heard| matches!(heard, Heard::Line(_))));
    assert!(messages.contains(&Heard::Stopped));
    let log = log::read(&page.log()).unwrap();
    assert_eq!(log.inputs.len(), 10);
    assert!(log.inputs.iter().any(|input| input.stamp.instant > Time::ZERO));
    let (result, steps, end) = replay(log, false);
    assert_eq!(result, Ok(()));
    assert_eq!(u64::from(steps), page.slices().world().steps());
    assert_eq!(end, page.slices().world().now());
}
#[test]
fn a_replay_that_differs_fails_at_that_input() {
    let page = visit(LOG_BYTES);
    let log = log::read(&page.log()).unwrap();
    let (result, _, _) = replay(log, true);
    let differs = result.unwrap_err();
    assert_eq!(differs.input, 1);
    assert_eq!(differs.build.as_ref(), bare::BUILD);
    assert_ne!(differs.logged, differs.replayed);
}
#[test]
fn a_cut_log_replays_to_its_cut() {
    let page = visit(CUT_BYTES);
    let bytes = page.log();
    assert!(bytes.len() <= usize::try_from(CUT_BYTES).unwrap());
    let log = log::read(&bytes).unwrap();
    assert_eq!(log.end, End::Cut);
    assert_eq!(log.inputs.len(), 2);
    assert_eq!(replay(log, false).0, Ok(()));
}
#[test]
fn a_log_is_kept_across_a_worlds_failure() {
    let mut page =
        Bridge::new(bare::start(), bare::BUILD, Limits { exchanges: 2, log: LOG_BYTES }, Speed(1), Time::ZERO);
    while page.outgoing().is_some() {}
    let Duty::Exchange(exchange) = bare::exchange(ID, "/echo") else { unreachable!("exchange fixture") };
    page.send(&exchange);
    let sent = skein_ui_web::bridge::decode_to_world(&page.outgoing().unwrap()).unwrap();
    let clock = Clock { step: 7, now: Time::ZERO, wall: bare::start().start, idle: false, due: None };
    let stamp = Stamp { step: clock.step, instant: clock.now };
    page.incoming(&encode_to_page(&ToPage { clock, said: Said::Stamp(stamp) }), Time::ZERO).unwrap();
    let before = page.log();
    let failed = page
        .incoming(
            &encode_to_page(&ToPage { clock, said: Said::Failed { words: b"world failed".as_slice().into() } }),
            Time::ZERO,
        )
        .unwrap();
    assert_eq!(failed, Heard::Failed { words: b"world failed".as_slice().into() });
    assert_eq!(page.log(), before);
    let log = log::read(&page.log()).unwrap();
    assert_eq!(log.inputs.len(), 1);
    assert_eq!(skein_ui_web::bridge::ToWorld::Duty(log.inputs[0].duty.clone()), sent);
    assert_eq!(log.inputs[0].stamp, stamp);
}

#[test]
fn a_logs_digests_do_not_depend_on_the_slice_budget() {
    let one = visit_with_budget(LOG_BYTES, 1);
    let sixteen = visit(LOG_BYTES);
    let left = log::read(&one.log()).unwrap();
    let right = log::read(&sixteen.log()).unwrap();
    assert_eq!(one.slices().world().steps(), sixteen.slices().world().steps());
    assert_eq!(one.slices().world().now(), sixteen.slices().world().now());
    assert_eq!(one.slices().world().processes().len(), sixteen.slices().world().processes().len());
    assert_eq!(
        left.inputs.iter().map(|input| input.stamp).collect::<Vec<_>>(),
        right.inputs.iter().map(|input| input.stamp).collect::<Vec<_>>()
    );
    assert_eq!(
        left.inputs.iter().map(|input| input.digest).collect::<Vec<_>>(),
        right.inputs.iter().map(|input| input.digest).collect::<Vec<_>>()
    );
}
