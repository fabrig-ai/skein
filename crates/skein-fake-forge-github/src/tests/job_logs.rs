use super::{fixtures as f, read_support as r};
use crate::service::{Outcome, Response};
use skein_fake_forge_domain::api as a;
use skein_forge::github::{self, request::Method};
use skein_lib::{Duration, ReplyTo, Token, Wall};

fn failing() -> (f::Harness, u64) {
    configured(f::config())
}
fn configured(config: crate::Config) -> (f::Harness, u64) {
    let mut h = f::Harness::with_checks(
        f::limits(),
        config,
        r::domain_config(),
        None,
        a::Checks {
            contexts: Box::from([f::boxed(b"ci")]),
            latency_min: Duration::ZERO,
            latency_max: Duration::ZERO,
            silent: 0,
            passes: 0,
            reruns: 0,
            cue: None,
        },
    );
    let (_, head) = r::pull(&mut h);
    (h, head)
}
fn blob(h: &mut f::Harness, target: &[u8], range: bool, now: Wall) -> Response {
    let outcome = h.service.blob(Method::Get, target, false, range, now, ReplyTo::new(Token::new(77)));
    match outcome {
        Outcome::Admit(_) => panic!("blob fetches need no token admission"),
        Outcome::Reply(response) => response,
        Outcome::Call(pending, event) => {
            let result = h.drive(event);
            let outcome = h.service.finish(pending, result, ReplyTo::new(Token::new(77)), &h.domain);
            let Outcome::Reply(response) = outcome else { panic!("one domain log read") };
            response
        }
    }
}
#[test]
fn github_fake_job_log_redirect_expires_at_600_seconds_and_blob_ignores_range() {
    let (mut h, _) = failing();
    let token = r::token(&mut h);
    let issued = h.now;
    let answer = h.ask(Method::Get, b"/repos/acme/widgets/actions/jobs/1/logs", Some(token.expose()), b"");
    assert_eq!(answer.status, 302);
    assert!(answer.body.is_empty());
    let redirect = github::redirect::parse(answer.location.as_deref().expect("Location"), 1024, 1024).unwrap();
    assert_eq!(redirect.origin.host.as_ref(), b"logs.example.test");
    let a::Answer::File(expected) =
        h.domain.inspect(&h.env.limits, b"acme/widgets", &a::Read::Log { job: 1, attempt: 1, step: None }).unwrap()
    else {
        panic!("whole domain log")
    };
    for (seconds, range, status) in [(0, false, 200), (599, true, 200), (600, false, 403)] {
        let now = Wall::from_nanos(issued.as_nanos().checked_add(seconds * 1_000_000_000).unwrap());
        let response = blob(&mut h, &redirect.target, range, now);
        assert_eq!(response.status, status);
        if status == 200 {
            assert_eq!(response.body, expected);
        } else {
            assert!(response.body.is_empty());
        }
        let record = h.service.blob_records().iter().last().unwrap();
        assert!(record.path == redirect.target);
        assert!(!record.authorization);
        assert_eq!(record.range, range);
    }
    for part in [b"/logs/2/".as_slice(), b"/1?expires=", b"?expires=", b"&signature="] {
        let mut tampered = f::boxed(&redirect.target);
        let at = skein_lib::bytes::find(&tampered, part).unwrap() + part.len();
        tampered[at] = if tampered[at] == b'1' { b'2' } else { b'1' };
        let response = blob(&mut h, &tampered, false, issued);
        assert_eq!(response.status, 403);
        assert!(response.body.is_empty());
    }
    for (target, status) in [(b"/other".as_slice(), 404), (b"/logs/invalid", 403)] {
        let response = blob(&mut h, target, false, issued);
        assert_eq!(response.status, status);
        assert!(response.body.is_empty());
    }
    assert_eq!(h.ask(Method::Get, b"/repos/acme/widgets/actions/jobs/999/logs", Some(token.expose()), b"").status, 404);
}

