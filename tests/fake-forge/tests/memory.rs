//! Memory stays within the worst case (programming-model.md, 6.3), measured by
//! a counting allocator: the forge filled to its limits, every repository
//! holding as many items, comments, reviews, branches, statuses and wiki pages
//! as it may, each as large as it may be, the store as many commits, with
//! calls held, webhooks in flight and observations kept; every step checked.

#![cfg(test)]

use skein_fake_forge_domain::api::{
    Answer, Check, Checks, Cue, Error, File, Git, Op, Order, Permission, Protection, Read, Setup, Verdict, Write,
};
use skein_fake_forge_domain::{Config, Domain, Event, Limits, MAX_OUT, Request, Skew, fire, step, worst_case};
use skein_lib::{Duration, Env, List, Queue, ReplyTo, Time, Token, Wall, Writer};
use skein_world::domain::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

const LIMITS: Limits = Limits {
    repositories: 2,
    users: 4,
    bypassing_users: 4,
    labels: 2,
    items: 4,
    comments: 2,
    dependencies: 2,
    reviews: 2,
    branches: 4,
    commits: 24,
    files: 2,
    statuses: 4,
    contexts: 2,
    workflows: 0,
    workflow_runs: 0,
    pages: 2,
    name_bytes: 16,
    title_bytes: 16,
    body_bytes: 48,
    content_bytes: 64,
    page_size: 2,
    calls: 3,
    hooks: 8,
    observations: 8,
};

/// Calls answered at once; CI and webhooks so slow that they pile up.
const CONFIG: Config = Config {
    limits: LIMITS,
    latency_min: Duration::from_millis(1),
    latency_max: Duration::from_millis(1),
    settle_min: Duration::from_secs(2),
    settle_max: Duration::from_secs(2),
    late: 0,
    late_min: Duration::from_millis(1),
    late_max: Duration::from_millis(1),
    unavailable: 0,
    timeouts: 0,
    landing: 0,
    land_min: Duration::from_millis(1),
    land_max: Duration::from_millis(1),
    rate_limit: 1000,
    rate_window: Duration::from_secs(3600),
    ci: 9,
    setup_faults: 0,
    lost: 0,
    hook_min: Duration::from_secs(3600),
    hook_max: Duration::from_secs(3600),
    hooks_late: 0,
    hooks_lost: 0,
    resolution: Duration::from_secs(1),
    skew: Skew::None,
    status_updates: false,
    edit_updates: false,
};

/// The users of every repository: an admin, then writers. The second opens
/// everything; the others comment and review.
const ADMIN: u64 = 1;
const AUTHOR: u64 = 2;

/// A name as long as names may be: `tag`, then `index`.
fn name(tag: u8, index: u64) -> Box<[u8]> {
    let len = usize::try_from(LIMITS.name_bytes).expect("small");
    let suffix = index.to_be_bytes();
    let prefix = len.checked_sub(suffix.len()).expect("name holds index");
    let mut bytes = Writer::new(len);
    for _ in 0..prefix {
        bytes.put(&[tag]).expect("name prefix fits");
    }
    bytes.put(&suffix).expect("name index fits");
    bytes.finish()
}

/// Text `len` bytes long.
fn text(len: u32, tag: u8) -> Box<[u8]> {
    let mut bytes = Writer::new(usize::try_from(len).expect("small"));
    for _ in 0..len {
        bytes.put(&[tag]).expect("text fits");
    }
    bytes.finish()
}

fn names(tag: u8, count: u32) -> Box<[Box<[u8]>]> {
    let mut names = List::with_capacity(count);
    for index in 0..u64::from(count) {
        names.push(name(tag, index)).expect("name capacity");
    }
    names.into_boxed()
}

/// A full tree, its contents tagged with `tag`.
fn tree(tag: u8) -> Box<[File]> {
    let mut files = List::with_capacity(LIMITS.files);
    for index in 0..u64::from(LIMITS.files) {
        files.push(File { path: name(b'p', index), content: text(LIMITS.content_bytes, tag) }).expect("file capacity");
    }
    files.into_boxed()
}

