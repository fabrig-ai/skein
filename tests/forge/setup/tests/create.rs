use core::num::NonZeroU64;
use skein_fake_forge_domain::{self as domain, api};
use skein_fake_forge_protocol::Route;
use skein_forge::forgejo::request::{Grant, Request};
use skein_forge_setup::{
    host::File,
    reconcile::{Failure, Outcome, Stage, Why},
};
use skein_forge_setup_world::{Ended, World, fixtures};
use skein_lib::{Env, Time, Wall};

// The empty domain allocates repository one, README one and the first file commit two.
const REPOSITORY: u32 = 1;
const README: u64 = 1;
const FILE_COMMIT: u64 = 2;
const PATH: &[u8] = b".forgejo/workflows/ci.yaml";
const CONTENT: &[u8] = b"name: ci\n";
const FULL: &[u8] = b"first/starter";
fn files() -> Box<[File]> {
    Box::new([File { path: PATH.into(), content: CONTENT.into() }])
}
fn world() -> World {
    let mut world = World::owned(7);
    fixtures::bot_tokens(&mut world);
    world.creation_domain();
    world.listening("127.0.0.1:7200".parse().expect("listener"));
    world
}
fn run(world: &mut World) -> Ended {
    world.create(b"first", b"starter", files())
}
fn created() -> Outcome {
    Outcome::Created { id: NonZeroU64::new(1).expect("repository") }
}
fn stopped(step: Stage, subject: &[u8], why: Why) -> Ended {
    Ended::Stop(Failure { forge: 1, step, subject: subject.into(), why })
}

