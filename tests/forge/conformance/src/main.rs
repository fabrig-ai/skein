use std::path::PathBuf;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    skein_fail_stop::install();
    let mut forgejo = None;
    let mut runner = None;
    let mut scratch = None;
    let mut out = None;
    let mut component = false;
    let mut create_repository = false;
    let mut admin_scopes = false;
    let mut labels = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--labels" {
            labels = true;
            continue;
        }
        if arg == "--admin-scopes" {
            admin_scopes = true;
            continue;
        }
        if arg == "--create-repository" {
            create_repository = true;
            continue;
        }
        if arg == "--component" {
            component = true;
            continue;
        }
        let value = args.next().ok_or("expected --forgejo <path> [--runner <path>] --scratch <dir> --out <dir>")?;
        match arg.as_str() {
            "--forgejo" => forgejo = Some(PathBuf::from(value)),
            "--runner" => runner = Some(PathBuf::from(value)),
            "--scratch" => scratch = Some(PathBuf::from(value)),
            "--out" => out = Some(PathBuf::from(value)),
            _ => return Err("expected --forgejo <path> [--runner <path>] --scratch <dir> --out <dir>".into()),
        }
    }
    let forgejo = forgejo.ok_or("--forgejo required")?;
    let scratch = scratch.ok_or("--scratch required")?;
    let out = out.ok_or("--out required")?;
    if labels {
        forgejo_conformance::labels::run(&forgejo, &scratch, &out)
    } else if admin_scopes {
        forgejo_conformance::admin_scopes::run(&forgejo, &scratch, &out)
    } else if create_repository {
        forgejo_conformance::create_repository::run(&forgejo, &scratch, &out)
    } else if component {
        forgejo_conformance::component::record(&forgejo, runner.as_deref(), &scratch, &out)
    } else {
        forgejo_conformance::run(&forgejo, runner.as_deref(), &scratch, &out)
    }
}
