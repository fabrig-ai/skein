//! Recorded turn prerequisites on the pinned Forgejo (forge.md, 9).
use crate::{
    Result,
    cases::Recorder,
    facts::{Fact, add},
    fixture::Fixture,
    json::{At, Json},
    mask::Mask,
    write::Secrets,
};
use skein_throwaway_forgejo::Exchange;
use std::{path::Path, time::Duration};

mod actions;
mod git;
const REPO: &str = "/api/v1/repos/fixture/turn";

pub(crate) struct Probe<'a> {
    recorder: Recorder<'a>,
    facts: Vec<Fact>,
}
impl Probe<'_> {
    fn send(&mut self, name: &str, method: &str, target: &str, body: Option<&str>) -> Result<Exchange> {
        self.recorder.send(name, method, target, body, Some(&self.recorder.f.owner.token), None)
    }
    fn get(&mut self, name: &str, target: &str) -> Result<Json> {
        let response = self.send(name, "GET", target, None)?;
        if response.status != 200 {
            return Err(format!("{name}: HTTP {}", response.status).into());
        }
        crate::json(&response)
    }
    fn fact(&mut self, name: &'static str, holds: bool, response: &Exchange) {
        // The recorder has scanned these bytes before any diagnostic is retained.
        add(
            &mut self.facts,
            name,
            holds,
            &format!("HTTP {}: {}", response.status, crate::facts::diagnostic(&response.body)),
        );
    }
    fn check(&mut self, name: &'static str, holds: bool, detail: &str) {
        add(&mut self.facts, name, holds, detail);
    }
    fn head(&mut self, name: &str, branch: &str) -> Result<String> {
        text(self.get(name, &format!("{REPO}/branches/{branch}"))?.root().get("commit").get("id"))
    }
}
fn text(at: At<'_>) -> Result<String> {
    Ok(at.str().ok_or("expected string in turn fixture")?.to_owned())
}
fn number(at: At<'_>) -> Result<u64> {
    Ok(at.u64().ok_or("expected number in turn fixture")?)
}
fn rows<'a>(json: &'a Json, field: &str) -> Result<Vec<At<'a>>> {
    Ok(json.root().get(field).array().ok_or("expected rows in turn fixture")?.to_vec())
}
fn pause() {
    std::thread::sleep(Duration::from_millis(100));
}
pub(crate) fn record(f: &Fixture, out: &Path, mask: &mut Mask, secrets: &mut Secrets) -> Result<Vec<Fact>> {
    f.api("POST", "/api/v1/user/repos", br#"{"name":"turn","auto_init":true,"default_branch":"main"}"#)?;
    let mut p = Probe { recorder: Recorder { f, out, mask, secrets }, facts: Vec::new() };
    git::record(&mut p)?;
    actions::record(&mut p)?;
    Ok(p.facts)
}
