//! Shared UI example fixtures, for host tests and the browser tier.
use skein_http::server::{Body, Call};
use skein_http::{Method, Version};
#[must_use]
pub fn get(path: &[u8]) -> Call {
    Call {
        method: Method::Get,
        target: Box::from(path),
        version: Version::Http11,
        headers: Box::from([]),
        body: Body::None,
    }
}

/// The binary built by the browser suite's setup.
#[must_use]
pub fn server_binary() -> std::path::PathBuf {
    let exe = std::env::current_exe().expect("test executable");
    let binary = exe.parent().and_then(std::path::Path::parent).expect("target directory").join("skein-ui-example");
    assert!(
        binary.is_file(),
        "build the browser server with cargo build -p skein-ui-example-server: {}",
        binary.display()
    );
    binary
}
#[must_use]
pub fn chromium() -> std::path::PathBuf {
    use std::path::PathBuf;
    if let Some(configured) = std::env::var_os("SKEIN_TEST_CHROMIUM") {
        let path = PathBuf::from(configured);
        assert!(path.is_file(), "SKEIN_TEST_CHROMIUM does not name a file: {}", path.display());
        return path.canonicalize().expect("resolve configured Chromium path");
    }
    let path = std::env::var_os("PATH").expect("PATH locates chromium");
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join("chromium");
        if candidate.is_file() {
            return candidate.canonicalize().expect("resolve Chromium on PATH");
        }
    }
    panic!("Chromium is required for the browser suite: set SKEIN_TEST_CHROMIUM or put chromium on PATH");
}
/// A module literal carried through the real bundle and strict policy.
#[must_use]
pub fn fixture_bundle(scratch: &skein_scratch::Scratch, module: &[u8]) -> std::path::PathBuf {
    let out = scratch.path().join("bundle");
    let inputs = skein_ui_bundle::Inputs {
        worker: None,
        name: "fixture".into(),
        prefix: "/bundle/".into(),
        stylesheets: vec![],
        pictures: vec![],
        page: None,
        files: None,
        noscript: "Scripts required.".into(),
    };
    let glue = skein_ui_bundle::Glue { js: module.to_vec(), wasm: b"\0asm".to_vec(), snippets: vec![] };
    skein_ui_bundle::bundle(&inputs, &glue, None, &out).expect("fixture bundle");
    out
}
#[must_use]
pub fn fixture_harness(bundle: &std::path::Path) -> skein_fake_person_browser::Harness {
    fixture_harness_with(bundle, vec![])
}
#[must_use]
pub fn fixture_harness_with(
    bundle: &std::path::Path,
    origins: Vec<skein_fake_person_browser::origins::Origin>,
) -> skein_fake_person_browser::Harness {
    skein_fake_person_browser::Harness::new(skein_fake_person_browser::Setup {
        server: server_binary(),
        args: vec!["127.0.0.1:0".into(), "--bundle".into(), bundle.as_os_str().into()],
        printed: skein_fake_person_browser::harness::Printed {
            sent: skein_fake_person_browser::harness::Sent::Page,
            prefix: b"skein-ui-example: listening at ",
            on: skein_fake_person_browser::harness::Output::Stderr,
            names: skein_fake_person_browser::harness::Names::Address,
        },
        restart: vec!["{address}".into(), "--bundle".into(), bundle.as_os_str().into()],
        downloads: None,
        chromium: chromium(),
        origins,
        expected: vec![],
    })
}

/// The same bound the first story gives each visible expectation.
pub const SECOND: skein_lib::Duration = skein_lib::Duration::from_secs(2);

/// The bundle made by the repository gate from its dev-profile module.
#[must_use]
pub fn example_bundle() -> std::path::PathBuf {
    let directory = std::env::var_os("SKEIN_UI_EXAMPLE_BUNDLE")
        .map(std::path::PathBuf::from)
        .expect("run scripts/browser.sh to build the example bundle and set SKEIN_UI_EXAMPLE_BUNDLE");
    assert!(
        directory.join("index.html").is_file(),
        "run scripts/browser.sh to provide the example bundle at {}",
        directory.display()
    );
    directory
}
#[must_use]
pub fn example_harness() -> skein_fake_person_browser::Harness {
    fixture_harness(&example_bundle())
}

#[must_use]
pub fn page(path: &[u8], body: &[u8]) -> skein_fake_person_browser::origins::Page {
    skein_fake_person_browser::origins::Page {
        path: path.into(),
        kind: b"text/html; charset=utf-8".as_slice().into(),
        body: body.into(),
        headers: vec![],
    }
}

pub mod harness_peer;

/// Prebuilt beside the UI test executables, as the example server is.
#[must_use]
pub fn harness_fixture() -> std::path::PathBuf {
    let exe = std::env::current_exe().expect("test executable");
    let binary = exe.parent().and_then(std::path::Path::parent).expect("target directory").join("harness_fixture");
    assert!(binary.is_file(), "prebuild skein-ui-web-tests --bins");
    binary
}

#[must_use]
pub fn lending_harness(
    args: &[&str],
    origins: Vec<skein_fake_person_browser::harness::Origin>,
) -> skein_fake_person_browser::Harness {
    use skein_fake_person_browser::harness::{Harness, Names, Output, Printed, Setup};
    Harness::new(Setup {
        server: harness_fixture(),
        args: std::iter::once("server").chain(args.iter().copied()).map(Into::into).collect(),
        printed: Printed {
            sent: skein_fake_person_browser::harness::Sent::Page,
            prefix: b"fixture listening ",
            on: Output::Stderr,
            names: Names::Address,
        },
        restart: vec!["server".into()],
        downloads: None,
        chromium: chromium(),
        expected: vec![],
        origins,
    })
}
