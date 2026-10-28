use super::{
    Answer, Route, Unrouted,
    items::{number_value, page, query_value},
};
use crate::forgejo::{
    document::{self, Job, Run, Step},
    id,
    limits::Limits,
    path,
    request::{Method, Repo, Request},
};
use skein_json::writer::Encoder;
pub(super) fn route(method: Method, bytes: &[u8], limits: &Limits) -> Result<Route, Unrouted> {
    let Ok(target) = path::read(bytes, limits) else {
        return Err(Unrouted::Malformed);
    };
    let [api, v1, repos, owner, name, actions, tail @ ..] = target.segments.as_ref() else {
        return Err(Unrouted::NotFound);
    };
    if &**api != b"api" || &**v1 != b"v1" || &**repos != b"repos" || &**actions != b"actions" {
        return Err(Unrouted::NotFound);
    }
    let at = Repo { owner: owner.clone(), name: name.clone() };
    let request = match tail {
        [runs] if &**runs == b"runs" => {
            let head = query_value(&target.query, b"head_sha").ok_or(Unrouted::Malformed)?;
            if let Some(workflow) = query_value(&target.query, b"workflow") {
                if method != Method::Get {
                    return Err(Unrouted::Method);
                }
                return Ok(Route::Ready(Request::WorkflowRuns {
                    at,
                    workflow,
                    head_sha: id::read(&head).ok_or(Unrouted::Malformed)?,
                    page: page(&target.query)?,
                }));
            }
            let number = query_value(&target.query, b"run_number").ok_or(Unrouted::Malformed)?;
            Request::Runs {
                at,
                head_sha: id::read(&head).ok_or(Unrouted::Malformed)?,
                run_number: number_value(&number)?,
                page: page(&target.query)?,
            }
        }
        [runs, run] if &**runs == b"runs" => Request::Run { at, run: number_value(run)? },
        [runs, run, jobs] if &**runs == b"runs" && &**jobs == b"jobs" => Request::Jobs { at, run: number_value(run)? },
        [jobs, job] if &**jobs == b"jobs" => Request::Job { at, job: number_value(job)? },
        [jobs, job, logs] if &**jobs == b"jobs" && &**logs == b"logs" => {
            let attempt = query_value(&target.query, b"attempt").ok_or(Unrouted::Malformed)?;
            let Ok(attempt) = u32::try_from(number_value(&attempt)?) else {
                return Err(Unrouted::Malformed);
            };
            let step = match query_value(&target.query, b"step") {
                Some(text) => Some(match u32::try_from(number_value(&text)?) {
                    Ok(number) => number,
                    Err(_) => return Err(Unrouted::Malformed),
                }),
                None => None,
            };
            Request::JobLog { at, job: number_value(job)?, attempt, step }
        }
        _ => return Err(Unrouted::NotFound),
    };
    match method {
        Method::Get => Ok(Route::Ready(request)),
        Method::Post | Method::Put | Method::Patch | Method::Delete => Err(Unrouted::Method),
    }
}
fn run(json: &mut Encoder, value: &Run) {
    json.object_start();
    json.key(b"id");
    json.unsigned(value.id);
    json.key(b"index_in_repo");
    json.unsigned(value.index_in_repo);
    json.key(b"commit_sha");
    json.string(&id::write(&value.commit_sha));
    if !value.workflow.is_empty() {
        json.key(b"workflow_id");
        json.string(&value.workflow);
    }
    if !value.event.is_empty() {
        json.key(b"event");
        json.string(if value.event.as_ref() == b"workflow_dispatch" { b"" } else { &value.event });
        json.key(b"trigger_event");
        json.string(&value.event);
    }
    if value.trigger_actor != 0 {
        json.key(b"trigger_user");
        json.object_start();
        json.key(b"id");
        json.unsigned(value.trigger_actor);
        json.object_end();
    }
    if value.status != document::StepStatus::Unknown {
        json.key(b"status");
        json.string(document::step_status_text(value.status));
    }
    if !value.html_url.is_empty() {
        json.key(b"html_url");
        json.string(&value.html_url);
    }
    if !value.workflow.is_empty() || value.created_at != skein_lib::Wall::EPOCH {
        json.key(b"created");
        json.string(&crate::forgejo::time::write(value.created_at));
    }
    json.object_end();
}
pub(super) fn write(json: &mut Encoder, answer: &Answer) {
    match answer {
        Answer::Run(value) => run(json, value),
        Answer::Runs { total_count, runs } => {
            json.object_start();
            json.key(b"total_count");
            json.unsigned(*total_count);
            json.key(b"workflow_runs");
            json.array_start();
            for value in runs {
                run(json, value);
            }
            json.array_end();
            json.object_end();
        }
        Answer::Job { job: value, steps } => job(json, value, steps),
        Answer::Jobs(values) => {
            json.array_start();
            for value in values {
                job(json, value, &[]);
            }
            json.array_end();
        }
        Answer::Tree { .. }
        | Answer::Error(_)
        | Answer::Comment(_)
        | Answer::Status(_)
        | Answer::Review(_)
        | Answer::Token(_)
        | Answer::Tokens(_)
        | Answer::User(_)
        | Answer::ApiSettings(_)
        | Answer::Repository(_)
        | Answer::Reachable(_)
        | Answer::RepoSearch { .. }
        | Answer::Search { .. }
        | Answer::Issues(_)
        | Answer::Issue(_)
        | Answer::Comments(_)
        | Answer::Pull(_)
        | Answer::Org(_)
        | Answer::CreatedFile { .. }
        | Answer::CreatedRepository(_)
        | Answer::Team(_)
        | Answer::Labels(_)
        | Answer::Label(_)
        | Answer::Teams(_)
        | Answer::Hook(_)
        | Answer::Hooks(_)
        | Answer::Members(_)
        | Answer::CombinedStatus { .. }
        | Answer::Reviews(_)
        | Answer::ReviewComments(_)
        | Answer::Branch(_)
        | Answer::BranchRefs(_)
        | Answer::Branches(_)
        | Answer::Protection(_)
        | Answer::Protections(_)
        | Answer::Collaborators(_)
        | Answer::Permission(_)
        | Answer::Pulls(_)
        | Answer::Entries(_)
        | Answer::Files(_)
        | Answer::Compare { .. } => unreachable!("action answer selection"),
    }
}

fn job(json: &mut Encoder, value: &Job, steps: &[Step]) {
    json.object_start();
    json.key(b"id");
    json.unsigned(value.id);
    json.key(b"run_id");
    json.unsigned(value.run_id);
    json.key(b"attempt");
    json.unsigned(u64::from(value.attempt));
    if !value.name.is_empty() {
        json.key(b"name");
        json.string(&value.name);
    }
    if value.status != document::StepStatus::Unknown {
        json.key(b"status");
        json.string(document::step_status_text(value.status));
    }
    if !steps.is_empty() {
        json.key(b"steps");
        json.array_start();
        for step in steps {
            json.object_start();
            json.key(b"number");
            json.unsigned(u64::from(step.number));
            json.key(b"name");
            json.string(&step.name);
            json.key(b"status");
            json.string(document::step_status_text(step.status));
            json.object_end();
        }
        json.array_end();
    }
    json.object_end();
}