#[test]
fn creation_makes_the_org_repository_access_hook_and_bot_authored_file() {
    let mut world = world();
    let result = run(&mut world);
    assert_eq!(result, Ended::Done);
    assert_eq!(world.outcome(), Some(&created()));
    assert_eq!(world.store().organization(b"first").expect("created org").owner.as_ref(), b"ada");
    assert!(
        world
            .requests()
            .contains(&Request::CreateOrg { person: b"ada".as_slice().into(), name: b"first".as_slice().into() })
    );
    assert_eq!(world.store().collaborator(REPOSITORY, b"kit"), Some(Grant::Write));
    assert_eq!(world.repository_hooks(REPOSITORY).len(), 1);
    let head = world.domain().branch(FULL, b"main").expect("main");
    let object = world.domain().object(head).expect("file commit");
    assert_eq!(object.tree.get(b"README.md".as_slice()).expect("readme").as_ref(), b"# starter\n");
    assert_eq!(object.tree.get(PATH).expect("workflow").as_ref(), CONTENT);
    assert_eq!(object.parent, Some(README));
    assert_eq!(object.message.as_ref(), b"Add .forgejo/workflows/ci.yaml");
    let author = object.author.as_ref().expect("file author");
    assert_eq!(author.name.as_ref(), b"kit");
    assert_eq!(author.email.as_ref(), b"kit@example.invalid");
    assert_eq!(object.author, object.committer);
    let bot = world.store().find(b"kit").expect("bot").id;
    let (domain, config) = world.domain_mut();
    let mut out = skein_lib::Queue::uncapped();
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: *config };
    domain::step(
        domain,
        &env,
        domain::Event::Call {
            reply_to: skein_lib::ReplyTo::new(skein_lib::Token::new(1)),
            user: bot,
            repository: FULL.into(),
            op: api::Op::Git(api::Git::Clone),
        },
        &mut out,
    );
    domain::fire(domain, &env, &mut out);
    assert!(
        matches!(out.pop(), Some(domain::Request::Reply { result: Ok(api::Answer::Cloned { default, branches }), .. }) if default.as_ref() == b"main" && branches.iter().any(|head| head.commit == FILE_COMMIT))
    );
}
#[test]
fn a_second_creation_only_patches_the_existing_hook() {
    let mut world = world();
    run(&mut world);
    let since = world.requests().len();
    let result = run(&mut world);
    assert_eq!(result, Ended::Done);
    assert_eq!(world.outcome(), Some(&created()));
    assert_eq!(fixtures::write_routes(&world, since), [Route::EditRepoHook]);
    assert_eq!(world.domain().branch(FULL, b"main"), Some(FILE_COMMIT));
}
#[test]
fn creation_resumes_after_a_file_refusal_without_duplicate_commits() {
    let mut world = world();
    world.refuse(Route::CreateFile);
    let interrupted = run(&mut world);
    world.permit();
    let result = run(&mut world);
    run(&mut world);
    assert_eq!(interrupted, stopped(Stage::File, PATH, Why::Refused(403)));
    assert_eq!(result, Ended::Done);
    assert_eq!(world.domain().branch(FULL, b"main"), Some(FILE_COMMIT));
}
#[test]
fn an_existing_org_and_person_edited_file_are_left_alone() {
    let mut world = world();
    world.store_mut().org(b"first", b"someone-else");
    // The store requires the organization's owner account when creating its repository.
    world.store_mut().account(b"someone-else", false, b"");
    run(&mut world);
    let (domain, config) = world.domain_mut();
    let env = Env { now: Time::ZERO, wall: Wall::EPOCH, limits: *config };
    let edited = domain::advance(domain, &env, FULL, b"main", PATH, b"the person's bytes\n", 1).expect("person edit");
    let result = run(&mut world);
    assert_eq!(result, Ended::Done);
    assert_eq!(world.store().organization(b"first").expect("org").owner.as_ref(), b"someone-else");
    assert_eq!(world.domain().branch(FULL, b"main"), Some(edited));
    assert_eq!(
        world.domain().object(edited).expect("edited commit").tree.get(PATH).expect("file").as_ref(),
        b"the person's bytes\n"
    );
}
#[test]
fn a_missing_org_without_an_owner_is_made_owned_by_the_verified_admin() {
    let mut world = world();
    world.org_owner(None);
    world.store_mut().revoke(fixtures::ADMIN);
    let admin = world.store_mut().account(b"token-admin", true, b"");
    world.store_mut().token(
        admin,
        b"creation-admin",
        &[b"read:user", b"write:organization", b"write:repository", b"write:admin"],
        fixtures::secret(fixtures::ADMIN),
    );
    assert_eq!(run(&mut world), Ended::Done);
    assert_eq!(world.outcome(), Some(&created()));
    assert_eq!(world.store().organization(b"first").expect("created org").owner.as_ref(), b"token-admin");
    assert!(
        world.requests().contains(&Request::CreateOrg {
            person: b"token-admin".as_slice().into(),
            name: b"first".as_slice().into(),
        })
    );
    assert!(world.domain().object(FILE_COMMIT).expect("file commit").tree.contains_key(PATH));
}
#[test]
fn a_present_org_without_an_owner_is_left_owned_by_its_person() {
    let mut world = world();
    world.org_owner(None);
    world.store_mut().account(b"someone-else", false, b"");
    world.store_mut().org(b"first", b"someone-else");
    assert_eq!(run(&mut world), Ended::Done);
    assert_eq!(world.outcome(), Some(&created()));
    assert_eq!(world.store().organization(b"first").expect("org").owner.as_ref(), b"someone-else");
    assert!(!world.requests().iter().any(|request| matches!(request, Request::CreateOrg { .. })));
}
#[test]
fn a_token_without_repository_scope_stops_at_repository() {
    let mut world = world();
    world.admin_scopes(&[b"read:user", b"write:organization", b"write:admin"]);
    assert_eq!(run(&mut world), stopped(Stage::Repository, FULL, Why::Refused(403)));
}
#[test]
fn a_token_without_organization_write_stops_at_repository_creation() {
    let mut world = world();
    world.admin_scopes(&[b"read:user", b"read:organization", b"write:repository", b"write:admin"]);
    assert_eq!(run(&mut world), stopped(Stage::Repository, FULL, Why::Refused(403)));
}
