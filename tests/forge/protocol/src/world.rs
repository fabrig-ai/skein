use crate::driver::Driver;
use crate::fixtures::{self, LIMITS, Shape};
use crate::referee::{Accounting, Referee};
use skein_fake_forge_domain::{
    self as fake, Observation,
    api::{self as raw, Kind},
};
use skein_forge::api::{Answer, Error, Op, Read, Repository, Write};
use skein_forge_client::{
    self as client, Attempt, Entry, Event, Key, Outcome, RecoveryClock, Request, Resource, Stored,
};
use skein_forge_connection::Component;
use skein_forge_machine_world::{
    fixtures as machine,
    joint::{self, Joint, REPO},
};
use skein_lib::{Duration, Env, Map, Queue, Time, Token};
use std::collections::VecDeque;

const TURN: Duration = Duration::from_millis(10);
const MIN_TURN: Duration = Duration::from_nanos(100_000);
const TURNS: u32 = 100_000;

#[derive(Debug)]
struct Pending {
    client: Token,
    generation: u64,
    write: bool,
}

struct Delivery {
    generation: Option<u64>,
    event: Event,
    write: bool,
}

struct Release {
    decision: usize,
    write: bool,
    covered: bool,
}

struct Delivered {
    token_generation: u64,
    client_generation: u64,
}

enum Decision {
    Make(Box<Entry>),
    Output(Vec<Request>),
}

/// Scripted durable owner; finish settles its accepted calls.
pub struct World {
    referee: Referee,
    client: client::Domain,
    env: Env<client::Limits>,
    out: Queue<Request>,
    joint: Driver,
    durable: Map<Key, Stored>,
    entries: Map<u64, Entry>,
    held: Map<u64, Entry>,
    writers: Vec<Resource>,
    pending: Map<u64, Pending>,
    deliveries: VecDeque<Delivery>,
    decisions: Vec<Decision>,
    releases: Vec<Release>,
    delivered: Vec<Delivered>,
    outcomes: Vec<(u64, Outcome)>,
    read_results: Map<Token, Result<Answer, Error>>,
    changes: Map<Resource, Answer>,
    owner_sequence: u64,
    hints: u64,
    shape: Shape,
    sequence: u64,
    generation: u64,
    discarded: u64,
    lose_creation: bool,
    slow_creation: bool,
    seed: u64,
}

impl World {
    #[must_use]
    pub fn new(seed: u64, cut_chance: u32) -> Self {
        Self::with(seed, cut_chance, Shape::Plain)
    }

    pub(crate) fn with(seed: u64, cut_chance: u32, shape: Shape) -> Self {
        let settings = shape.settings();
        let mut referee = Referee::default();
        referee.settings(&settings);
        let joint = match shape {
            Shape::Plain | Shape::Hooked => {
                let config = match shape {
                    Shape::Plain => machine::protocol_config(),
                    Shape::Hooked => machine::hooked_protocol_config(),
                    Shape::Github { .. } => unreachable!("Forgejo fixture branch"),
                };
                Driver::Forgejo(Box::new(Joint::with(
                    seed,
                    cut_chance,
                    config,
                    shape.limits(),
                    settings,
                    fixtures::shared(),
                    joint::checks(1000),
                )))
            }
            Shape::Github { log_poll } => {
                Driver::Github(Box::new(crate::github::Github::new(seed, cut_chance, log_poll)))
            }
        };
        let mut world = Self {
            referee,
            client: configured(seed, shape.author()),
            env: Env { now: joint.now(), wall: joint.wall(), limits: LIMITS },
            out: Queue::uncapped(),
            joint,
            durable: Map::with_capacity(LIMITS.resources + LIMITS.repositories),
            entries: Map::with_capacity(LIMITS.entries),
            held: Map::with_capacity(LIMITS.entries),
            writers: Vec::new(),
            pending: Map::with_capacity(LIMITS.calls),
            deliveries: VecDeque::new(),
            decisions: Vec::new(),
            releases: Vec::new(),
            delivered: Vec::new(),
            outcomes: Vec::new(),
            read_results: Map::with_capacity(LIMITS.pending),
            changes: Map::with_capacity(LIMITS.resources),
            owner_sequence: 0,
            hints: 0,
            shape,
            sequence: 0,
            generation: 0,
            discarded: 0,
            lose_creation: false,
            slow_creation: false,
            seed,
        };
        if let Shape::Github { .. } = shape {
            let access = crate::github_fixture::access(crate::github_fixture::key());
            world.referee.access(&access);
            let request = skein_forge_connection::Request::Connect { forge: 1, access };
            world.referee.fact(format!("{request:?}"));
            world.joint.action(request);
        }
        world.event(Event::Restored { clock: RecoveryClock::Monotonic });
        world
    }

