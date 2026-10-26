use super::{Glue, Inputs, Refusal, STYLESHEET, bundle};
use skein_scratch::Scratch;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

fn inputs(scratch: &Scratch) -> Inputs {
    let path = scratch.path();
    fs::create_dir_all(path.join("pictures")).expect("pictures");
    fs::write(path.join("pictures/icon.svg"), b"<svg/>").expect("picture");
    fs::create_dir_all(path.join("files")).expect("files");
    fs::write(path.join("files/about.txt"), b"About").expect("file");
    fs::write(path.join("a.css"), b"body { color: navy; }").expect("stylesheet");
    Inputs {
        worker: None,
        name: "notes".into(),
        prefix: "/static/".into(),
        stylesheets: vec![path.join("a.css")],
        pictures: vec![("notes".into(), path.join("pictures"))],
        page: None,
        files: Some(path.join("files")),
        noscript: "Notes needs WebAssembly and scripts.".into(),
    }
}

fn glue() -> Glue {
    Glue {
        js: b"export default function init() {}".to_vec(),
        wasm: b"\0asm".to_vec(),
        snippets: vec![("notes/grow.js".into(), b"export function grow() {}".to_vec())],
    }
}

#[test]
fn the_digest_covers_paths_and_bytes() {
    let scratch = Scratch::new("bundle-digest");
    let mut inputs = inputs(&scratch);
    let out = scratch.path().join("out");
    let original = bundle(&inputs, &glue(), None, &out).expect("bundle");
    fs::write(scratch.path().join("files/about.txt"), b"about").expect("change one byte");
    let changed = bundle(&inputs, &glue(), None, &out).expect("bundle");
    assert_ne!(original, changed, "bytes affect the digest");
    inputs.name = "other".into();
    let renamed = bundle(&inputs, &glue(), None, &out).expect("bundle");
    assert_ne!(changed, renamed, "paths affect the digest");
}

fn contents(directory: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(directory).expect("directory") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).expect("relative").to_str().expect("utf8").to_owned(),
                    fs::read(&path).expect("file"),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(directory, directory, &mut files);
    files
}

#[test]
fn the_same_inputs_give_the_same_bundle() {
    let scratch = Scratch::new("bundle-same");
    let inputs = inputs(&scratch);
    let a = scratch.path().join("a");
    let b = scratch.path().join("b");
    fs::create_dir(&a).expect("out");
    fs::write(a.join("stale.txt"), b"stale").expect("old output");
    let first = bundle(&inputs, &glue(), None, &a).expect("first");
    let second = bundle(&inputs, &glue(), None, &b).expect("second");
    assert_eq!(first, second);
    let files = contents(&a);
    assert_eq!(files, contents(&b));
    assert_eq!(files.len(), 9);
    assert_eq!(files.get(&format!("{first}/files/about.txt")), Some(&b"About".to_vec()));
}

#[test]
fn the_page_matches_its_golden() {
    let scratch = Scratch::new("bundle-golden");
    let inputs = inputs(&scratch);
    let out = scratch.path().join("out");
    bundle(&inputs, &glue(), None, &out).expect("bundle");
    assert_eq!(fs::read(out.join("index.html")).expect("page"), include_bytes!("tests/golden/index.html"));
}

#[test]
fn an_own_page_gets_the_directory_at_each_marker() {
    let scratch = Scratch::new("bundle-own");
    let mut inputs = inputs(&scratch);
    let page = scratch.path().join("own.html");
    fs::write(&page, b"\xff{bundle}boot.js {bundle}a.css").expect("own page bytes");
    inputs.page = Some(page);
    let out = scratch.path().join("out");
    let digest = bundle(&inputs, &glue(), None, &out).expect("bundle");
    let mut expected = vec![255];
    expected.extend_from_slice(format!("/static/{digest}/boot.js /static/{digest}/a.css").as_bytes());
    assert_eq!(fs::read(out.join("index.html")).expect("page"), expected);
}

#[test]
fn an_own_page_without_the_marker_is_refused() {
    let scratch = Scratch::new("bundle-marker");
    let mut inputs = inputs(&scratch);
    let page = scratch.path().join("own.html");
    fs::write(&page, b"<html></html>").expect("own page");
    inputs.page = Some(page.clone());
    assert_eq!(bundle(&inputs, &glue(), None, &scratch.path().join("out")), Err(Refusal::NoMarker { page }));
}

