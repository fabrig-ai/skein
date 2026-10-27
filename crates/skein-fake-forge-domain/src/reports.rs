//! Test ids and completed runs' report files (fake-forge.md, 2.3).
use crate::{
    Domain, Limits,
    actions::Run,
    api::{self, Answer, Artifact, Cue, Error, ReportFormat, What, Workflow},
    store::Repository,
};
use alloc::boxed::Box;
use skein_lib::{List, bytes::copy_of};

type Ids = Box<[Box<[u8]>]>;

pub(crate) fn normalize(values: &[Box<[u8]>], limits: &Limits) -> Result<Box<[Box<[u8]>]>, Error> {
    if values.len() > usize::try_from(limits.content_bytes).expect("id count fits") {
        return Err(Error::TooLarge);
    }
    let mut bytes = 0u32;
    for value in values {
        crate::store::fits(value, limits.content_bytes)?;
        bytes = bytes.checked_add(u32::try_from(value.len()).or(Err(Error::TooLarge))?).ok_or(Error::TooLarge)?;
    }
    if bytes > limits.content_bytes {
        return Err(Error::TooLarge);
    }
    let mut sorted = List::with_capacity(u32::try_from(values.len()).or(Err(Error::TooLarge))?);
    for value in values {
        if !value.is_empty() {
            sorted.push(copy_of(value)).expect("one id per value");
        }
    }
    let mut sorted = sorted.into_boxed();
    sorted.sort_unstable();
    let mut unique = List::with_capacity(u32::try_from(sorted.len()).expect("bounded ids"));
    for value in sorted {
        if unique.as_slice().last() != Some(&value) {
            unique.push(value).expect("at most one per id");
        }
    }
    Ok(unique.into_boxed())
}

pub(crate) fn lines(text: &[u8], limits: &Limits) -> Result<Box<[Box<[u8]>]>, Error> {
    if text.len() > usize::try_from(limits.content_bytes).expect("content bound fits") {
        return Err(Error::TooLarge);
    }
    let mut values = List::with_capacity(u32::try_from(text.len()).or(Err(Error::TooLarge))?);
    let mut start = 0usize;
    for end in 0..=text.len() {
        if end == text.len() || text.get(end) == Some(&b'\n') {
            let value = text.get(start..end).expect("line positions within text");
            let value = value.strip_suffix(b"\r").unwrap_or(value);
            if !value.is_empty() {
                values.push(copy_of(value)).expect("each id consumes a byte");
            }
            start = end.saturating_add(1);
        }
    }
    normalize(values.as_slice(), limits)
}
fn selection(workflow: &Workflow, run: &Run, limits: &Limits) -> Result<Option<Ids>, Error> {
    if let Some(name) = &workflow.select {
        for (key, value) in &run.inputs {
            if key == name {
                return Ok(Some(lines(value, limits)?));
            }
        }
    }
    Ok(None)
}
pub(crate) fn failed(
    domain: &Domain,
    workflow: &Workflow,
    run: &Run,
    limits: &Limits,
) -> Result<Box<[Box<[u8]>]>, Error> {
    let ids = match &workflow.cue {
        Some(Cue::Tests { path }) => match domain.object(run.commit).expect("run commit").tree.get(&**path) {
            Some(text) => lines(text, limits)?,
            None => Box::new([]),
        },
        Some(Cue::Message { .. } | Cue::File { .. }) | None => workflow.fails.clone(),
    };
    let Some(selected) = selection(workflow, run, limits)? else {
        return Ok(ids);
    };
    let mut failures = List::with_capacity(u32::try_from(ids.len()).expect("bounded ids"));
    let mut position = 0usize;
    for id in ids {
        for candidate in selected.get(position..).expect("selected position") {
            if candidate < &id {
                position = position.saturating_add(1);
            } else {
                break;
            }
        }
        if selected.get(position) == Some(&id) {
            failures.push(id).expect("failure subset");
        }
    }
    Ok(failures.into_boxed())
}
fn put(out: &mut List<u8>, bytes: &[u8]) -> Result<(), Error> {
    for byte in bytes {
        out.push(*byte).or(Err(Error::TooLarge))?;
    }
    Ok(())
}
fn escaped(out: &mut List<u8>, text: &[u8]) -> Result<(), Error> {
    for byte in text {
        match byte {
            b'&' => put(out, b"&amp;")?,
            b'<' => put(out, b"&lt;")?,
            b'>' => put(out, b"&gt;")?,
            b'"' => put(out, b"&quot;")?,
            b'\'' => put(out, b"&apos;")?,
            byte => put(out, core::slice::from_ref(byte))?,
        }
    }
    Ok(())
}
pub(crate) fn artifact(
    domain: &Domain,
    workflow: &Workflow,
    run: &Run,
    limits: &Limits,
) -> Result<Option<Artifact>, Error> {
    let Some(report) = &workflow.report else {
        return Ok(None);
    };
    let failed = failed(domain, workflow, run, limits)?;
    let selected = selection(workflow, run, limits)?;
    let mut out = List::with_capacity(limits.content_bytes);
    let path = match report.format {
        ReportFormat::Lines => {
            for id in &failed {
                put(&mut out, id)?;
                put(&mut out, b"\n")?;
            }
            b"failed.txt".as_slice()
        }
        ReportFormat::Junit => {
            put(&mut out, b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><testsuite>")?;
            let ids = match &selected {
                Some(ids) => ids,
                None => &failed,
            };
            let mut position = 0usize;
            for id in ids {
                for previous in failed.get(position..).expect("failed position") {
                    if previous < id {
                        position = position.saturating_add(1);
                    } else {
                        break;
                    }
                }
                put(&mut out, b"<testcase name=\"")?;
                escaped(&mut out, id)?;
                if failed.get(position) == Some(id) {
                    put(&mut out, b"\"><failure/></testcase>")?;
                } else {
                    put(&mut out, b"\"/>")?;
                }
            }
            put(&mut out, b"</testsuite>")?;
            b"report.xml".as_slice()
        }
    };
    Ok(Some(Artifact {
        name: report.artifact.clone(),
        path: copy_of(path),
        bytes: out.into_boxed(),
        deflate: report.deflate,
    }))
}

pub(crate) fn commits(
    domain: &Domain,
    repository: &Repository,
    limits: &Limits,
    before: u64,
    after: u64,
) -> Result<Answer, Error> {
    if !repository.has.contains(&before) || !repository.has.contains(&after) {
        return Err(Error::Missing(What::Commit));
    }
    let old = crate::git::ancestors(domain, before);
    let new = crate::git::ancestors(domain, after);
    let mut commits = List::with_capacity(limits.commits);
    for &id in &new {
        if old.contains(&id) {
            continue;
        }
        let object = domain.object(id).expect("commit history");
        let mut parents = List::with_capacity(2);
        for parent in [object.parent, object.merge_parent].into_iter().flatten() {
            parents.push(parent).expect("two parents");
        }
        commits
            .push(api::CommitMessage { id, parents: parents.into_boxed(), message: object.message.clone() })
            .expect("bounded history");
    }
    Ok(Answer::Commits { commits: commits.into_boxed() })
}

/// Every latest job attempt completed without a runner fault.
pub(crate) fn reportable(kept: &crate::actions::Kept) -> bool {
    if kept.run.state != crate::actions::RunState::Completed {
        return false;
    }
    for job in &kept.jobs {
        for step in &job.steps {
            if step.status == api::StepStatus::Cancelled
                || (step.number == 0 && step.status == api::StepStatus::Failure)
            {
                return false;
            }
        }
    }
    true
}
