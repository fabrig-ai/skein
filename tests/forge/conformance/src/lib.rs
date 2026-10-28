//! Records format-faithful exchanges against an isolated Forgejo (protocol/forge.md, 10).
pub mod admin_scopes;
mod cases;
mod cli;
pub mod component;
pub mod create_repository;
mod deliveries;
pub mod facts;
mod fixture;
mod hooks;
mod job_clone;
pub mod json;
pub mod labels;
pub mod mask;
mod orgs;
mod registration;
mod repo_setup;
mod repos;
mod runner;
mod search;
mod setup;
mod tokens;
mod turn;
mod users;
pub mod write;
use skein_throwaway_forgejo::{Exchange, Forgejo};
use std::{error::Error, path::Path};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
fn failure(error: &skein_throwaway_forgejo::Failure) -> Box<dyn Error> {
    format!("{error:?}").into()
}
/// # Errors
/// Returns a refused binary, provisioning, exchange or secret-scan failure.
pub fn run(binary: &Path, runner: Option<&Path>, scratch: &Path, out: &Path) -> Result<()> {
    let forgejo = Forgejo::from_path(binary).map_err(|e| format!("{e:?}"))?;
    let mut fixture = fixture::Fixture::new(&forgejo, scratch)?;
    let mut mask = mask::Mask::new(fixture.instance.port());
    let mut secrets = write::Secrets::from_instance(&fixture.instance);
    cases::record(&fixture, out, &mut mask, &mut secrets)?;
    let mut facts = facts::evaluate(&forgejo, &fixture, out)?;
    let (runner_version, actions) = runner::run(runner, &fixture, out, &mut mask, &mut secrets)?;
    facts.extend(actions);
    cases::flags(&fixture, out, &mut mask, &mut secrets)?;
    facts.extend(facts::flags(out)?);
    setup::record(&fixture, out, &mut secrets)?;
    facts.extend(setup::facts(&fixture, out)?);
    facts.extend(cli::facts(&forgejo, &mut fixture, &mut secrets)?);
    facts.extend(deliveries::record(runner, &forgejo, scratch, out, &mut secrets)?);
    facts.extend(labels::record(&fixture, out, &mut mask, &mut secrets)?);
    let lines = facts.iter().map(facts::Fact::line).collect::<String>();
    write::check("facts.txt", b"", lines.as_bytes(), &secrets)?;
    facts::write(out, &facts)?;
    std::fs::write(
        out.join("facts.txt"),
        format!("# forgejo {} sha256 {}\n# runner {runner_version}\n{lines}", forgejo.version(), forgejo.sha256()),
    )?;
    std::fs::write(
        out.join("transcripts/binary.txt"),
        format!("version {}\nsha256 {}\n", forgejo.version(), forgejo.sha256()),
    )?;
    fixture.instance.stop().map_err(|e| failure(&e))
}
fn json(exchange: &Exchange) -> Result<json::Json> {
    json::Json::parse(&exchange.body)
}
#[cfg(test)]
mod tests;