#[test]
fn references_go_under_the_prefix() {
    let scratch = Scratch::new("bundle-prefix");
    let mut inputs = inputs(&scratch);
    inputs.prefix = "/assets/ui/".into();
    inputs.noscript = "Use <scripts> & WebAssembly".into();
    let out = scratch.path().join("out");
    let digest = bundle(&inputs, &glue(), None, &out).expect("bundle");
    let page = fs::read_to_string(out.join("index.html")).expect("page");
    for reference in ["skein-ui.css", "a.css", "boot.js"] {
        assert!(page.contains(&format!("\"/assets/ui/{digest}/{reference}\"")), "{reference}");
    }
    assert!(page.contains("Use &lt;scripts&gt; &amp; WebAssembly"));
    assert_eq!(
        fs::read(out.join(digest.to_string()).join("boot.js")).expect("starter"),
        b"import init from \"./notes.js\";\ninit();\n"
    );
}

#[test]
fn the_stylesheet_names_no_class() {
    let stylesheet = include_str!("../skein-ui.css");
    let mut remaining = stylesheet;
    let mut without_comments = String::new();
    while let Some(start) = remaining.find("/*") {
        without_comments.push_str(&remaining[..start]);
        let comment = &remaining[start + 2..];
        remaining = &comment[comment.find("*/").expect("comment closes") + 2..];
    }
    without_comments.push_str(remaining);
    assert!(!without_comments.contains('.'), "the stylesheet uses no class selector");
}

#[test]
fn skein_ui_css_comes_first_in_the_page() {
    let scratch = Scratch::new("bundle-css");
    let inputs = inputs(&scratch);
    let out = scratch.path().join("out");
    let digest = bundle(&inputs, &glue(), None, &out).expect("bundle");
    let page = fs::read_to_string(out.join("index.html")).expect("page");
    assert!(page.find("/skein-ui.css").expect("kit stylesheet") < page.find("/a.css").expect("owner stylesheet"));
    assert_eq!(fs::read(out.join(digest.to_string()).join("skein-ui.css")).expect("stylesheet"), STYLESHEET);
}

#[test]
fn each_refusal_names_its_file() {
    let scratch = Scratch::new("bundle-refusals");
    let base = inputs(&scratch);
    let paths =
        [scratch.path().join("bad.xml"), scratch.path().join("skein-ui.css"), scratch.path().join("other/a.css")];
    fs::create_dir(scratch.path().join("other")).expect("other directory");
    for path in &paths {
        fs::write(path, b"input").expect("input file");
    }
    let expected = [
        Refusal::Type { file: paths[0].clone() },
        Refusal::ReservedName { file: paths[1].clone() },
        Refusal::SameName { file: paths[2].clone() },
    ];
    for (path, expected) in paths.iter().zip(expected) {
        let mut inputs = base.clone();
        inputs.stylesheets.push(path.clone());
        assert_eq!(bundle(&inputs, &glue(), None, &scratch.path().join("out")), Err(expected), "{}", path.display());
    }
}

#[test]
fn each_extension_has_its_type() {
    const CASES: &[(&str, Option<&str>)] = &[
        ("index.html", Some("text/html; charset=utf-8")),
        ("boot.js", Some("text/javascript")),
        ("notes.wasm", Some("application/wasm")),
        ("a.css", Some("text/css")),
        ("a.svg", Some("image/svg+xml")),
        ("a.png", Some("image/png")),
        ("a.webp", Some("image/webp")),
        ("a.ico", Some("image/x-icon")),
        ("a.woff2", Some("font/woff2")),
        ("about.txt", Some("text/plain; charset=utf-8")),
        ("app.webmanifest", Some("application/manifest+json")),
        ("a.xml", None),
        ("no-extension", None),
    ];
    for (name, expected) in CASES {
        assert_eq!(super::types::of(Path::new(name)), *expected, "{name}");
    }
}

#[test]
fn the_glue_loads_its_sibling_module_when_init_has_no_argument() {
    let scratch = Scratch::new("glue-module-path");
    let module = scratch.path().join("notes.wasm");
    fs::write(&module, b"\0asm\x01\0\0\0").expect("empty module");
    let glue = super::glue(&module, "notes").expect("web glue");
    let js = std::str::from_utf8(&glue.js).expect("generated JavaScript");
    assert!(js.contains("new URL('notes_bg.wasm', import.meta.url)"));
}

const PAGE_FILES: &[&str] = &[
    "a.css",
    "boot.js",
    "files/about.txt",
    "notes.js",
    "notes_bg.wasm",
    "pictures/notes/icon.svg",
    "skein-ui.css",
    "snippets/notes/grow.js",
];

#[test]
fn a_bundle_without_a_worker_is_unchanged() {
    let scratch = Scratch::new("bundle-no-worker");
    let inputs = inputs(&scratch);
    let out = scratch.path().join("out");
    let digest = bundle(&inputs, &glue(), None, &out).expect("bundle");
    let files = contents(&out.join(digest.to_string()));
    assert_eq!(files.keys().map(String::as_str).collect::<Vec<_>>(), PAGE_FILES);
    assert!(!out.join(digest.to_string()).join("worker.js").exists());
}

