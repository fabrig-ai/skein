use super::fixtures::{self, Harness, boxed};
use skein_forge::forgejo::{
    document::Document,
    request::{Author, Repo, Request},
};
use skein_lib::Secret;
const NEW: &[u8] = b"creation/starter";
const REPO_ROUTE: &[u8] = b"/api/v1/orgs/creation/repos";
const REPO_BODY: &[u8] = br#"{"name":"starter","private":true,"auto_init":true,"default_branch":"main"}"#;
fn org() -> Harness {
    let mut h = Harness::creating();
    h.service.setup_mut().org(b"creation", b"kit-admin");
    h
}
fn repository() -> Harness {
    let mut h = org();
    h.ask(b"POST", REPO_ROUTE, REPO_BODY);
    h
}
fn file() -> Request {
    Request::CreateFile {
        at: Repo { owner: boxed(b"creation"), name: boxed(b"starter") },
        path: boxed(b"dir/file"),
        branch: boxed(b"main"),
        content: boxed(b"\0\xffdata\n"),
        message: boxed(b"Add dir/file"),
        author: Author { name: boxed(b"kit"), email: boxed(b"kit@example.invalid") },
    }
}
#[test]
fn repository_creation_is_201_and_git_reads_its_readme() {
    let h = repository();
    assert_eq!(h.status(), 201);
    let tip = h.domain.branch(NEW, b"main").expect("created main");
    assert_eq!(
        h.domain.object(tip).expect("first commit").tree.get(b"README.md".as_slice()).expect("readme").as_ref(),
        b"# starter\n"
    );
}
#[test]
fn repository_creation_of_an_existing_name_is_409() {
    let mut h = repository();
    h.ask(b"POST", REPO_ROUTE, REPO_BODY);
    assert_eq!(h.status(), 409);
}
#[test]
fn a_created_repository_lists_only_explicit_collaborators() {
    let mut h = repository();
    let path = b"/api/v1/repos/creation/starter/collaborators?page=1&limit=1";
    // The organization owner can administer the repository without being a collaborator.
    h.ask(b"GET", path, b"");
    assert_eq!(h.status(), 200);
    assert_eq!(h.body().as_ref(), b"[]");

    let permission = b"/api/v1/repos/creation/starter/collaborators/kit/permission";
    h.ask(b"GET", permission, b"");
    assert_eq!(h.status(), 200);
    let request =
        Request::Permission { at: Repo { owner: boxed(b"creation"), name: boxed(b"starter") }, login: boxed(b"kit") };
    let Document::Permission(before) = h.decoded(&request).document else { panic!("permission answer") };
    assert_eq!(before.permission, skein_forge::forgejo::document::Permission::None);
    h.ask(b"PUT", b"/api/v1/repos/creation/starter/collaborators/kit", br#"{"permission":"write"}"#);
    assert_eq!(h.status(), 204);
    let token = b"created-repository-bot";
    h.service.setup_mut().token(fixtures::PERSON, b"bot", &[b"read:repository"], Secret::new(boxed(token)));
    h.ask_as(b"GET", path, b"", token);
    assert_eq!(h.status(), 200);
    assert_eq!(h.body().as_ref(), br#"[{"id":2,"login":"kit"}]"#);
    h.ask_as(b"GET", b"/api/v1/repos/creation/starter/collaborators?page=2&limit=1", b"", token);
    assert_eq!(h.body().as_ref(), b"[]", "implicit access does not consume a page row");
    h.ask_as(b"GET", permission, b"", token);
    assert_eq!(h.status(), 200);
    let Document::Permission(after) = h.decoded(&request).document else { panic!("permission answer") };
    assert_eq!(after.permission, skein_forge::forgejo::document::Permission::Write);
}
#[test]
fn repository_creation_in_a_missing_org_is_404() {
    let mut h = Harness::creating();
    h.ask(b"POST", REPO_ROUTE, REPO_BODY);
    assert_eq!(h.status(), 404);
}
#[test]
fn file_creation_is_201_and_branch_and_raw_find_its_commit() {
    let mut h = repository();
    let file = file();
    h.send(&file);
    assert_eq!(h.status(), 201);
    let Document::CreatedFile { commit } = h.decoded(&file).document else { panic!("created file") };
    let branch =
        Request::Branch { at: Repo { owner: boxed(b"creation"), name: boxed(b"starter") }, name: boxed(b"main") };
    h.send(&branch);
    let Document::Branch(branch) = h.decoded(&branch).document else { panic!("branch") };
    assert_eq!(branch.commit, commit);
    h.send(&Request::Raw {
        at: Repo { owner: boxed(b"creation"), name: boxed(b"starter") },
        path: boxed(b"dir/file"),
        reference: commit,
    });
    assert_eq!(h.status(), 200);
    assert_eq!(h.body().as_ref(), b"\0\xffdata\n");
    let tip = h.domain.branch(NEW, b"main").expect("branch");
    let object = h.domain.object(tip).expect("file commit");
    assert_eq!(object.author.as_ref().expect("author").name.as_ref(), b"kit");
    assert_eq!(object.author, object.committer);
}
#[test]
fn an_existing_file_is_422_and_keeps_the_same_head() {
    let mut h = repository();
    let file = file();
    h.send(&file);
    let head = h.domain.branch(NEW, b"main");
    h.send(&file);
    assert_eq!(h.status(), 422);
    assert_eq!(h.domain.branch(NEW, b"main"), head);
}
#[test]
fn creating_a_file_on_a_missing_branch_is_404() {
    let mut h = repository();
    let mut file = file();
    let Request::CreateFile { branch, .. } = &mut file else { unreachable!() };
    *branch = boxed(b"missing");
    h.send(&file);
    assert_eq!(h.status(), 404);
}
#[test]
fn creating_a_file_in_a_missing_repository_is_404() {
    let mut h = org();
    h.send(&file());
    assert_eq!(h.status(), 404);
}
#[test]
fn a_missing_file_reads_raw_404() {
    let mut h = repository();
    let reference = crate::routes::object(h.domain.branch(NEW, b"main").expect("main"));
    h.send(&Request::Raw {
        at: Repo { owner: boxed(b"creation"), name: boxed(b"starter") },
        path: boxed(b"missing"),
        reference,
    });
    assert_eq!(h.status(), 404);
}