    /// A Forgejo repository with dispatch-only workflows and a seeded tree.
    #[must_use]
    pub fn turn(seed: u64, workflows: Box<[raw::Workflow]>, tree: Box<[raw::File]>) -> Self {
        let mut world = Self::new(seed, 0);
        let Driver::Forgejo(joint) = &mut world.joint else { unreachable!("Forgejo world") };
        joint.fake_env.limits.limits.workflows = u32::try_from(workflows.len()).expect("fixture workflows");
        joint.fake_env.limits.limits.workflow_runs = 4;
        let mut domain = fake::Domain::new(&joint.fake_env.limits, seed);
        fake::repository(
            &mut domain,
            &joint.fake_env.limits,
            raw::Setup {
                id: 2,
                name: Box::from(b"org/repo".as_slice()),
                default: Box::from(b"main".as_slice()),
                tree,
                workflows,
                labels: Box::new([]),
                checks: raw::Checks { contexts: Box::new([]), ..joint::checks(1000) },
                protection: None,
                hooked: false,
            },
        );
        fake::grant(&mut domain, b"org/repo", 1, raw::Permission::Admin);
        fake::grant(&mut domain, b"org/repo", 2, raw::Permission::Write);
        joint.domain = domain;
        world
    }
    /// A person's fixture call outside the component's HTTP traffic.
    /// # Errors
    /// Returns the fake domain's refusal.
    pub fn direct(&mut self, op: raw::Op) -> Result<raw::Answer, raw::Error> {
        self.joint.direct(op)
    }
    /// A snapshot used by the story's independent expectations.
    /// # Errors
    /// Returns the fake domain's refusal of the read.
    pub fn inspect(&self, read: &raw::Read) -> Result<raw::Answer, raw::Error> {
        self.joint.domain().inspect(&self.joint.fake_env().limits, self.joint.path(), read)
    }
    /// An immutable commit prepared before a person's push.
    pub fn prepare_commit(&mut self, parent: u64, tree: Box<[raw::File]>, message: &[u8]) -> u64 {
        let Driver::Forgejo(joint) = &mut self.joint else { unreachable!("Forgejo world") };
        fake::commit(&mut joint.domain, &joint.fake_env.limits, parent, tree, message)
            .expect("fixture commit")
            .expect("changed tree")
    }
    /// A durable write through the client's owner and the real dialect.
    pub fn write(&mut self, write: skein_forge::api::Write) -> Outcome {
        let condition = match &write {
            Write::Update { number, .. } | Write::Merge { number, .. } => {
                let Answer::Pull(pull) = self.read(Read::Pull { number: *number }).expect("write precondition") else {
                    panic!("pull precondition")
                };
                if let Write::Update { .. } = write {
                    client::Condition::Update { head: pull.commit, base: pull.base_commit.expect("base tip") }
                } else {
                    client::Condition::Merge { base: pull.base }
                }
            }
            Write::CreateIssue { .. }
            | Write::OpenPull { .. }
            | Write::Post { .. }
            | Write::Review { .. }
            | Write::Edit { .. }
            | Write::SetReviewers { .. }
            | Write::Close { .. }
            | Write::Reopen { .. }
            | Write::Status { .. }
            | Write::Dispatch { .. }
            | Write::Rerun { .. }
            | Write::CreateBranch { .. }
            | Write::DeleteBranch { .. } => client::Condition::None,
        };
        self.owner_sequence += 1;
        let number = self.owner_sequence;
        self.event(Event::Make {
            entry: Entry {
                number,
                task: 1,
                repository: REPO,
                effect: skein_forge_client::Effect { write, condition },
                start: None,
                attempt: None,
                failures: 0,
                queued: false,
            },
        });
        let end = self.joint.now().saturating_add(Duration::from_secs(20));
        for _ in 0..TURNS {
            if let Some((_, outcome)) = self.outcomes.iter().find(|(entry, _)| *entry == number) {
                return *outcome;
            }
            assert!(self.iterate(end), "write settles within 20 seconds");
        }
        panic!("write step budget")
    }

