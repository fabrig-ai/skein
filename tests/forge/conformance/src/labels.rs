//! Label writes recorded against an isolated repository on the pinned Forgejo.
use crate::{
    Result,
    cases::Recorder,
    facts::{Fact, add, case},
    fixture::Fixture,
    mask::Mask,
    write::Secrets,
};
use skein_throwaway_forgejo::Forgejo;
use std::path::Path;

const R: &str = "/api/v1/repos/fixture/labels";
const FINDING: &str = r##"{"name":"finding","color":"#12abef"}"##;
const SEVERITY: &str = r##"{"name":"severity:high","color":"#ff0000"}"##;

pub(crate) fn record(f: &Fixture, out: &Path, mask: &mut Mask, secrets: &mut Secrets) -> Result<Vec<Fact>> {
    f.api("POST", "/api/v1/user/repos", br#"{"name":"labels","auto_init":true,"default_branch":"main"}"#)?;
    let mut r = Recorder { f, out, mask, secrets };
    let token = Some(&f.owner.token);
    let finding = r.send("label-created", "POST", &format!("{R}/labels"), Some(FINDING), token, None)?;
    let finding = crate::json(&finding)?.root().get("id").u64().ok_or("label id missing")?;
    r.send("label-created-again", "POST", &format!("{R}/labels"), Some(FINDING), token, None)?;
    let severity = r.send("label-severity-created", "POST", &format!("{R}/labels"), Some(SEVERITY), token, None)?;
    let severity = crate::json(&severity)?.root().get("id").u64().ok_or("severity id missing")?;
    r.send("labels-listed", "GET", &format!("{R}/labels?page=1&limit=50"), None, token, None)?;
    let body = format!(r#"{{"title":"finding","body":"labelled issue","labels":[{finding},{severity}]}}"#);
    let issue = r.send("issue-created-with-labels", "POST", &format!("{R}/issues"), Some(&body), token, None)?;
    let issue = crate::json(&issue)?.root().get("number").u64().ok_or("issue number missing")?;
    r.send(
        "issue-labels-replaced",
        "PUT",
        &format!("{R}/issues/{issue}/labels"),
        Some(&format!(r#"{{"labels":[{finding}]}}"#)),
        token,
        None,
    )?;
    r.send("issue-labels-after-replacement", "GET", &format!("{R}/issues/{issue}"), None, token, None)?;
    // This id is beyond the ids returned by the isolated repository's label creations.
    let missing = severity + finding + 1;
    let body = format!(r#"{{"labels":[{missing}]}}"#);
    r.send("issue-label-unknown", "PUT", &format!("{R}/issues/{issue}/labels"), Some(&body), token, None)?;
    let body = format!(r#"{{"title":"missing label","body":"unknown","labels":[{missing}]}}"#);
    r.send("issue-created-label-unknown", "POST", &format!("{R}/issues"), Some(&body), token, None)?;
    facts(out)
}

pub(crate) fn facts(out: &Path) -> Result<Vec<Fact>> {
    let mut facts = Vec::new();
    for (name, capture, status) in [
        ("label-created-is-201", "label-created", 201),
        ("label-created-twice-is-409", "label-created-again", 409),
        ("label-list-is-200", "labels-listed", 200),
        ("issue-created-with-labels-is-201", "issue-created-with-labels", 201),
        ("issue-labels-replaced-is-200", "issue-labels-replaced", 200),
        ("issue-unknown-label-is-refused", "issue-label-unknown", 422),
        ("issue-created-unknown-label-is-refused", "issue-created-label-unknown", 422),
    ] {
        let c = case(out, capture)?;
        add(
            &mut facts,
            name,
            c.status == status,
            &format!(
                "HTTP {}: {}",
                c.status,
                crate::facts::diagnostic(&std::fs::read(out.join("transcripts").join(capture).join("response"))?)
            ),
        );
    }
    let created = case(out, "issue-created-with-labels")?;
    let replaced = case(out, "issue-labels-after-replacement")?;
    add(
        &mut facts,
        "issue-created-keeps-two-labels",
        created.body.root().get("labels").array().is_some_and(|a| a.len() == 2),
        "issue did not keep two labels",
    );
    add(
        &mut facts,
        "issue-labels-replacement-keeps-one",
        replaced.body.root().get("labels").array().is_some_and(|a| a.len() == 1),
        "replacement did not keep one label",
    );
    Ok(facts)
}

/// # Errors
/// Returns a provisioning, recording, or secret scan failure.
pub fn run(binary: &Path, scratch: &Path, out: &Path) -> Result<()> {
    let forgejo = Forgejo::from_path(binary).map_err(|e| format!("{e:?}"))?;
    let fixture = Fixture::bare(&forgejo, scratch, &skein_throwaway_forgejo::Settings::small())?;
    let mut mask = Mask::new(fixture.instance.port());
    let mut secrets = Secrets::from_instance(&fixture.instance);
    let facts = record(&fixture, out, &mut mask, &mut secrets)?;
    crate::facts::write(out, &facts)?;
    fixture.instance.stop().map_err(|e| crate::failure(&e))?;
    if !matches_pin(&facts) {
        return Err("label conformance failed; see facts.txt".into());
    }
    println!("labels: {} facts match the pin", facts.len());
    Ok(())
}

fn matches_pin(facts: &[Fact]) -> bool {
    // The task accepts these three negative facts as the pinned API's behavior.
    const FACTS: usize = 9;
    if facts.len() != FACTS {
        return false;
    }
    facts.iter().all(|fact| {
        let negative =
            ["label-created-twice-is-409", "issue-unknown-label-is-refused", "issue-created-unknown-label-is-refused"]
                .contains(&fact.name);
        match fact.outcome {
            crate::facts::Outcome::Holds | crate::facts::Outcome::Observed(_) => !negative,
            crate::facts::Outcome::Fails(_) => negative,
            crate::facts::Outcome::Unchecked(_) => false,
        }
    })
}
#[cfg(test)]
mod tests {
    #[test]
    fn label_recording_replays_the_accepted_pin_facts() {
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("labels");
        let facts = super::facts(&out).expect("recorded label facts");
        assert!(super::matches_pin(&facts));
    }
}