fn setup(index: u64) -> Setup {
    Setup {
        workflows: Box::new([]),
        id: u32::try_from(index).expect("scenario id fits").checked_add(1).expect("scenario id fits"),
        name: name(b'r', index),
        default: name(b'b', 0),
        tree: tree(b'0'),
        labels: names(b'l', LIMITS.labels),
        checks: Checks {
            contexts: names(b'c', LIMITS.contexts),
            latency_min: Duration::from_secs(3600),
            latency_max: Duration::from_secs(3600),
            silent: 0,
            passes: 1000,
            reruns: 1000,
            cue: Some(Cue::File { path: name(b'p', 0), green: text(LIMITS.content_bytes, b'g') }),
        },
        protection: Some(Protection {
            linear: true,
            code_owners: true,
            bypassing_users: Box::from([101, 102, 103, 104]),
            queue: false,
            branch: name(b'z', 0),
            contexts: names(b'c', LIMITS.contexts),
            approvals: 1,
            dismiss_stale: false,
        }),
        hooked: true,
    }
}

/// The forge, measured: each step's peak is checked against the worst case,
/// less what it handed out in requests, which their receivers count.
struct Measured {
    domain: Domain,
    env: Env<Config>,
    out: Queue<Request>,
    meter: Meter,
    bound: u64,
    calls: u64,
}

#[test]
fn full_merge_history_and_both_parent_traversals_stay_bounded() {
    let mut forge = Measured::new();
    let repository = name(b'r', 0);
    let mut configured = setup(0);
    configured.protection = None;
    let first = skein_fake_forge_domain::repository(&mut forge.domain, &CONFIG, configured);
    skein_fake_forge_domain::grant(&mut forge.domain, &repository, AUTHOR, Permission::Write);
    let mut left = first;
    let mut right = first;
    for _ in 1..LIMITS.commits {
        let files = tree(b'm');
        forge.meter.start();
        let next = skein_fake_forge_domain::merge_commit(
            &mut forge.domain,
            &CONFIG,
            left,
            right,
            files,
            &text(
                LIMITS
                    .title_bytes
                    .checked_add(LIMITS.body_bytes)
                    .expect("title and body fit")
                    .checked_add(2)
                    .expect("message length fits"),
                b'm',
            ),
        )
        .expect("one commit per free store slot");
        let measured = forge.meter.end();
        forge.meter.check(measured, forge.bound, &"a merge commit");
        left = right;
        right = next;
    }
    forge.meter.start();
    assert!(forge.domain.is_ancestor(first, right));
    assert!(forge.domain.is_ancestor(left, right));
    let measured = forge.meter.end();
    forge.meter.check(measured, forge.bound, &"both-parent ancestry");
    forge.ok(AUTHOR, &repository, Op::Git(Git::Push { branch: name(b'w', 0), commit: right, expected: None }));
    assert_eq!(forge.domain.has(&repository).len(), LIMITS.commits);
    // Reading mergeability walks both histories as a real merge-base read does.
    forge.ok(AUTHOR, &repository, Op::Git(Git::Create { branch: name(b'w', 1), commit: left }));
    let number = forge
        .ok(
            AUTHOR,
            &repository,
            Op::Write(Write::OpenPull {
                opens_empty: false,
                title: text(LIMITS.title_bytes, b't'),
                body: text(LIMITS.body_bytes, b'b'),
                head: name(b'w', 0),
                base: name(b'w', 1),
            }),
        )
        .expect("a pull was opened");
    forge.ok(AUTHOR, &repository, Op::Read(Read::Pull { number }));
    forge.ok(AUTHOR, &repository, Op::Read(Read::PullFiles { number, page: 1, limit: 0 }));
    forge.ok(AUTHOR, &repository, Op::Read(Read::Checks { commit: right }));
    forge.ok(AUTHOR, &repository, Op::Read(Read::Settings));
    forge.ok(AUTHOR, &repository, Op::Read(Read::Collaborators));
    skein_fake_forge_domain::grant(&mut forge.domain, &repository, ADMIN, Permission::Admin);
    forge.ok(ADMIN, &repository, Op::Read(Read::Protection { branch: name(b'w', 0) }));
    // Each held comparison owns the full changed trees and every new commit
    // in the converging history, rather than relying on a single-parent walk.
    for _ in 0..LIMITS.calls {
        forge.send(AUTHOR, &repository, Op::Read(Read::Compare { base: first, head: right, page: 1, limit: 1 }));
    }
    assert_eq!(forge.domain.calls(), LIMITS.calls);
}