    /// One GitHub forge, with its App access handed at runtime.
    #[must_use]
    pub fn github(seed: u64, cut_chance: u32) -> Self {
        Self::github_with_log_poll(seed, cut_chance, fixtures::limits().log_poll)
    }

    /// A scenario's log interval, retained across component restarts.
    #[must_use]
    pub fn github_with_log_poll(seed: u64, cut_chance: u32, log_poll: Duration) -> Self {
        Self::with(seed, cut_chance, Shape::Github { log_poll })
    }

    /// Replace the fake App's key and the component's access while calls run.
    pub fn replace_github_access(&mut self) {
        let end = self.joint.now().saturating_add(Duration::from_secs(5));
        for _ in 0..TURNS {
            if !self.pending.is_empty() {
                break;
            }
            assert!(self.iterate(end), "owner releases a call before key replacement");
        }
        assert!(!self.pending.is_empty(), "replacement crosses an admitted call");
        let written = self.joint.accounting().written;
        for _ in 0..10_000 {
            self.joint.tick();
            if self.joint.accounting().written > written {
                break;
            }
            self.joint.time(self.joint.now().saturating_add(MIN_TURN));
        }
        assert!(self.joint.accounting().written > written, "old access wrote a request before replacement");
        let key = skein_fake_forge_github::keys::KEYS[1];
        let access = crate::github_fixture::access(key);
        self.referee.access(&access);
        self.referee.fact(format!("{:?}", skein_forge_connection::Request::Connect { forge: 1, access }));
        self.joint.rotate(key);
    }

    /// Delivery-log request lines observed independently of call costs.
    #[must_use]
    pub fn github_log_reads(&self) -> usize {
        self.joint.lines().iter().filter(|line| line.starts_with(b"GET /app/hook/deliveries")).count()
    }

    /// Repository discovery request lines, independent of call costs.
    #[must_use]
    pub fn github_repository_reads(&self) -> usize {
        self.joint.lines().iter().filter(|line| line.starts_with(b"GET /repositories/")).count()
    }

    /// Branch leaf request lines, independent of call costs.
    #[must_use]
    pub fn github_branch_reads(&self, branch: &[u8]) -> usize {
        let mut prefix = b"GET /repos/".to_vec();
        prefix.extend_from_slice(crate::github_fixture::PATH);
        prefix.extend_from_slice(b"/branches/");
        prefix.extend_from_slice(branch);
        prefix.push(b' ');
        self.joint.lines().iter().filter(|line| line.starts_with(&prefix)).count()
    }

    #[must_use]
    pub fn github_mints(&self) -> u64 {
        self.joint.mints()
    }

    #[must_use]
    pub fn github_credentials(&self) -> (usize, usize, usize) {
        self.referee.credential_counts()
    }

    pub fn event(&mut self, event: Event) {
        self.referee.fact(format!("{event:?}"));
        match &event {
            Event::Make { entry } | Event::Remake { entry } => {
                assert_eq!(entry.repository, REPO);
                self.referee.record(format!("{entry:?}"));
                self.held.remove(&entry.number);
                self.entries.insert(entry.number, entry.clone()).expect("owner outbox capacity");
                self.decisions.push(Decision::Make(Box::new(entry.clone())));
            }
            Event::Read { repository, .. } => assert_eq!(*repository, REPO),
            Event::Writer { resource, taken } => {
                if *taken {
                    if !self.writers.contains(resource) {
                        self.writers.push(resource.clone());
                    }
                } else {
                    self.writers.retain(|claimed| claimed != resource);
                }
            }
            Event::Hint { .. } => self.hints += 1,
            Event::Keep { .. }
            | Event::Pushed { .. }
            | Event::Withdraw { .. }
            | Event::Restore { .. }
            | Event::Restored { .. }
            | Event::PauseOutbox
            | Event::ReadAfresh
            | Event::Settle
            | Event::SettleOutbox
            | Event::Answered { .. } => {}
        }
        self.sync_env();
        client::step(&mut self.client, &self.env, event, &mut self.out);
        self.commit();
        self.client.reclaim();
    }

    pub fn run_for(&mut self, span: Duration) {
        let end = self.joint.now().saturating_add(span);
        for _ in 0..TURNS {
            if !self.iterate(end) {
                return;
            }
        }
        panic!("protocol world did not reach its horizon, seed {}", self.seed);
    }

