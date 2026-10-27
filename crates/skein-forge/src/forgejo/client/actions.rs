use crate::forgejo::{
    decode::{
        Malformed, Step,
        walk::{Field, Kind, Move, Read, Value, Walk},
    },
    document::{self, Body, Document, Job, Row, Run, StepStatus},
};
use alloc::boxed::Box;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ActionKind {
    Artifacts,
    Artifact,
    Run,
    Actor,
    Runs,
    Listing,
    Job,
    Jobs,
    Step,
    Steps,
}
const fn field(key: &'static [u8], read: Read) -> Field {
    Field { key, read, required: true }
}
pub(crate) fn fields(kind: ActionKind) -> &'static [Field] {
    match kind {
        ActionKind::Artifact => {
            const {
                &[
                    field(b"id", Read::Number),
                    field(b"name", Read::Text),
                    field(b"run_id", Read::Number),
                    field(b"size_in_bytes", Read::Number),
                ]
            }
        }
        ActionKind::Run => {
            const {
                &[
                    field(b"id", Read::Number),
                    field(b"index_in_repo", Read::Number),
                    field(b"commit_sha", Read::Id { empty: false }),
                    Field { key: b"workflow_id", read: Read::Text, required: false },
                    Field { key: b"event", read: Read::Text, required: false },
                    Field { key: b"trigger_event", read: Read::Text, required: false },
                    Field {
                        key: b"trigger_actor",
                        read: Read::Object(Kind::Action(ActionKind::Actor)),
                        required: false,
                    },
                    Field {
                        key: b"trigger_user",
                        read: Read::Object(Kind::Action(ActionKind::Actor)),
                        required: false,
                    },
                    Field { key: b"created", read: Read::Time, required: false },
                    Field { key: b"status", read: Read::Text, required: false },
                    Field { key: b"html_url", read: Read::Text, required: false },
                    Field { key: b"created_at", read: Read::Time, required: false },
                ]
            }
        }
        ActionKind::Job => {
            const {
                &[
                    field(b"id", Read::Number),
                    field(b"run_id", Read::Number),
                    field(b"attempt", Read::Number),
                    Field { key: b"name", read: Read::Text, required: false },
                    Field { key: b"status", read: Read::Text, required: false },
                    Field { key: b"steps", read: Read::Array(Kind::Action(ActionKind::Steps)), required: false },
                ]
            }
        }
        ActionKind::Listing => {
            const {
                &[
                    field(b"total_count", Read::Number),
                    field(b"workflow_runs", Read::Array(Kind::Action(ActionKind::Runs))),
                ]
            }
        }
        ActionKind::Step => {
            const { &[field(b"number", Read::Number), field(b"name", Read::Body), field(b"status", Read::Text)] }
        }
        ActionKind::Actor => const { &[field(b"id", Read::Number)] },
        ActionKind::Artifacts | ActionKind::Runs | ActionKind::Jobs | ActionKind::Steps => &[],
    }
}
pub(crate) fn element(kind: ActionKind) -> Option<Kind> {
    match kind {
        ActionKind::Artifacts => Some(Kind::Action(ActionKind::Artifact)),
        ActionKind::Runs => Some(Kind::Action(ActionKind::Run)),
        ActionKind::Jobs => Some(Kind::Action(ActionKind::Job)),
        ActionKind::Steps => Some(Kind::Action(ActionKind::Step)),
        ActionKind::Artifact
        | ActionKind::Actor
        | ActionKind::Run
        | ActionKind::Job
        | ActionKind::Step
        | ActionKind::Listing => None,
    }
}
#[derive(Debug)]
pub(super) struct Actions {
    root: ActionKind,
    run: Run,
    artifact: document::Artifact,
    job: Job,
    step: document::Step,
    total_count: u64,
    actor_seen: bool,
    created_seen: bool,
    trigger_event_seen: bool,
}
impl Actions {
    pub(super) fn new(root: ActionKind) -> Self {
        Self {
            root,
            run: document::run(),
            artifact: artifact(),
            job: document::job(),
            step: document::Step { number: 0, name: Box::from([]), status: StepStatus::Unknown },
            total_count: 0,
            actor_seen: false,
            created_seen: false,
            trigger_event_seen: false,
        }
    }
    pub(super) fn step(&mut self, movement: Move, walk: &Walk) -> Step {
        let result = match movement {
            Move::Demand(demand) => return Step::Demand(demand),
            Move::Scalar { kind, key, value } => self.scalar(kind, key, value),
            Move::End(Kind::Action(ActionKind::Artifact)) => {
                return Step::Row(
                    Row::Artifact(core::mem::replace(&mut self.artifact, artifact())),
                    skein_json::tokenizer::Request::Next,
                );
            }
            Move::End(Kind::Action(ActionKind::Artifacts)) => Ok(()),
            Move::End(Kind::Action(ActionKind::Run)) => {
                if self.root == ActionKind::Listing {
                    self.actor_seen = false;
                    self.created_seen = false;
                    self.trigger_event_seen = false;
                    return Step::Row(
                        Row::Run(core::mem::replace(&mut self.run, document::run())),
                        skein_json::tokenizer::Request::Next,
                    );
                }
                Ok(())
            }
            Move::End(Kind::Action(ActionKind::Job)) => {
                if self.root == ActionKind::Jobs {
                    return Step::Row(
                        Row::Job(core::mem::replace(&mut self.job, document::job())),
                        skein_json::tokenizer::Request::Next,
                    );
                }
                Ok(())
            }
            Move::End(Kind::Action(ActionKind::Step)) => {
                let step = core::mem::replace(
                    &mut self.step,
                    document::Step { number: 0, name: Box::from([]), status: StepStatus::Unknown },
                );
                if self.root == ActionKind::Job {
                    return Step::Row(Row::Step(step), skein_json::tokenizer::Request::Next);
                }
                Ok(())
            }
            Move::End(Kind::Action(ActionKind::Runs | ActionKind::Jobs | ActionKind::Listing | ActionKind::Steps)) => {
                Ok(())
            }
            Move::End(Kind::Action(ActionKind::Actor)) => {
                if self.actor_seen {
                    Err(Malformed::Shape { field: b"trigger_actor" })
                } else {
                    self.actor_seen = true;
                    Ok(())
                }
            }
            Move::End(
                Kind::User
                | Kind::Reachable(_)
                | Kind::Error
                | Kind::Write(_)
                | Kind::Settings
                | Kind::Repository
                | Kind::Search
                | Kind::Setup(_)
                | Kind::Users
                | Kind::Item(_)
                | Kind::Review(_)
                | Kind::Delivery(_)
                | Kind::Branch(_)
                | Kind::File(_),
            ) => unreachable!("action tables"),
            Move::Done => {
                return Step::Done(match self.root {
                    ActionKind::Run => Document::Run(core::mem::replace(&mut self.run, document::run())),
                    ActionKind::Listing => Document::Runs { total_count: self.total_count },
                    ActionKind::Artifacts | ActionKind::Jobs => Document::Listed { skipped: walk.skipped() },
                    ActionKind::Job => Document::Job(core::mem::replace(&mut self.job, document::job())),
                    ActionKind::Artifact
                    | ActionKind::Actor
                    | ActionKind::Step
                    | ActionKind::Steps
                    | ActionKind::Runs => {
                        unreachable!("action root selection")
                    }
                });
            }
        };
        match result {
            Ok(()) => Step::Demand(skein_json::tokenizer::Request::Next),
            Err(error) => Step::Malformed(error),
        }
    }
    fn scalar(&mut self, kind: Kind, key: &'static [u8], value: Value) -> Result<(), Malformed> {
        match value {
            Value::Input(_) => unreachable!("dispatch input table"),
            Value::Number(n) => match key {
                b"id" => {
                    if kind == Kind::Action(ActionKind::Artifact) {
                        self.artifact.id = n;
                    } else if kind == Kind::Action(ActionKind::Actor) {
                        self.run.trigger_actor = n;
                    } else if kind == Kind::Action(ActionKind::Run) {
                        self.run.id = n;
                    } else {
                        self.job.id = n;
                    }
                }
                b"index_in_repo" => self.run.index_in_repo = n,
                b"run_id" => {
                    if kind == Kind::Action(ActionKind::Artifact) {
                        self.artifact.run_id = n;
                    } else {
                        self.job.run_id = n;
                    }
                }
                b"size_in_bytes" => self.artifact.size_in_bytes = n,
                b"attempt" | b"number" => {
                    let Ok(n) = u32::try_from(n) else {
                        return Err(Malformed::Range { field: key });
                    };
                    if key == b"attempt" {
                        self.job.attempt = n;
                    } else {
                        self.step.number = n;
                    }
                }
                b"total_count" => self.total_count = n,
                _ => unreachable!("action number table"),
            },
            Value::Id(id) => self.run.commit_sha = id.expect("required run commit"),
            Value::Text(text) => match key {
                b"workflow_id" => self.run.workflow = text,
                b"event" => {
                    if !self.trigger_event_seen {
                        self.run.event = text;
                    }
                }
                b"trigger_event" => {
                    self.trigger_event_seen = true;
                    self.run.event = text;
                }
                b"html_url" => self.run.html_url = text,
                b"name" => {
                    if kind == Kind::Action(ActionKind::Artifact) {
                        self.artifact.name = text;
                    } else {
                        self.job.name = text;
                    }
                }
                b"status" => {
                    let status = document::step_status(&text).ok_or(Malformed::Value { field: key })?;
                    if kind == Kind::Action(ActionKind::Run) {
                        self.run.status = status;
                    } else if kind == Kind::Action(ActionKind::Job) {
                        self.job.status = status;
                    } else {
                        self.step.status = status;
                    }
                }
                _ => unreachable!("action text table"),
            },
            Value::Body(body) => {
                self.step.name = match body {
                    Body::Kept(name) => name,
                    Body::Long(_) => Box::from([]),
                }
            }
            Value::Time(time) => {
                if self.created_seen {
                    return Err(Malformed::Shape { field: key });
                }
                self.created_seen = true;
                self.run.created_at = time;
            }
            Value::Boolean(_) => unreachable!("action scalar table"),
        }
        Ok(())
    }
}

fn artifact() -> document::Artifact {
    document::Artifact { id: 0, name: Box::new([]), run_id: 0, size_in_bytes: 0 }
}