#[test]
fn many_check_contexts_and_small_trees_keep_the_temporary_status_array_bounded() {
    let limits = Limits { repositories: 1, files: 1, contexts: 64, statuses: 1, commits: 2, ..LIMITS };
    let config = Config { limits, ..CONFIG };
    let mut forge = Measured::with(config);
    let repository = name(b'r', 0);
    let mut configured = setup(0);
    configured.tree = Box::new([File { path: name(b'p', 0), content: text(limits.content_bytes, b'g') }]);
    configured.checks.contexts = names(b'c', limits.contexts);
    configured.protection = None;
    let first = skein_fake_forge_domain::repository(&mut forge.domain, &config, configured);
    skein_fake_forge_domain::grant(&mut forge.domain, &repository, AUTHOR, Permission::Write);
    forge.ok(AUTHOR, &repository, Op::Git(Git::Push { branch: name(b'w', 0), commit: first, expected: None }));
    for _ in 0..limits.calls {
        forge.send(AUTHOR, &repository, Op::Read(Read::Checks { commit: first }));
    }
    assert_eq!(forge.domain.calls(), limits.calls);
}

impl Measured {
    fn new() -> Measured {
        Self::with(CONFIG)
    }

    fn with(config: Config) -> Measured {
        let bound = worst_case(&config.limits).expect("the test limits fit");
        let meter = Meter::new();
        let domain = Domain::new(&config, 7);
        let out = Queue::with_capacity(MAX_OUT);
        Measured {
            domain,
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: config },
            out,
            meter,
            bound,
            calls: 0,
        }
    }

    /// Steps a call by `user` on `repository`, measured, without answering
    /// it.
    fn send(&mut self, user: u64, repository: &[u8], op: Op) -> Token {
        self.calls = self.calls.checked_add(1).expect("call count fits");
        let token = Token::new(self.calls);
        let event = Event::Call { reply_to: ReplyTo::new(token), user, repository: repository.into(), op };
        self.meter.start();
        step(&mut self.domain, &self.env, event, &mut self.out);
        let measured = self.meter.end();
        assert!(self.out.is_empty(), "a call is held");
        self.meter.check(measured, self.bound, &"a call");
        token
    }

    /// Fires the next timer, measured: what it emits is dropped, but for a
    /// reply's outcome.
    fn fire(&mut self) -> Option<(Token, Result<Option<u64>, Error>)> {
        self.env.now = self.domain.next_deadline().expect("a timer is armed");
        self.meter.start();
        fire(&mut self.domain, &self.env, &mut self.out);
        let measured = self.meter.end();
        let outcome = match self.out.pop() {
            Some(Request::Reply { to, result }) => Some((to.into_token(), outcome(result))),
            Some(Request::Hook { .. }) | None => None,
        };
        self.meter.check(measured, self.bound, &"a timer");
        self.domain.reclaim();
        outcome
    }

    /// Calls and waits for the answer.
    fn call(&mut self, user: u64, repository: &[u8], op: Op) -> Result<Option<u64>, Error> {
        let token = self.send(user, repository, op);
        for _ in 0..64_u32 {
            if let Some((to, outcome)) = self.fire()
                && to == token
            {
                return outcome;
            }
        }
        unreachable!("the call is answered");
    }

    fn ok(&mut self, user: u64, repository: &[u8], op: Op) -> Option<u64> {
        self.call(user, repository, op).expect("the call succeeds")
    }
}

