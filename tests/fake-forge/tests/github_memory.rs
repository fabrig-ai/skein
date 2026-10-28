//! The GitHub layer's full stores, measured independently of the shared domain.
#[path = "github/fixtures.rs"]
mod f;

use skein_fake_forge_github::{
    self as fake,
    installations::{Kind, Reach, Scope},
    pages::Page,
};
use skein_forge::github::request::Method;
use skein_lib::List;
use skein_world::domain::heap::{Counting, Meter};

#[global_allocator]
static HEAP: Counting = Counting;

// Bounds construction and requests while GitHub's stores are full.
const FAKE_GITHUB_SERVICE_PEAK_CEILING: u64 = 140_000;

#[test]
fn github_service_full_stores_stay_under_its_peak_ceiling() {
    // The queue belongs to the shared domain and has its own memory binary.
    // Keep every queue full while measuring only the GitHub layer's ownership.
    let (_domain, _domain_config) = f::domain();
    let meter = Meter::new();
    meter.start();
    let config = f::config();
    let limits = f::limits();
    let mut peak = 0;
    let mut service = fake::Service::new(config, &limits, f::SEED).expect("service");
    peak = peak.max(meter.check(meter.end(), FAKE_GITHUB_SERVICE_PEAK_CEILING, &"construction"));
    for (account, login, ids) in [(22, b"acme".as_slice(), [1, 2]), (23, b"example", [3, 4])] {
        meter.start();
        let mut listed = List::with_capacity(limits.installations.listed);
        for id in ids {
            listed.push(id).unwrap();
        }
        service.installations_mut().install(account, login, Kind::Organization, Reach::Listed(listed), f::NOW).unwrap();
        peak = peak.max(meter.check(meter.end(), FAKE_GITHUB_SERVICE_PEAK_CEILING, &"full installation reach"));
    }
    assert_eq!(service.installations().iter().count(), 2);
    let mut git_token = None;
    for n in 0..limits.installations.tokens {
        meter.start();
        let token = service.installations_mut().mint(1 + u64::from(n % 2), Scope::Full, f::NOW).unwrap();
        let measured = meter.end();
        if n == 0 {
            git_token = Some(token.0);
        }
        peak = peak.max(meter.check(measured, FAKE_GITHUB_SERVICE_PEAK_CEILING, &"mint"));
    }
    assert!(service.installations_mut().mint(1, Scope::Full, f::NOW).is_err(), "token store is full");
    let form = f::form();
    for (target, status) in [
        (b"/settings/apps/new?state=one".as_slice(), 200),
        (b"/settings/apps/new?state=two", 200),
        (b"/settings/apps/new?state=three", 503),
    ] {
        meter.start();
        let page = service.page(&Page { method: Method::Post, target, body: &form }, f::NOW);
        let measured = meter.end();
        assert_eq!(page.status, status);
        drop(page);
        peak = peak.max(meter.check(measured, FAKE_GITHUB_SERVICE_PEAK_CEILING, &"full manifest store"));
    }
    let blob_path = vec![b'x'; usize::try_from(limits.http.head).expect("head length")];
    for _ in 0..=limits.blob_records {
        meter.start();
        let fake::service::Outcome::Reply(response) = service.blob(
            Method::Get,
            &blob_path,
            false,
            true,
            f::NOW,
            skein_lib::ReplyTo::new(skein_lib::Token::new(1)),
        ) else {
            panic!("unknown blob target")
        };
        let measured = meter.end();
        assert_eq!(response.status, 404);
        drop(response);
        peak = peak.max(meter.check(measured, FAKE_GITHUB_SERVICE_PEAK_CEILING, &"full blob records"));
    }
    assert_eq!(service.blob_records().len(), limits.blob_records);
    let git_auth = basic(git_token.as_ref().unwrap().expose());
    // A body at its bound fills the compact decoder before its schema refusal.
    let mut body = b"[".to_vec();
    body.extend(std::iter::repeat_n(b'0', usize::try_from(limits.body).unwrap().saturating_sub(2)));
    body.push(b']');
    // Separate numeric tokens fill the record array as well as the text buffer.
    for byte in body.iter_mut().skip(2).step_by(2) {
        *byte = b',';
    }
    for _ in 0..=limits.git_records {
        meter.start();
        let response = f::ask(&mut service, Method::Post, b"/.fake/git/clone", Some(git_auth.expose()), &body);
        let measured = meter.end();
        assert_eq!(response.status, 400);
        drop(response);
        peak = peak.max(meter.check(measured, FAKE_GITHUB_SERVICE_PEAK_CEILING, &"full git decoder and records"));
    }
    assert_eq!(service.git_calls().len(), limits.git_records);

    let jwt = f::jwt(7, fake::keys::KEYS[0], f::NOW);
    for target in [b"/app".as_slice(), b"/app/installations?per_page=100&page=1"] {
        meter.start();
        let response = f::ask(&mut service, Method::Get, target, Some(jwt.expose()), b"");
        let measured = meter.end();
        assert_eq!(response.status, 200);
        drop(response);
        peak = peak.max(meter.check(measured, FAKE_GITHUB_SERVICE_PEAK_CEILING, &"full App and installation reads"));
    }
    assert!(service.manifest(b"one").is_some());
    assert!(service.manifest(b"two").is_some());
    assert!(peak <= FAKE_GITHUB_SERVICE_PEAK_CEILING);
}

fn basic(token: &[u8]) -> skein_lib::Secret {
    use skein_lib::{
        Writer,
        base64::{self, Alphabet},
    };
    let mut login = b"x-access-token:".to_vec();
    login.extend_from_slice(token);
    let count = base64::encoded_len(Alphabet::Standard, u32::try_from(login.len()).expect("bounded basic header"))
        .expect("bounded basic header");
    let mut header = Writer::new(usize::try_from(count).expect("bounded basic header").saturating_add(6));
    header.put(b"Basic ").expect("bounded basic header");
    base64::encode(Alphabet::Standard, &login, &mut header).expect("bounded basic header");
    skein_lib::Secret::new(header.finish())
}