#[test]
fn a_worker_module_lies_beside_the_page_module() {
    let scratch = Scratch::new("bundle-worker");
    let mut inputs = inputs(&scratch);
    inputs.worker = Some("world".into());
    let out = scratch.path().join("out");
    let mut worker = glue();
    worker.snippets = vec![("world/run.js".into(), b"run".to_vec())];
    let digest = bundle(&inputs, &glue(), Some(&worker), &out).expect("bundle");
    let files = contents(&out.join(digest.to_string()));
    let mut expected = PAGE_FILES.to_vec();
    expected.extend(["snippets/world/run.js", "worker.js", "world.js", "world_bg.wasm"]);
    expected.sort_unstable();
    assert_eq!(files.keys().map(String::as_str).collect::<Vec<_>>(), expected);
    assert_eq!(files.get("worker.js").expect("starter"), b"import init from \"./world.js\";\ninit();\n");
    assert_eq!(files.get("world.js"), Some(&worker.js));
    assert_eq!(files.get("world_bg.wasm"), Some(&worker.wasm));
    assert_eq!(files.get("snippets/world/run.js").expect("worker snippet"), b"run");
    let page = fs::read_to_string(out.join("index.html")).expect("page");
    assert!(page.contains(&format!("/static/{digest}/boot.js")));
    assert!(!page.contains("worker.js"));
}

#[test]
fn the_digest_covers_the_worker_module() {
    let scratch = Scratch::new("bundle-worker-digest");
    let mut inputs = inputs(&scratch);
    inputs.worker = Some("world".into());
    let out = scratch.path().join("out");
    let mut worker = glue();
    let original = bundle(&inputs, &glue(), Some(&worker), &out).expect("bundle");
    worker.wasm[0] = 1;
    let changed = bundle(&inputs, &glue(), Some(&worker), &out).expect("changed bundle");
    assert_ne!(original, changed);
}

#[test]
fn a_snippet_both_modules_hold_is_written_once() {
    let scratch = Scratch::new("bundle-worker-shared");
    let mut inputs = inputs(&scratch);
    inputs.worker = Some("world".into());
    let out = scratch.path().join("out");
    let digest = bundle(&inputs, &glue(), Some(&glue()), &out).expect("shared snippet");
    let files = contents(&out.join(digest.to_string()));
    assert_eq!(files.len(), PAGE_FILES.len() + 3);
    assert_eq!(files.get("snippets/notes/grow.js"), Some(&glue().snippets[0].1));
}

#[test]
fn a_snippet_differing_between_the_modules_is_refused() {
    let scratch = Scratch::new("bundle-worker-snippet-clash");
    let mut inputs = inputs(&scratch);
    inputs.worker = Some("world".into());
    let mut worker = glue();
    worker.snippets[0].1.push(b'!');
    assert_eq!(
        bundle(&inputs, &glue(), Some(&worker), &scratch.path().join("out")),
        Err(Refusal::SameName { file: "notes/grow.js".into() })
    );
}

#[test]
fn a_worker_named_as_the_page_module_is_refused() {
    let scratch = Scratch::new("bundle-worker-name");
    let mut inputs = inputs(&scratch);
    inputs.worker = Some(inputs.name.clone());
    assert_eq!(
        bundle(&inputs, &glue(), Some(&glue()), &scratch.path().join("out")),
        Err(Refusal::SameName { file: "notes".into() })
    );
}

#[test]
fn a_worker_without_its_glue_is_refused() {
    let scratch = Scratch::new("bundle-worker-missing");
    let mut inputs = inputs(&scratch);
    let out = scratch.path().join("out");
    assert_eq!(bundle(&inputs, &glue(), Some(&glue()), &out), Err(Refusal::Worker));
    inputs.worker = Some("world".into());
    assert_eq!(bundle(&inputs, &glue(), None, &out), Err(Refusal::Worker));
}

#[test]
fn the_worker_starters_name_is_reserved() {
    let scratch = Scratch::new("bundle-worker-reserved");
    let mut inputs = inputs(&scratch);
    for (page, worker) in [("worker", "world"), ("notes", "worker"), ("notes", "boot")] {
        inputs.name = page.into();
        inputs.worker = Some(worker.into());
        assert_eq!(
            bundle(&inputs, &glue(), Some(&glue()), &scratch.path().join("out")),
            Err(Refusal::SameName { file: worker.into() })
        );
    }
}