    /// One read through the client, as an owner's decision read.
    /// # Errors
    /// Returns the client's terminal refusal or failed read.
    pub fn read(&mut self, mut read: Read) -> Result<Answer, Error> {
        let end = self.joint.now().saturating_add(Duration::from_secs(5));
        for _ in 0..100 {
            self.owner_sequence = self.owner_sequence.checked_add(1).expect("owner read sequence bound");
            let owner = Token::new(self.owner_sequence);
            let retry = read.clone();
            self.event(Event::Read { owner, repository: REPO, read });
            let mut result = None;
            for _ in 0..TURNS {
                if let Some(answer) = self.read_results.remove(&owner) {
                    result = Some(answer);
                    break;
                }
                assert!(self.iterate(end), "owner read exceeded 5 s, seed {}", self.seed);
            }
            let result = result.unwrap_or_else(|| panic!("owner read did not settle, seed {}", self.seed));
            if result != Err(Error::Busy) {
                return result;
            }
            assert!(self.iterate(end), "owner read exceeded 5 s, seed {}", self.seed);
            read = retry;
        }
        panic!("owner read busy after 100 tries, seed {}", self.seed);
    }

    /// Seeds a visible commit without changing either world's default repository.
    #[must_use]
    pub fn tree(&mut self, tree: Box<[raw::File]>) -> skein_forge::api::Commit {
        let raw::Answer::Commit(parent) = self
            .joint
            .direct(raw::Op::Read(raw::Read::Branch { branch: Box::from(b"main".as_slice()) }))
            .expect("main")
        else {
            panic!("branch head")
        };
        let head = self.joint.commit_tree(parent, tree);
        self.joint
            .direct(raw::Op::Git(raw::Git::Push {
                branch: Box::from(b"main".as_slice()),
                commit: head,
                expected: None,
            }))
            .expect("push tree");
        let Answer::Commit(head) =
            self.read(Read::Branch { branch: Box::from(b"main".as_slice()) }).expect("visible branch")
        else {
            panic!("branch head")
        };
        head
    }
    /// The fake's items, read past the client: (number, title, open), by number.
    #[must_use]
    pub fn snapshot(&self) -> Vec<(u64, Box<[u8]>, bool)> {
        let mut rows = Vec::new();
        let mut page = 1;
        loop {
            let read = raw::Read::Items {
                order: raw::Order::LeastUpdated,
                state: None,
                kind: None,
                labels: Box::new([]),
                author: None,
                since: Time::ZERO,
                page,
                limit: self.joint.fake_env().limits.limits.page_size,
            };
            let raw::Answer::Items { items, more, .. } = self
                .joint
                .domain()
                .inspect(&self.joint.fake_env().limits, self.joint.path(), &read)
                .expect("fake snapshot")
            else {
                panic!("fake snapshot must return items, seed {}", self.seed)
            };
            rows.extend(
                items.into_vec().into_iter().map(|item| (item.number, item.title, item.state == raw::State::Open)),
            );
            if !more {
                rows.sort_by_key(|row| row.0);
                return rows;
            }
            page += 1;
        }
    }

    pub fn person_issue(&mut self, title: &[u8]) -> u64 {
        self.joint.issue(title, b"person")
    }

    pub fn person_edit(&mut self, number: u64, title: &[u8]) {
        self.joint
            .direct(raw::Op::Write(raw::Write::EditItem { number, title: Some(Box::from(title)), body: None }))
            .expect("person edits item");
    }

    pub fn person_close(&mut self, number: u64) {
        self.joint.close(number);
    }

    /// Hints routed to a client.
    #[must_use]
    pub fn hints(&self) -> u64 {
        self.hints
    }

    /// The latest answer the client reported for a kept resource.
    #[must_use]
    pub fn changed(&self, resource: &Resource) -> Option<&Answer> {
        self.changes.get(resource)
    }

