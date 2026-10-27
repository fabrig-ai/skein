//! The server consumes a bundle without linking its producer.
use skein_scratch::Scratch;
use skein_ui_bundle::{Glue, Inputs};
use skein_ui_example_server::{
    State,
    bundle::{self, Bundle},
    routes,
};
use std::path::Path;
#[test]
fn the_servers_types_are_the_bundlers() {
    for (path, expected) in [
        ("a.html", Some("text/html; charset=utf-8")),
        ("a.js", Some("text/javascript")),
        ("a.wasm", Some("application/wasm")),
        ("a.css", Some("text/css")),
        ("a.svg", Some("image/svg+xml")),
        ("a.png", Some("image/png")),
        ("a.webp", Some("image/webp")),
        ("a.ico", Some("image/x-icon")),
        ("a.woff2", Some("font/woff2")),
        ("a.txt", Some("text/plain; charset=utf-8")),
        ("a.webmanifest", Some("application/manifest+json")),
        ("a.unknown", None),
    ] {
        assert_eq!(bundle::of(Path::new(path)), expected, "server {path}");
        assert_eq!(skein_ui_bundle::types::of(Path::new(path)), expected, "bundler {path}");
    }
}
#[test]
fn the_server_serves_a_bundle_it_did_not_make() {
    let scratch = Scratch::new("example-serves-bundle");
    let out = scratch.path().join("bundle");
    let inputs = Inputs {
        worker: None,
        name: "notes".into(),
        prefix: "/bundle/".into(),
        stylesheets: vec![],
        pictures: vec![],
        page: None,
        files: None,
        noscript: "Scripts required.".into(),
    };
    let glue = Glue { js: b"export default function init() {}".to_vec(), wasm: b"\0asm".to_vec(), snippets: vec![] };
    let digest = skein_ui_bundle::bundle(&inputs, &glue, None, &out).expect("bundle written");
    let bundle = Bundle::read(&out).expect("server reads startup files");
    assert_eq!(bundle.digest, digest.to_string());
    let mut state = State::new();
    let answer = routes::route(&mut state, &bundle, &skein_ui_web_tests::get(b"/notes/7"), &[]);
    assert_eq!(answer.head.status, 200);
    assert_eq!(answer.body.as_ref(), std::fs::read(out.join("index.html")).expect("page").as_slice());
    for (name, expected, kind) in [
        ("notes.js", glue.js.as_slice(), "text/javascript"),
        ("notes_bg.wasm", glue.wasm.as_slice(), "application/wasm"),
    ] {
        let path = format!("/bundle/{digest}/{name}");
        let answer = routes::route(&mut state, &bundle, &skein_ui_web_tests::get(path.as_bytes()), &[]);
        assert_eq!(answer.head.status, 200, "{name}");
        assert_eq!(answer.body.as_ref(), expected, "{name}");
        for (name, expected) in
            [(b"Content-Type".as_slice(), kind.as_bytes()), (b"Cache-Control", b"public, max-age=31536000, immutable")]
        {
            let header =
                answer.head.headers.iter().find(|header| header.name.eq_ignore_ascii_case(name)).expect("file header");
            assert_eq!(header.value.as_ref(), expected);
        }
    }
}
