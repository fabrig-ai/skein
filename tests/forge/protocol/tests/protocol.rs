use skein_forge::api::{Answer, Read, State};
use skein_forge_client::{Event, Resource, Watch, What};
use skein_forge_machine_world::joint::REPO;
use skein_forge_protocol_world::{
    World,
    fixtures::{self, LIMITS},
};
use skein_lib::{Duration, Time, Token};
use std::collections::BTreeSet;

#[test]
fn a_creation_lost_on_the_wire_is_found_after_restart() {
    let mut world = fixtures::lost_creation(23);
    assert_eq!(world.creations(), 1);
    assert!(!world.made(1));
    world.restart();
    world.finish();
    assert_eq!(world.creations(), 1);
    assert!(world.made(1));
    assert_eq!(world.owner_counts(), (0, 0));
}

#[test]
fn listings_follow_another_partys_changes() {
    let mut world = World::new(5, 0);
    world.run_for(Duration::from_secs(1));
    let a = world.person_issue(b"A");
    let b = world.person_issue(b"B");
    let c = world.person_issue(b"C");
    world.person_edit(b, b"B2");
    world.person_close(c);

    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    let mut page = 1;
    loop {
        let Answer::Items { items, more, .. } =
            world.read(Read::Items { since: Time::ZERO, page, kind: None }).expect("owner listing")
        else {
            panic!("listing returns items")
        };
        for item in items {
            assert!(seen.insert(item.number), "no number twice across pages");
            rows.push((item.number, item.title, item.state == State::Open));
        }
        if !more {
            break;
        }
        page += 1;
    }
    assert!(page > 1, "listing crosses the fake's page size of two");
    rows.sort_by_key(|row| row.0);
    assert_eq!(rows, world.snapshot());
    assert_eq!(
        rows,
        vec![
            (a, Box::from(b"A".as_slice()), true),
            (b, Box::from(b"B2".as_slice()), true),
            (c, Box::from(b"C".as_slice()), false)
        ]
    );
}

#[test]
fn signed_deliveries_reach_the_clients_owner() {
    let mut world = fixtures::hooked(7);
    world.run_for(Duration::from_secs(1));
    let number = world.person_issue(b"before");
    let resource = Resource { repository: REPO, what: What::Issue(number) };
    world.event(Event::Keep {
        owner: Token::new(1),
        watches: Box::new([Watch { resource: resource.clone(), participating: false }]),
    });
    world.run_for(Duration::from_secs(2));
    let Some(Answer::Item { item, .. }) = world.changed(&resource) else { panic!("kept issue is read") };
    assert_eq!(&*item.title, b"before");

    world.person_edit(number, b"after");
    let after = Duration::from_secs(2);
    assert!(after < LIMITS.poll, "the hint brings the change before polling");
    world.run_for(after);
    let Some(Answer::Item { item, .. }) = world.changed(&resource) else { panic!("hinted issue is read") };
    assert_eq!(&*item.title, b"after");
    assert_eq!(world.hints(), 2);
}

use skein_fake_forge_domain::api as raw;
use skein_forge::api::{self, Conclusion, EntryKind, JobAttempt, MergeHow, RunState, UpdateStyle, Write};
use skein_forge_client::{Made, Outcome};
use skein_forge_client_world::translate::commit;