    pub fn restart(&mut self) {
        self.route();
        self.generation = self.generation.checked_add(1).expect("owner generation bound");
        let settings = self.shape.settings();
        self.referee.settings(&settings);
        let component = Component::new(settings, &self.shape.limits(), &fixtures::shared())
            .expect("component admits client limits");
        self.joint.replace(component);
        self.startup();
        // The replaced component owns no calls from the former process.
        self.pending = Map::with_capacity(LIMITS.calls);
        self.client = configured(self.seed, self.shape.author());
        let records: Vec<_> = self.durable.iter().map(|(_, record)| record.clone()).collect();
        for record in records {
            self.event(Event::Restore { record });
        }
        self.event(Event::Restored { clock: RecoveryClock::Monotonic });
        for resource in self.writers.clone() {
            self.event(Event::Writer { resource, taken: true });
        }
        self.event(Event::PauseOutbox);
        let entries: Vec<_> = self.entries.iter().map(|(_, entry)| entry.clone()).collect();
        for entry in entries {
            assert!(!self.held.contains_key(&entry.number));
            self.event(Event::Make { entry });
        }
        self.event(Event::ReadAfresh);
    }

    pub fn finish(&mut self) {
        let end = self.joint.now().saturating_add(Duration::from_secs(60));
        for _ in 0..TURNS {
            self.route();
            if self.entries.is_empty()
                && self.pending.is_empty()
                && self.deliveries.is_empty()
                && !self.client.is_ready()
                && !self.client.is_due(self.joint.now())
            {
                assert!(
                    self.entries.is_empty() && self.durable.is_empty(),
                    "owner outbox and store settled: seed {}, calls/answers {:?}, accounting {:?}, entries {:?}, durable {:?}",
                    self.seed,
                    self.calls_and_answers(),
                    self.accounting(),
                    self.entries,
                    self.durable
                );
                assert_eq!(self.joint.domain().observations_lost(), 0);
                self.referee.freeze();
                self.referee.fact(format!("{:?}", skein_forge_connection::Request::Close));
                self.joint.action(skein_forge_connection::Request::Close);
                for _ in 0..TURNS {
                    self.joint.tick();
                    self.route();
                    if self.joint.closed() && !self.joint.busy() {
                        return;
                    }
                    assert!(self.joint.now() < end, "component teardown within 60 s, seed {}", self.seed);
                    self.joint.time(self.joint.now().saturating_add(MIN_TURN));
                }
                panic!("component teardown did not settle, seed {}", self.seed);
            }
            assert!(self.iterate(end), "protocol world did not finish within 60 s, seed {}", self.seed);
        }
        panic!("protocol world did not settle, seed {}", self.seed);
    }

    #[must_use]
    pub fn calls_and_answers(&self) -> (u64, u64) {
        self.referee.counts()
    }

    #[must_use]
    pub fn accounting(&self) -> Accounting {
        let mut accounting = self.joint.accounting();
        accounting.reported = self.referee.reported();
        accounting
    }

    #[must_use]
    pub fn secrets_absent(&self) -> bool {
        self.referee.secrets_absent(
            self.joint.lines(),
            self.joint.events(),
            &format!("{:?}", self.joint.observations()),
        )
    }

    pub(crate) fn startup(&mut self) {
        let connected = self.joint.connected();
        // A replacement begins its start synchronously; only Connected releases work.
        for _ in 0..60 {
            if matches!(self.joint.component().start(1), Some(skein_forge_connection::start::Start::Ready { .. })) {
                self.joint.tick();
                return;
            }
            self.joint.run_for(Duration::from_secs(1));
            if self.joint.connected() > connected {
                return;
            }
        }
        panic!("startup did not connect within 60 s, seed {}", self.seed);
    }

    /// Retain the synchronous refusal before the driver's Stopped panic boundary.
    pub(crate) fn reconnect(&mut self) {
        let mut invalid = machine::access(machine::TOKEN, 1, Some(machine::HOOK_SECRET));
        invalid.credential = skein_forge::credential::Credential::Token(skein_lib::Secret::new(Box::from(
            [machine::TOKEN, b"\r"].concat(),
        )));
        for (access, refused) in
            [(invalid, true), (machine::access(machine::TOKEN, 1, Some(machine::HOOK_SECRET)), false)]
        {
            self.referee.access(&access);
            let request = skein_forge_connection::Request::Connect { forge: 1, access };
            self.referee.fact(format!("{request:?}"));
            self.joint.action(request);
            if refused {
                let event = self.joint.pop().expect("synchronous invalid connect terminal");
                self.referee.fact(format!("{event:?}"));
                assert!(matches!(
                    event,
                    skein_forge_connection::Event::Stopped {
                        forge: 1,
                        why: skein_forge_connection::Stopped::Invalid { value: skein_forge_connection::Value::Token }
                    }
                ));
            }
        }
        self.startup();
    }