/// What a test needs of an answer: the number it gives, if it gives one.
fn outcome(result: Result<Answer, Error>) -> Result<Option<u64>, Error> {
    match result? {
        Answer::Created(number)
        | Answer::Commented(number)
        | Answer::Reviewed(number)
        | Answer::Merged(number)
        | Answer::Revision(number) => Ok(Some(number)),
        Answer::Located { .. }
        | Answer::Items { .. }
        | Answer::Item { .. }
        | Answer::Pull(_)
        | Answer::Statuses(_)
        | Answer::Permission(_)
        | Answer::Comment { .. }
        | Answer::Dependencies(_)
        | Answer::Labels(_)
        | Answer::Commit(_)
        | Answer::GitTree { .. }
        | Answer::Tree(_)
        | Answer::Artifact { .. }
        | Answer::Commits { .. }
        | Answer::File(_)
        | Answer::Pages { .. }
        | Answer::Page(_)
        | Answer::Done
        | Answer::Cloned { .. }
        | Answer::Pushed(_)
        | Answer::PullFiles { .. }
        | Answer::Comparison { .. }
        | Answer::Checks(_)
        | Answer::Runs(_)
        | Answer::Run(_)
        | Answer::Jobs(_)
        | Answer::Job { .. }
        | Answer::Comments(_)
        | Answer::Pulls(_)
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators(_)
        | Answer::Branch(_) => Ok(None),
    }
}

/// Fills the repository `index` with all it may hold.
fn fill(forge: &mut Measured, index: u64, first: u64) {
    let repository = name(b'r', index);
    let config = forge.env.limits;
    // Three branches, each a commit of a full tree ahead of the default.
    for branch in 0..3_u64 {
        let content = b'a'
            .checked_add(
                u8::try_from(branch.checked_add(3_u64.checked_mul(index).expect("small")).expect("small"))
                    .expect("small"),
            )
            .expect("small");
        let commit = skein_fake_forge_domain::commit(
            &mut forge.domain,
            &config,
            first,
            tree(content),
            &text(
                LIMITS
                    .title_bytes
                    .checked_add(LIMITS.body_bytes)
                    .expect("title and body fit")
                    .checked_add(2)
                    .expect("message length fits"),
                b'm',
            ),
        )
        .expect("room")
        .expect("a change");
        let push = Op::Git(Git::Push { branch: name(b'w', branch), commit, expected: None });
        forge.ok(AUTHOR, &repository, push);
    }
    // Two pull requests, and two issues carrying every label.
    for branch in 0..2_u64 {
        let open = Write::OpenPull {
            opens_empty: false,
            title: text(LIMITS.title_bytes, b't'),
            body: text(LIMITS.body_bytes, b'b'),
            head: name(b'w', branch),
            base: name(b'b', 0),
        };
        forge.ok(AUTHOR, &repository, Op::Write(open)).expect("opened pull");
        // Group branches need room for their generated name. This fixture's
        // names are sixteen bytes; queue occupancy is measured separately.
    }
    for _ in 0..2_u32 {
        let create = Write::CreateIssue {
            title: text(LIMITS.title_bytes, b't'),
            body: text(LIMITS.body_bytes, b'b'),
            labels: names(b'l', LIMITS.labels),
        };
        forge.ok(AUTHOR, &repository, Op::Write(create));
    }
    for number in 1..=u64::from(LIMITS.items) {
        for user in 3..3_u64.checked_add(u64::from(LIMITS.comments)).expect("users fit") {
            let comment = Write::Comment { number, body: text(LIMITS.body_bytes, b'c') };
            forge.ok(user, &repository, Op::Write(comment));
        }
        let mut dependencies = List::with_capacity(2);
        for dependency in number.checked_add(1).expect("item count fits")..=u64::from(LIMITS.items) {
            if dependencies.len() == 2 {
                break;
            }
            dependencies.push(dependency).expect("two dependencies");
        }
        let dependencies = dependencies.into_boxed();
        forge.ok(AUTHOR, &repository, Op::Write(Write::SetDependencies { number, dependencies }));
    }
    for number in 1..=2 {
        for user in 3..3_u64.checked_add(u64::from(LIMITS.reviews)).expect("users fit") {
            let review = Write::Review { number, verdict: Some(Verdict::Approve), body: text(LIMITS.body_bytes, b'v') };
            forge.ok(user, &repository, Op::Write(review));
        }
        let reviewers = Box::new([ADMIN, 3, 4]);
        forge.ok(ADMIN, &repository, Op::Write(Write::SetReviewers { number, reviewers }));
    }
    for page in 0..u64::from(LIMITS.pages) {
        let put = Write::PutPage { name: name(b'n', page), content: text(LIMITS.content_bytes, b'w') };
        forge.ok(AUTHOR, &repository, Op::Write(put));
    }
    // The default branch's commit has statuses too, of every context.
    for context in 0..u64::from(LIMITS.contexts) {
        let status = Write::Status {
            commit: first,
            context: name(b'c', context),
            state: Check::Passed,
            description: Box::new([]),
            link: Box::new([]),
        };
        forge.ok(AUTHOR, &repository, Op::Write(status));
    }
}

