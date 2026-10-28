use super::fixtures;
use crate::{
    given::{self, Place, Refused},
    host::{File, Plan},
};
// forge-setup.md, 4.9: paths at most 255 bytes, repository names at most 100.
const PATH: usize = 255;
const NAME: usize = 100;
const STRING: u32 = 512;
fn plan(path: &[u8], content: &[u8]) -> Plan {
    Plan::Create {
        forge: 1,
        org: b"first".as_slice().into(),
        repository: b"starter".as_slice().into(),
        files: Box::new([File { path: path.into(), content: content.into() }]),
    }
}
fn check(plan: &Plan) -> Result<(), Refused> {
    given::check_create(&fixtures::limits(), STRING, plan)
}
fn refused(place: Place) -> Result<(), Refused> {
    Err(Refused { place, other: None })
}
#[test]
fn creation_accepts_a_path_at_255_bytes() {
    assert_eq!(check(&plan(&[b'a'; PATH], b"")), Ok(()));
}
#[test]
fn creation_refuses_a_path_one_byte_past_255() {
    assert_eq!(check(&plan(&[b'a'; PATH + 1], b"")), refused(Place::FilePath(0)));
}
#[test]
fn creation_accepts_content_at_its_byte_limit() {
    let bytes = skein_lib::bytes::zeroed(usize::try_from(fixtures::limits().file_bytes).unwrap());
    assert_eq!(check(&plan(b"file", &bytes)), Ok(()));
}
#[test]
fn creation_refuses_content_one_byte_past_its_limit() {
    let bytes = skein_lib::bytes::zeroed(usize::try_from(fixtures::limits().file_bytes).unwrap() + 1);
    assert_eq!(check(&plan(b"file", &bytes)), refused(Place::FileContent(0)));
}
#[test]
fn creation_refuses_absolute_empty_and_dot_segments() {
    for path in [b"".as_slice(), b"/file", b"file/", b"a//b", b".", b"..", b"a/../b", b"a/./b"] {
        assert_eq!(check(&plan(path, b"")), refused(Place::FilePath(0)));
    }
}
#[test]
fn creation_accepts_the_file_count_limit() {
    assert_eq!(check(&plan(b"file", b"")), Ok(()));
}
#[test]
fn creation_refuses_one_file_past_the_count_limit() {
    let mut plan = plan(b"file", b"");
    let Plan::Create { files, .. } = &mut plan else { unreachable!() };
    *files = Box::new([
        File { path: b"one".as_slice().into(), content: Box::new([]) },
        File { path: b"two".as_slice().into(), content: Box::new([]) },
    ]);
    assert_eq!(check(&plan), refused(Place::Files));
}
fn named(org: &[u8], repository: &[u8]) -> Plan {
    Plan::Create { forge: 1, org: org.into(), repository: repository.into(), files: Box::new([]) }
}
#[test]
fn creation_accepts_a_100_byte_repository_name() {
    assert_eq!(check(&named(b"first", &[b'a'; NAME])), Ok(()));
}
#[test]
fn creation_refuses_a_repository_name_past_100_bytes() {
    assert_eq!(check(&named(b"first", &[b'a'; NAME + 1])), refused(Place::CreateRepository));
}
#[test]
fn creation_repository_names_allow_a_leading_dot_or_dash() {
    for name in [b".repo".as_slice(), b"-repo", b"_repo"] {
        assert_eq!(check(&named(b"first", name)), Ok(()));
    }
}
#[test]
fn creation_refuses_non_names_in_both_namespaces() {
    for name in [b"".as_slice(), b"a b", b"a/b", b"\xff"] {
        assert_eq!(check(&named(name, b"repo")), refused(Place::CreateOrg));
        assert_eq!(check(&named(b"first", name)), refused(Place::CreateRepository));
    }
}
#[test]
fn creation_refuses_dot_and_dot_dot_repository_names() {
    for name in [b".".as_slice(), b".."] {
        assert_eq!(check(&named(b"first", name)), refused(Place::CreateRepository));
    }
}
#[test]
fn creation_names_must_fit_the_host_decoder_bound() {
    let limits = fixtures::limits();
    for (name, expected) in [(b"first".as_slice(), Ok(())), (b"firsts", refused(Place::CreateOrg))] {
        assert_eq!(given::check_create(&limits, 5, &named(name, b"repo")), expected);
    }
    for (name, expected) in [(b"repos".as_slice(), Ok(())), (b"reposx", refused(Place::CreateRepository))] {
        assert_eq!(given::check_create(&limits, 5, &named(b"first", name)), expected);
    }
}
#[test]
fn creation_orgs_follow_the_login_grammar_at_40_bytes() {
    assert_eq!(check(&named(&[b'a'; 40], b"repo")), Ok(()));
    assert_eq!(check(&named(&[b'a'; 41], b"repo")), refused(Place::CreateOrg));
    assert_eq!(check(&named(b"-org", b"repo")), refused(Place::CreateOrg));
}
#[test]
fn zero_file_limits_are_refused() {
    let mut limits = fixtures::limits();
    limits.files = 0;
    assert_eq!(given::check_create(&limits, STRING, &named(b"first", b"repo")), refused(Place::FilesLimit));
    limits.files = 1;
    limits.file_bytes = 0;
    assert_eq!(given::check_create(&limits, STRING, &named(b"first", b"repo")), refused(Place::FileBytesLimit));
}

#[test]
fn creation_with_a_decoder_bound_admin_login_fits_its_memory_bound() {
    use crate::{
        create,
        reconcile::{Event, Need, Reply},
    };
    use skein_forge::forgejo::{document::Document, request::Request};
    use skein_heap::Meter;
    use skein_lib::Queue;

    let given = fixtures::given();
    let limits = super::host::limits();
    let bound = crate::memory::per_run(&given, &limits).expect("creation bound");
    let meter = Meter::new();
    meter.start();
    let mut machine = Box::new(
        create::Create::new(
            &given,
            Box::from([b'o'; 40]),
            Box::from([b'r'; NAME]),
            Box::new([File {
                path: Box::from([b'p'; PATH]),
                content: skein_lib::bytes::zeroed(usize::try_from(given.limits.file_bytes).expect("file bound")),
            }]),
        )
        .expect("maximum creation"),
    );
    let mut out = Queue::uncapped();
    create::step(&mut machine, Event::Begin, &mut out);
    drop(out.pop());
    create::step(&mut machine, Event::Read(Some(fixtures::secret(fixtures::ADMIN))), &mut out);
    drop(out.pop());
    let mut admin = fixtures::admin();
    admin.login = skein_lib::bytes::zeroed(usize::try_from(limits.string_bytes).expect("decoder bound"));
    admin.login.fill(b'a');
    create::step(&mut machine, Event::Reply(Reply::Document(Document::User(admin))), &mut out);
    drop(out.pop());
    create::step(&mut machine, Event::Reply(Reply::Status(404)), &mut out);
    let measured = meter.end();
    assert!(measured.peak() <= bound, "{} exceeds {bound}", measured.peak());
    let Some(Need::Send { request: crate::call::Call::Forgejo(Request::CreateOrg { person, .. }), .. }) = out.pop()
    else {
        panic!("missing org is made");
    };
    assert_eq!(person.len(), usize::try_from(limits.string_bytes).expect("decoder bound"));
    drop(person);
    drop(machine);
    drop(out);
    assert_eq!(meter.held(), 0);
}