    /// Restart only after the component's submitted set has all its terminals.
    pub(crate) fn checkpoint(&mut self) {
        let end = self.joint.now().saturating_add(Duration::from_secs(60));
        for _ in 0..TURNS {
            self.route();
            if self.referee.outstanding() == 0 {
                self.restart();
                return;
            }
            assert!(self.iterate(end), "checkpoint did not settle, seed {}", self.seed);
        }
        panic!("checkpoint did not settle, seed {}", self.seed);
    }

    #[must_use]
    pub fn creations(&self) -> u32 {
        u32::try_from(
            self.joint
                .observations()
                .iter()
                .filter(|observation| matches!(observation, Observation::Opened { kind: Kind::Issue, by, .. } if *by == self.shape.author()))
                .count(),
        )
        .expect("world creation count")
    }

    #[must_use]
    pub fn made(&self, entry: u64) -> bool {
        self.outcomes.iter().any(|(number, outcome)| *number == entry && matches!(outcome, Outcome::Made { .. }))
    }

    /// Writes without committed Progress; old-generation answers delivered
    /// to the new client.
    #[must_use]
    pub fn owner_counts(&self) -> (u64, u64) {
        let uncovered = self
            .releases
            .iter()
            .filter(|release| release.write && (!release.covered || release.decision >= self.decisions.len()))
            .count();
        let stale =
            self.delivered.iter().filter(|delivery| delivery.token_generation != delivery.client_generation).count();
        (u64::try_from(uncovered).expect("release count"), u64::try_from(stale).expect("delivery count"))
    }

    /// The latest attempt the decision log committed for the entry, kept
    /// after the entry settles.
    #[must_use]
    pub fn attempt(&self, entry: u64) -> Option<Attempt> {
        for decision in self.decisions.iter().rev() {
            match decision {
                Decision::Make(saved) => {
                    if saved.number == entry {
                        return saved.attempt;
                    }
                }
                Decision::Output(output) => {
                    for request in output.iter().rev() {
                        if let Request::Progress { entry: saved } = request
                            && saved.number == entry
                        {
                            return saved.attempt;
                        }
                    }
                }
            }
        }
        None
    }

    #[must_use]
    pub fn released_writes(&self) -> u64 {
        u64::try_from(self.releases.iter().filter(|release| release.write).count()).expect("release count")
    }