#[test]
fn a_forge_filled_to_its_limits_stays_within_its_worst_case() {
    let mut forge = Measured::new();
    let config = forge.env.limits;
    let mut firsts = List::with_capacity(LIMITS.repositories);
    for index in 0..u64::from(LIMITS.repositories) {
        let first = skein_fake_forge_domain::repository(&mut forge.domain, &config, setup(index));
        firsts.push(first).expect("repository capacity");
        for user in 1..=u64::from(LIMITS.users) {
            let permission = if user == ADMIN { Permission::Admin } else { Permission::Write };
            skein_fake_forge_domain::grant(&mut forge.domain, &name(b'r', index), user, permission);
        }
    }
    for (index, &first) in firsts.iter().enumerate() {
        fill(&mut forge, u64::try_from(index).expect("small"), first);
    }
    let room = forge.domain.room();
    assert_eq!(room.statuses, 0, "every repository holds as many statuses as it may");
    // A full history makes whole-message answers exercise every remaining commit.
    let first = *firsts.as_slice().first().expect("first repository");
    let mut last = first;
    for content in 0..u8::MAX {
        let made = skein_fake_forge_domain::commit(
            &mut forge.domain,
            &config,
            last,
            tree(content),
            &text(
                LIMITS
                    .title_bytes
                    .checked_add(LIMITS.body_bytes)
                    .expect("title and body fit")
                    .checked_add(2)
                    .expect("message length fits"),
                b'm',
            ),
        );
        match made {
            Ok(Some(commit)) => last = commit,
            Ok(None) => {}
            Err(Error::Full) => break,
            Err(error) => panic!("history fill: {error:?}"),
        }
    }
    let repository = name(b'r', 0);
    forge.ok(AUTHOR, &repository, Op::Git(Git::Push { branch: name(b'b', 0), commit: last, expected: Some(first) }));
    assert_eq!(forge.domain.room().commits, 0);
    let tally = forge.domain.tally();
    assert_eq!((tally.full, tally.unreported), (0, 0), "nothing was refused for room while filling");
    assert!(forge.domain.observations_lost() > 0, "the observations kept are as many as may be");
    assert!(tally.hooks_dropped > 0, "the webhooks in flight are as many as may be");
    // Calls held, each with as large an answer as there is.
    let repository = name(b'r', 0);
    forge.ok(
        ADMIN,
        &repository,
        Op::Read(Read::Checks { commit: *firsts.as_slice().first().expect("first repository") }),
    );
    forge.ok(ADMIN, &repository, Op::Read(Read::Protection { branch: name(b'z', 0) }));
    forge.ok(ADMIN, &repository, Op::Read(Read::Settings));
    forge.ok(ADMIN, &repository, Op::Read(Read::Collaborators));
    let big = [
        Op::Read(Read::Tree { commit: *firsts.as_slice().first().expect("first repository") }),
        Op::Read(Read::Items {
            order: Order::LeastUpdated,
            state: None,
            kind: None,
            labels: Box::new([]),
            author: None,
            since: Time::ZERO,
            page: 2,
            limit: 0,
        }),
        Op::Read(Read::Pull { number: 1 }),
    ];
    for op in big {
        forge.send(ADMIN, &repository, op);
    }
    assert_eq!(forge.domain.calls(), LIMITS.calls);
    for _ in 0..LIMITS.calls {
        let (_, result) = forge.fire().expect("one retained answer goes out");
        result.expect("the retained read succeeds");
    }
    for _ in 0..LIMITS.calls {
        forge.send(ADMIN, &repository, Op::Read(Read::Commits { before: first, after: last }));
    }
    assert_eq!(forge.domain.calls(), LIMITS.calls);
    let held = forge.meter.held();
    assert!(
        held.checked_mul(3).expect("measured heap fits") >= forge.bound.checked_mul(2).expect("bound fits"),
        "the fill reaches two thirds of the worst case: {held} of {}",
        forge.bound
    );
}

