use std::path::PathBuf;
use std::process::ExitCode;

use skein_ui_bundle::{Inputs, bundle, glue};

const USAGE: &str = "usage: skein-ui-bundle --module PATH --name NAME --prefix /PREFIX/ --out DIR [--worker-module PATH --worker-name NAME] [--stylesheet PATH]… [--pictures OWNER-PREFIX=DIR]… [--page PATH] [--files DIR] [--noscript WORDS]";

#[derive(Debug, PartialEq, Eq)]
struct Arguments {
    module: PathBuf,
    worker_module: Option<PathBuf>,
    out: PathBuf,
    inputs: Inputs,
}

fn arguments(args: &[String]) -> Result<Arguments, &'static str> {
    let mut module = None;
    let mut worker_module = None;
    let mut out = None;
    let mut name = None;
    let mut prefix = None;
    let mut inputs = Inputs {
        worker: None,
        name: String::new(),
        prefix: String::new(),
        stylesheets: Vec::new(),
        pictures: Vec::new(),
        page: None,
        files: None,
        noscript: "This application needs WebAssembly and scripts.".into(),
    };
    let mut noscript = None;
    let mut pairs = args.chunks_exact(2);
    for pair in &mut pairs {
        let value = &pair[1];
        match pair[0].as_str() {
            "--module" if module.is_none() => module = Some(PathBuf::from(value)),
            "--worker-module" if worker_module.is_none() => worker_module = Some(PathBuf::from(value)),
            "--worker-name" if inputs.worker.is_none() && segment(value) => inputs.worker = Some(value.clone()),
            "--out" if out.is_none() => out = Some(PathBuf::from(value)),
            "--name" if name.is_none() && segment(value) => name = Some(value.clone()),
            "--prefix" if prefix.is_none() && valid_prefix(value) => prefix = Some(value.clone()),
            "--stylesheet" => inputs.stylesheets.push(value.into()),
            "--pictures" => {
                let (owner, directory) = value.split_once('=').ok_or(USAGE)?;
                if !segment(owner) || directory.is_empty() {
                    return Err(USAGE);
                }
                inputs.pictures.push((owner.into(), directory.into()));
            }
            "--page" if inputs.page.is_none() => inputs.page = Some(value.into()),
            "--files" if inputs.files.is_none() => inputs.files = Some(value.into()),
            "--noscript" if noscript.is_none() => noscript = Some(value.clone()),
            _ => return Err(USAGE),
        }
    }
    if !pairs.remainder().is_empty() || worker_module.is_some() != inputs.worker.is_some() {
        return Err(USAGE);
    }
    inputs.name = name.ok_or(USAGE)?;
    inputs.prefix = prefix.ok_or(USAGE)?;
    if let Some(words) = noscript {
        inputs.noscript = words;
    }
    Ok(Arguments { module: module.ok_or(USAGE)?, worker_module, out: out.ok_or(USAGE)?, inputs })
}

fn segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

fn valid_prefix(value: &str) -> bool {
    value
        .strip_prefix('/')
        .and_then(|value| value.strip_suffix('/'))
        .is_some_and(|middle| middle.split('/').all(segment))
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = arguments(&args).map_err(str::to_owned)?;
    let page_glue = glue(&args.module, &args.inputs.name).map_err(|error| error.to_string())?;
    let worker = args
        .worker_module
        .as_ref()
        .zip(args.inputs.worker.as_ref())
        .map(|(module, name)| glue(module, name))
        .transpose()
        .map_err(|error| error.to_string())?;
    let digest = bundle(&args.inputs, &page_glue, worker.as_ref(), &args.out).map_err(|error| error.to_string())?;
    println!("{}", args.out.join(digest.to_string()).display());
    Ok(())
}

fn main() -> ExitCode {
    skein_fail_stop::install();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Arguments, Inputs, USAGE, arguments};

    const REQUIRED: &[&str] = &["--module", "notes.wasm", "--name", "notes", "--prefix", "/static/", "--out", "out"];
    const OPTIONAL: &[&str] = &[
        "--stylesheet",
        "a.css",
        "--stylesheet",
        "b.css",
        "--pictures",
        "notes=images",
        "--page",
        "own.html",
        "--files",
        "files",
        "--noscript",
        "Enable scripts.",
    ];
    #[test]
    fn the_arguments_make_inputs() {
        let inputs = Inputs {
            worker: None,
            name: "notes".into(),
            prefix: "/static/".into(),
            stylesheets: vec!["a.css".into(), "b.css".into()],
            pictures: vec![("notes".into(), "images".into())],
            page: Some("own.html".into()),
            files: Some("files".into()),
            noscript: "Enable scripts.".into(),
        };
        let complete: Vec<String> = REQUIRED.iter().chain(OPTIONAL).map(|word| (*word).into()).collect();
        assert_eq!(
            arguments(&complete),
            Ok(Arguments { module: "notes.wasm".into(), worker_module: None, out: "out".into(), inputs })
        );
        for tail in [
            &["--unknown", "x"][..],
            &["--name", "second"],
            &["--pictures", "images"],
            &["--pictures", "../=images"],
            &["--page"],
            &["--prefix", "/other/"],
        ] {
            let args = REQUIRED.iter().chain(tail).map(|word| (*word).into()).collect::<Vec<String>>();
            assert_eq!(arguments(&args), Err(USAGE), "{tail:?}");
        }
        for prefix in ["/", "relative/", "/missing-end", "/a//b/", "/../"] {
            let args =
                ["--module", "notes.wasm", "--name", "notes", "--prefix", prefix, "--out", "out"].map(str::to_owned);
            assert_eq!(arguments(&args), Err(USAGE), "{prefix}");
        }
        assert_eq!(arguments(&[]), Err(USAGE));
    }
    #[test]
    fn the_worker_arguments_make_inputs_together() {
        let args = REQUIRED
            .iter()
            .chain(["--worker-module", "world.wasm", "--worker-name", "world"].iter())
            .map(|word| (*word).into())
            .collect::<Vec<String>>();
        let parsed = arguments(&args).expect("worker arguments");
        assert_eq!(parsed.worker_module, Some("world.wasm".into()));
        assert_eq!(parsed.inputs.worker.as_deref(), Some("world"));
        for tail in [
            vec!["--worker-module", "world.wasm"],
            vec!["--worker-name", "world"],
            vec!["--worker-module", "world.wasm", "--worker-name", "world", "--worker-module", "other.wasm"],
            vec!["--worker-module", "world.wasm", "--worker-name", "world", "--worker-name", "other"],
        ] {
            let args = REQUIRED.iter().chain(tail.iter()).map(|word| (*word).into()).collect::<Vec<String>>();
            assert_eq!(arguments(&args), Err(USAGE), "{tail:?}");
        }
        for name in ["", ".", "..", "../world", "world/child", "world name"] {
            let args = REQUIRED
                .iter()
                .copied()
                .chain(["--worker-module", "world.wasm", "--worker-name", name])
                .map(str::to_owned)
                .collect::<Vec<_>>();
            assert_eq!(arguments(&args), Err(USAGE), "{name}");
        }
    }
}