fn workflow() -> raw::Workflow {
    raw::Workflow {
        file: Box::from(b"gate.yml".as_slice()),
        jobs: Box::new([Box::from(b"build".as_slice()), Box::from(b"test".as_slice())]),
        latency_min: Duration::from_secs(2),
        latency_max: Duration::from_secs(2),
        silent: 0,
        passes: 500,
        cue: None,
    }
}
fn file(path: &[u8], content: &[u8]) -> raw::File {
    raw::File { path: Box::from(path), content: Box::from(content) }
}
fn branch(world: &mut World, name: &[u8]) -> api::Commit {
    let Answer::Commit(head) = world.read(Read::Branch { branch: Box::from(name) }).expect("branch read") else {
        panic!("branch head")
    };
    head
}
fn completed(world: &mut World, id: u64) -> api::Run {
    for _ in 0..10 {
        let Answer::Run(run) = world.read(Read::Run { run: id }).expect("run read") else { panic!("run answer") };
        if run.state == RunState::Completed {
            return run;
        }
        world.run_for(Duration::from_secs(1));
    }
    panic!("run completes within ten seconds")
}
#[test]
fn a_dispatched_run_is_found_followed_and_rerun() {
    let mut world = World::turn(23, Box::new([workflow()]), Box::new([]));
    let head = branch(&mut world, b"main");
    assert_eq!(
        world.write(Write::Dispatch {
            workflow: Box::from(b"gate.yml".as_slice()),
            branch: Box::from(b"main".as_slice()),
            inputs: Box::new([api::Input {
                name: Box::from(b"select".as_slice()),
                value: Box::from(b"all".as_slice())
            }]),
        }),
        Outcome::Made { made: Made::Set, found: false }
    );
    let Answer::Runs { runs, more } = world
        .read(Read::Runs { workflow: Box::from(b"gate.yml".as_slice()), commit: head, page: 1 })
        .expect("find dispatch")
    else {
        panic!("runs answer")
    };
    assert_eq!(runs.len(), 1);
    assert!(!more);
    assert_eq!((runs[0].commit, runs[0].event, runs[0].actor), (head, api::Event::Dispatch, 1));
    let raw::Answer::Run(kept) = world.inspect(&raw::Read::Run { run: runs[0].id }).expect("kept run") else {
        panic!("raw run")
    };
    assert_eq!(&*kept.inputs, &[(Box::from(b"select".as_slice()), Box::from(b"all".as_slice()))]);
    let run = completed(&mut world, runs[0].id);
    assert_eq!(run.jobs.len(), 2);
    assert_eq!(run.jobs.iter().filter(|job| job.conclusion == Some(Conclusion::Success)).count(), 1);
    let failed = run.jobs.iter().find(|job| job.conclusion == Some(Conclusion::Failure)).expect("one failed job");
    assert_eq!(failed.steps, 3);
    assert_eq!(failed.step.as_ref().expect("failed workflow step").number, raw::WORKFLOW_STEP);
    let attempt = JobAttempt { head, run: run.id, job: failed.id, attempt: failed.attempt };
    assert_eq!(world.write(Write::Rerun { attempt }), Outcome::Made { made: Made::Set, found: false });
    let rerun = completed(&mut world, run.id);
    let job = rerun.jobs.iter().find(|job| job.id == failed.id).expect("same job");
    assert_eq!((job.attempt, job.conclusion, job.steps), (attempt.attempt + 1, Some(Conclusion::Success), 0));
    world.finish();
}
#[test]
fn a_tree_is_read_by_commit_then_by_directory() {
    let mut world = World::turn(
        31,
        Box::new([]),
        Box::new([file(b"docs/a.md", b"a"), file(b"src/lib.rs", b"lib"), file(b"src.rs", b"module")]),
    );
    let head = branch(&mut world, b"main");
    let Answer::Tree { entries, more } = world.read(Read::Tree { object: head, page: 1 }).expect("root tree") else {
        panic!("tree answer")
    };
    assert!(more, "root crosses a two-entry page");
    assert_eq!((&*entries[0].name, entries[0].kind), (b"docs".as_slice(), EntryKind::Tree));
    assert_eq!((&*entries[1].name, entries[1].kind), (b"src.rs".as_slice(), EntryKind::Blob));
    let docs = entries[0].object;
    let Answer::Tree { entries, more } = world.read(Read::Tree { object: head, page: 2 }).expect("second root page")
    else {
        panic!("tree answer")
    };
    assert!(more, "Forgejo keeps truncated set on the short last page");
    assert_eq!((&*entries[0].name, entries[0].kind), (b"src".as_slice(), EntryKind::Tree));
    assert_eq!(
        world.read(Read::Tree { object: head, page: 3 }).expect("past last root page"),
        Answer::Tree { entries: Box::new([]), more: false }
    );
    let Answer::Tree { entries, more } = world.read(Read::Tree { object: docs, page: 1 }).expect("directory id") else {
        panic!("tree answer")
    };
    assert!(!more);
    assert_eq!((&*entries[0].name, entries[0].kind), (b"a.md".as_slice(), EntryKind::Blob));
    let mut id = [0; 20];
    id.copy_from_slice(&docs[..20]);
    let raw::Answer::GitTree { entries: expected, .. } = world
        .inspect(&raw::Read::GitTree { object: raw::TreeOf::Tree(id), page: 1, limit: 2 })
        .expect("domain directory")
    else {
        panic!("domain tree")
    };
    assert_eq!(&entries[0].object[..20], expected[0].object);
    world.finish();
}
#[test]
fn a_pull_is_rebased_then_merged_fast_forward() {
    let mut world = World::turn(37, Box::new([]), Box::new([file(b"base", b"root")]));
    world
        .direct(raw::Op::Write(raw::Write::CreateBranch { branch: Box::from(b"work".as_slice()), commit: 1 }))
        .expect("work branch");
    let work = world.prepare_commit(1, Box::new([file(b"base", b"root"), file(b"work", b"change")]), b"work");
    world
        .direct(raw::Op::Git(raw::Git::Push { branch: Box::from(b"work".as_slice()), commit: work, expected: None }))
        .expect("work push");
    let raw::Answer::Created(number) = world
        .direct(raw::Op::Write(raw::Write::OpenPull {
            head: Box::from(b"work".as_slice()),
            base: Box::from(b"main".as_slice()),
            title: Box::from(b"work".as_slice()),
            body: Box::new([]),
            opens_empty: false,
        }))
        .expect("pull")
    else {
        panic!("pull number")
    };
    let main = world.prepare_commit(1, Box::new([file(b"base", b"root"), file(b"main", b"advance")]), b"main");
    world
        .direct(raw::Op::Git(raw::Git::Push { branch: Box::from(b"main".as_slice()), commit: main, expected: None }))
        .expect("main push");
    assert!(matches!(
        world.write(Write::Update { number, style: UpdateStyle::Rebase }),
        Outcome::Made { made: Made::Updated(_), .. }
    ));
    let head = branch(&mut world, b"work");
    let Answer::Compare { contains_before, .. } =
        world.read(Read::Compare { before: commit(main), after: head }).expect("rebased ancestry")
    else {
        panic!("compare")
    };
    assert!(contains_before);
    world.run_for(Duration::from_secs(3));
    assert_eq!(
        world.write(Write::Merge { number, head, how: MergeHow::FastForwardOnly }),
        Outcome::Made { made: Made::Merged(head), found: false }
    );
    assert_eq!(branch(&mut world, b"main"), head);
    world
        .direct(raw::Op::Write(raw::Write::CreateBranch { branch: Box::from(b"behind".as_slice()), commit: main }))
        .expect("behind branch");
    let raw::Answer::Created(behind) = world
        .direct(raw::Op::Write(raw::Write::OpenPull {
            head: Box::from(b"behind".as_slice()),
            base: Box::from(b"main".as_slice()),
            title: Box::from(b"behind".as_slice()),
            body: Box::new([]),
            opens_empty: true,
        }))
        .expect("behind pull")
    else {
        panic!("pull number")
    };
    world.run_for(Duration::from_secs(3));
    assert_eq!(
        world.write(Write::Merge { number: behind, head: commit(main), how: MergeHow::FastForwardOnly }),
        Outcome::Uncertain
    );
    assert_eq!(branch(&mut world, b"main"), head, "the 500 refusal leaves the base in place");
    world.finish();
}
#[test]
fn a_status_keeps_its_words_and_link() {
    let mut world = World::turn(41, Box::new([]), Box::new([]));
    let head = branch(&mut world, b"main");
    assert_eq!(
        world.write(Write::Status {
            commit: head,
            context: Box::from(b"gate".as_slice()),
            check: api::Check::Passed,
            description: Box::from(b"proved candidate".as_slice()),
            link: Box::from(b"https://ci.example/gates/42".as_slice()),
        }),
        Outcome::Made { made: Made::Set, found: false }
    );
    let Answer::Statuses { statuses, .. } = world.read(Read::Statuses { commit: head, page: 1 }).expect("status")
    else {
        panic!("statuses")
    };
    assert_eq!(
        (&*statuses[0].description, &*statuses[0].url),
        (b"proved candidate".as_slice(), b"https://ci.example/gates/42".as_slice())
    );
    world.finish();
}

#[test]
fn runs_ignore_other_workflows_and_page_by_the_raw_forgejo_listing() {
    let gate = workflow();
    let mut other = gate.clone();
    other.file = Box::from(b"other.yml".as_slice());
    let mut world = World::turn(43, Box::new([gate, other]), Box::new([]));
    let head = branch(&mut world, b"main");
    for file in [b"other.yml".as_slice(), b"gate.yml"] {
        world
            .direct(raw::Op::Write(raw::Write::Dispatch {
                workflow: Box::from(file),
                branch: Box::from(b"main".as_slice()),
                inputs: Box::new([]),
            }))
            .expect("dispatch");
    }
    let Answer::Runs { runs, more } = world
        .read(Read::Runs { workflow: Box::from(b"gate.yml".as_slice()), commit: head, page: 1 })
        .expect("locally filtered runs")
    else {
        panic!("runs")
    };
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].workflow.as_ref(), b"gate.yml");
    assert!(more, "two raw rows fill Forgejo's page");
    assert_eq!(
        world
            .read(Read::Runs { workflow: Box::from(b"gate.yml".as_slice()), commit: head, page: 2 })
            .expect("past raw page"),
        Answer::Runs { runs: Box::new([]), more: false }
    );
    world.finish();
}