#[test]
fn counted_group_branches_and_queue_landings_stay_within_memory_bound() {
    // A generated group name needs at most 18 + 4 + 4 + 20 + 1 + 40
    // bytes here. Compared with the full fixture, names gain eighty bytes;
    // one repository and three branches hold main, the pull head and its group.
    let limits = Limits { repositories: 1, name_bytes: 96, branches: 3, ..LIMITS };
    let config = Config { limits, ..CONFIG };
    let mut forge = Measured::with(config);
    let repository = name(b'r', 0);
    let mut configured = setup(0);
    configured.default = Box::from(*b"main");
    configured.checks.latency_min = Duration::from_secs(2);
    configured.checks.latency_max = Duration::from_secs(2);
    configured.checks.reruns = 0;
    configured.protection = Some(Protection {
        linear: false,
        code_owners: false,
        bypassing_users: Box::from([]),
        queue: true,
        branch: Box::from(*b"main"),
        contexts: names(b'c', LIMITS.contexts),
        approvals: 0,
        dismiss_stale: false,
    });
    let first = skein_fake_forge_domain::repository(&mut forge.domain, &config, configured);
    skein_fake_forge_domain::grant(&mut forge.domain, &repository, AUTHOR, Permission::Write);
    let head = skein_fake_forge_domain::commit(&mut forge.domain, &config, first, tree(b'g'), b"").unwrap().unwrap();
    forge.ok(AUTHOR, &repository, Op::Git(Git::Push { branch: Box::from(*b"alice"), commit: head, expected: None }));
    let number = forge
        .ok(
            AUTHOR,
            &repository,
            Op::Write(Write::OpenPull {
                title: Box::from(*b"group"),
                body: Box::new([]),
                head: Box::from(*b"alice"),
                base: Box::from(*b"main"),
                opens_empty: false,
            }),
        )
        .unwrap();
    forge.ok(AUTHOR, &repository, Op::Write(Write::Enqueue { number, head }));
    assert_eq!(forge.domain.branches(&repository).len(), limits.branches);
    for _ in 0..64_u32 {
        if forge.domain.next_deadline().is_none() {
            break;
        }
        forge.fire();
    }
    let Answer::Pull(pull) = forge.domain.inspect(&config, &repository, &Read::Pull { number }).unwrap() else {
        panic!("pull")
    };
    assert!(pull.queue_merged);
    assert_eq!(forge.domain.branches(&repository).len(), 2);
}