#[test]
fn github_fake_blob_record_and_origins_stay_within_admitted_bounds() {
    let limits = f::limits();
    let mut service = crate::Service::new(f::config(), &limits, 5).unwrap();
    let path = f::boxed(&[b'x'; 4096]);
    assert_eq!(path.len(), usize::try_from(limits.http.head).unwrap());
    for _ in 0..=limits.blob_records {
        let Outcome::Reply(response) =
            service.blob(Method::Get, &path, true, true, Wall::EPOCH, ReplyTo::new(Token::new(1)))
        else {
            panic!("unknown blob path")
        };
        assert_eq!(response.status, 404);
    }
    let records = service.blob_records();
    assert_eq!(records.len(), limits.blob_records);
    let mut held = skein_lib::Queue::<crate::logs::Record>::worst_case(limits.blob_records).unwrap();
    for row in records {
        held = held.checked_add(u64::try_from(row.path.len()).unwrap()).unwrap();
    }
    assert!(held <= crate::logs::worst_case(&limits).unwrap());
    service.set_blob_origin(f::boxed(b"http://logs.example.test")).unwrap();
    let oversized = crate::service::joined(b"http://", &[b'a'; 1024]).unwrap();
    assert!(service.set_blob_origin(oversized).is_err());
}

#[test]
fn github_fake_job_log_redirects_to_a_third_origin_when_the_scenario_names_one() {
    for origin in [b"http://third.example.test".as_slice(), b"http://unfamiliar.example.test:8443"] {
        let mut config = f::config();
        config.third_origin = Some(f::boxed(origin));
        let (mut h, _) = configured(config);
        let token = r::token(&mut h);
        let answer = h.ask(Method::Get, b"/repos/acme/widgets/actions/jobs/1/logs", Some(token.expose()), b"");
        assert_eq!(answer.status, 302);
        assert!(answer.body.is_empty());
        let location = answer.location.expect("third origin Location");
        assert!(location.starts_with(origin));
        let redirect = github::redirect::parse(&location, 1024, 1024).unwrap();
        assert!(redirect.target.starts_with(b"/logs/"));
        assert!(h.service.blob_records().is_empty(), "redirect emits no blob request");
    }
}

#[test]
fn github_fake_blob_answers_a_redirect_when_the_scenario_asks() {
    for origin in [b"http://another.example.test".as_slice(), b"http://another.example.test:8443"] {
        let mut config = f::config();
        config.blob_redirect = Some(f::boxed(origin));
        let (mut h, _) = configured(config);
        let token = r::token(&mut h);
        let issued = h.now;
        let answer = h.ask(Method::Get, b"/repos/acme/widgets/actions/jobs/1/logs", Some(token.expose()), b"");
        let redirect = github::redirect::parse(answer.location.as_deref().expect("blob Location"), 1024, 1024).unwrap();
        let response = blob(&mut h, &redirect.target, true, issued);
        assert_eq!(response.status, 302);
        assert!(response.body.is_empty());
        assert_eq!(response.location, crate::service::joined(origin, &redirect.target));
        let unknown = blob(&mut h, b"/other", false, issued);
        assert_eq!(unknown.status, 404);
        let invalid = blob(&mut h, b"/logs/invalid", false, issued);
        assert_eq!(invalid.status, 403);
    }
}

#[test]
fn github_fake_log_origins_are_checked_at_admission() {
    use crate::{Problem, Service};
    let oversized = crate::service::joined(b"http://", &[b'a'; 1024]).unwrap();
    for origin in [
        b"".as_slice(),
        b"ftp://logs.example.test",
        b"http://logs.example.test/path",
        b"http://logs.example.test\r\n",
        &oversized,
    ] {
        for choice in 0_u8..3 {
            let mut config = f::config();
            match choice {
                0 => config.blob = f::boxed(origin),
                1 => config.third_origin = Some(f::boxed(origin)),
                2 => config.blob_redirect = Some(f::boxed(origin)),
                _ => panic!("three origin choices"),
            }
            assert_eq!(Service::new(config, &f::limits(), 5).expect_err("invalid origin"), Problem::Config);
        }
    }
    let mut limits = f::limits();
    limits.blob_records = 0;
    assert_eq!(Service::new(f::config(), &limits, 5).expect_err("zero record bound"), Problem::Limits);
}