    #[must_use]
    pub fn discarded(&self) -> u64 {
        self.discarded
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn lose_creation_answer(&mut self, entry: Entry) {
        self.lose_creation = true;
        self.event(Event::Make { entry });
        let end = self.joint.now().saturating_add(Duration::from_secs(20));
        for _ in 0..TURNS {
            self.route();
            if self
                .deliveries
                .iter()
                .any(|delivery| delivery.write && matches!(delivery.event, Event::Answered { result: Err(_), .. }))
            {
                return;
            }
            assert!(self.iterate(end), "lost creation terminal not queued within 20 s, seed {}", self.seed);
        }
        panic!("lost creation did not settle, seed {}", self.seed);
    }

    fn sync_env(&mut self) {
        self.env.now = self.joint.now();
        self.env.wall = self.joint.wall();
    }

    fn iterate(&mut self, end: Time) -> bool {
        self.route();
        if let Some(delivery) = self.deliveries.pop_front() {
            if let Some(generation) = delivery.generation {
                if generation != self.generation {
                    self.discarded += 1;
                    return true;
                }
                self.delivered.push(Delivered { token_generation: generation, client_generation: self.generation });
            }
            self.event(delivery.event);
        } else if self.client.is_ready() {
            self.sync_env();
            client::resume(&mut self.client, &self.env, &mut self.out);
            self.commit();
            self.client.reclaim();
        } else if self.client.is_due(self.joint.now()) {
            self.sync_env();
            client::fire(&mut self.client, &self.env, &mut self.out);
            self.commit();
            self.client.reclaim();
        } else {
            let now = self.joint.now();
            if now >= end {
                return false;
            }
            let mut next = self.client.next_deadline().unwrap_or(end).min(end);
            if !self.pending.is_empty() {
                next = next.min(now.saturating_add(TURN));
            }
            next = next.max(now.saturating_add(MIN_TURN)).min(end);
            self.joint.run_for(next.saturating_since(now));
        }
        true
    }

    fn route(&mut self) {
        for secret in self.joint.secrets() {
            self.referee.bearer(secret.expose());
        }
        for (token, answer) in self.joint.answers() {
            let pending = self.pending.remove(&token).expect("terminal names an owner call");
            self.referee.terminal(&answer, pending.generation, pending.client);
            self.deliveries.push_back(Delivery {
                generation: Some(pending.generation),
                event: Event::Answered {
                    call: pending.client,
                    cost: answer.cost,
                    free: answer.free,
                    rate: answer.rate,
                    result: answer.result,
                },
                write: pending.write,
            });
        }
        for hinted in self.joint.hints() {
            self.deliveries.push_back(Delivery {
                generation: None,
                event: Event::Hint { hint: hinted.hint },
                write: false,
            });
        }
        if self.slow_creation && self.creations() > 0 {
            self.joint.restore_latency();
            self.slow_creation = false;
        }
    }

    fn commit(&mut self) {
        let mut decision = Vec::new();
        let mut calls = Vec::new();
        let mut settle = false;
        while let Some(request) = self.out.pop() {
            self.referee.fact(format!("{request:?}"));
            match &request {
                Request::Save { record } => {
                    self.referee.record(format!("{record:?}"));
                    let key = match record {
                        Stored::Live(record) => Key::Live(record.watch.resource.clone()),
                        Stored::Repository(record) => Key::Repository(record.repository),
                        Stored::Writer(writer) => Key::Writer(writer.forge),
                    };
                    self.durable.insert(key, record.clone()).expect("owner store capacity");
                }
                Request::Erase { key } => {
                    self.durable.remove(key).expect("stored row erased");
                }
                Request::Progress { entry } => {
                    self.referee.record(format!("{entry:?}"));
                    self.entries.insert(entry.number, entry.clone()).expect("owner outbox capacity");
                }
                Request::Outcome { entry, outcome, .. } => {
                    match outcome {
                        Outcome::Uncertain | Outcome::Queued => {}
                        Outcome::Made { .. } | Outcome::Failed(_) | Outcome::Raced { .. } | Outcome::Withdrawn => {
                            self.entries.remove(entry).expect("stored entry settled");
                        }
                        Outcome::Held => {
                            let saved = self.entries.remove(entry).expect("stored entry held");
                            self.held.insert(*entry, saved).expect("held entry capacity");
                        }
                    }
                    self.outcomes.push((*entry, *outcome));
                }
                Request::Call { call, repository, op } => calls.push((*call, *repository, op.clone())),
                Request::ReadAfreshDone => settle = true,
                Request::Kept { result, .. } => assert_eq!(*result, Ok(())),
                Request::Changed { resource, result } => {
                    if let Ok(answer) = result {
                        self.changes.insert(resource.clone(), answer.clone()).expect("owner changed capacity");
                    }
                }
                Request::Read { owner, result } => {
                    assert!(self.read_results.insert(*owner, result.clone()).expect("owner read capacity").is_none());
                }
                Request::Drift { .. } | Request::Settled { .. } | Request::OutboxDone => {}
            }
            decision.push(request);
        }
        self.decisions.push(Decision::Output(decision));
        for (call, repository, op) in calls {
            self.submit(call, repository, op);
        }
        if settle {
            self.event(Event::SettleOutbox);
        }
    }

    fn submit(&mut self, call: Token, repository: Repository, op: Op) {
        assert_eq!(repository, REPO);
        let (write, covered) = match &op {
            Op::Write(write) => {
                (true, self.entries.iter().any(|(_, entry)| entry.attempt.is_some() && entry.effect.write == *write))
            }
            Op::Read(_) => (false, true),
        };
        self.sequence = self.sequence.checked_add(1).expect("owner call sequence bound");
        self.pending
            .insert(self.sequence, Pending { client: call, generation: self.generation, write })
            .expect("owner call capacity");
        self.releases.push(Release { decision: self.decisions.len() - 1, write, covered });
        if self.lose_creation && matches!(op, Op::Write(Write::CreateIssue { .. })) {
            self.joint.delay_creation();
            self.lose_creation = false;
            self.slow_creation = true;
        }
        self.referee.submit(self.sequence, self.generation, call);
        self.joint.call(self.sequence, repository, op);
    }
}

fn configured(seed: u64, author: u64) -> client::Domain {
    client::Domain::configured(
        &LIMITS,
        seed,
        client::Config {
            namespace: Box::from(b"kit".as_slice()),
            writers: Box::new([client::Writer { forge: 1, author }]),
        },
    )
    .expect("owner authenticated writer")
}
