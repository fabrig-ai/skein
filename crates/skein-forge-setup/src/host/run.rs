//! One bounded plan and its outstanding owner or forge need (6.3).
use super::{
    Answer, Forge, Kind, Need, Plan,
    states::{Purpose, States},
};
use crate::{
    adopt::{self, Adopt},
    call::{Auth, Call},
    check::{self, Check},
    create::{self, Create},
    github::{
        accept::{self, Accept},
        convert::{self, Convert},
        install::{self, Install},
        manifest::{self, Manifest},
        paste::{self, Paste},
        reconcile as github_reconcile,
    },
    given::Given,
    item::{self, Bounds, Item},
    reachable::{self, Reachable},
    reconcile::{self, Event, Failure, Reconcile, Reply, Stage, Why},
    take_over::{self, TakeOver},
};
use alloc::boxed::Box;
use skein_forge::forgejo::request::Request;
use skein_lib::{CryptoRng, Duration, Env, Queue, Secret, Time, Token};
/// The forge's settings given, with the plan's adopted repositories.
pub(crate) fn given_for(forge: &Given, adopted: &[u64]) -> Given {
    let mut given = forge.clone();
    given.adopted = adopted.into();
    given
}
/// A Forgejo plan's machine; GitHub's plans are refused.
fn forgejo_machine(forge: &Forge, given: &Given, plan: Plan, bounds: &Bounds) -> Result<Machine, Failure> {
    Ok(match plan {
        Plan::Paste { .. }
        | Plan::Manifest { .. }
        | Plan::Convert { .. }
        | Plan::Install { .. }
        | Plan::Accept { .. } => {
            return Err(super::about(forge, &plan, Why::Unexpected));
        }
        Plan::Reconcile { adopted, .. } => Machine::Reconcile(Box::new(
            Reconcile::new(&given_for(given, &adopted)).expect("the owner checks a plan's adopted repositories"),
        )),
        Plan::Check { forge } => Machine::Check(Check::new(forge)),
        Plan::TakeOver { admin_token, .. } => {
            let taken = if item::token(admin_token.expose(), bounds.token_bytes).is_some() {
                admin_token
            } else {
                Secret::new(Box::new([]))
            };
            Machine::TakeOver(TakeOver::new(given, taken).expect("host checks the given"))
        }
        Plan::Create { org, repository, files, .. } => Machine::Create(Box::new(
            Create::new(given, org, repository, files).expect("the owner checks the creation plan"),
        )),
        Plan::Adopt { repository, .. } => Machine::Adopt(
            Adopt::new(given, repository.get()).expect("host checks the given and the repository is nonzero"),
        ),
        Plan::Reachable { installation, page, .. } => Machine::Reachable(
            Reachable::new(given, installation, page.get()).expect("host checks the given and the page is nonzero"),
        ),
    })
}
/// One machine per plan, sharing the host's need and connection loop.
#[derive(Debug)]
pub(crate) enum Machine {
    Manifest(Manifest),
    Convert(Convert),
    Paste(Box<Paste>),
    Install(Install),
    Accept(Accept),
    Reconcile(Box<Reconcile>),
    GitHubReconcile(Box<github_reconcile::Reconcile>),
    Check(Check),
    TakeOver(TakeOver),
    Adopt(Adopt),
    Create(Box<Create>),
    Reachable(Reachable),
}
impl Machine {
    pub(crate) fn step(&mut self, event: Event, out: &mut Queue<reconcile::Need>) {
        match self {
            Self::Manifest(machine) => manifest::step(machine, event, out),
            Self::Convert(machine) => convert::step(machine, event, out),
            Self::Paste(machine) => paste::step(machine, event, out),
            Self::Install(machine) => install::step(machine, event, out),
            Self::Accept(machine) => accept::step(machine, event, out),
            Self::Reconcile(machine) => reconcile::step(machine, event, out),
            Self::GitHubReconcile(machine) => github_reconcile::step(machine, event, out),
            Self::Check(machine) => check::step(machine, event, out),
            Self::TakeOver(machine) => take_over::step(machine, event, out),
            Self::Adopt(machine) => adopt::step(machine, event, out),
            Self::Create(machine) => create::step(machine, event, out),
            Self::Reachable(machine) => reachable::step(machine, event, out),
        }
    }
    /// The host halts only machines that have received Begin.
    pub(crate) fn halt(&mut self, why: Why, out: &mut Queue<reconcile::Need>) {
        match self {
            Self::Reconcile(machine) => reconcile::halt(machine, why, out),
            Self::Create(machine) => create::halt(machine, why, out),
            Self::Convert(machine) => convert::halt(machine, why, out),
            Self::Paste(machine) => paste::halt(machine, why, out),
            Self::GitHubReconcile(machine) => github_reconcile::halt(machine, why, out),
            Self::Manifest(_)
            | Self::Install(_)
            | Self::Accept(_)
            | Self::Check(_)
            | Self::TakeOver(_)
            | Self::Adopt(_)
            | Self::Reachable(_) => {
                self.halt_begun(why, out);
            }
        }
    }
    fn halt_begun(&mut self, why: Why, out: &mut Queue<reconcile::Need>) {
        let mut stopped = Queue::uncapped();
        self.step(Event::Begin, &mut stopped);
        match stopped.pop() {
            Some(reconcile::Need::Stop(mut failure)) => {
                failure.why = why;
                out.push(reconcile::Need::Stop(failure));
            }
            Some(
                reconcile::Need::Send { .. }
                | reconcile::Need::Read(_)
                | reconcile::Need::Write { .. }
                | reconcile::Need::Draw(_)
                | reconcile::Need::Done(_),
            )
            | None => (),
        }
    }
}
#[derive(Debug)]
pub(super) enum Owner {
    Read(Item),
    Write,
}
#[derive(Debug)]
pub(super) struct Send {
    pub call: Call,
    pub auth: Auth,
}
/// What a Forgejo forge sends: the request and the token it carries.
pub(crate) fn forgejo_pair(call: Call, auth: Auth) -> Result<(Request, Secret), Why> {
    match call {
        Call::Forgejo(request) => match auth {
            Auth::Token(token) => Ok((request, token)),
            Auth::App { .. } | Auth::Bearer(_) | Auth::None => Err(Why::Unexpected),
        },
        Call::GitHub(_) => Err(Why::Unexpected),
    }
}
#[derive(Debug)]
pub(super) struct Run {
    pub bounds: Bounds,
    pub issued_state: Option<(Purpose, Box<[u8]>)>,
    pub forge: u16,
    pub writes: bool,
    pub machine: Machine,
    pub deadline: Time,
    pub pending: Option<reconcile::Need>,
    pub owner: Option<Owner>,
    pub send: Option<Send>,
    pub connection: Option<Token>,
    pub retry: bool,
    pub retry_at: Option<Time>,
    pub backoff: Duration,
    pub backoff_max: Duration,
    pub answered: bool,
}
impl Run {
    pub fn new(
        forge: &Forge,
        plan: Plan,
        bounds: Bounds,
        env: &Env<super::Limits>,
        crypto: &mut CryptoRng,
        states: &mut States,
        out: &mut Queue<reconcile::Need>,
    ) -> Result<Self, Failure> {
        let now = env.now;
        let writes = plan.writes();
        let mut issued_state = None;
        let machine = match &forge.kind {
            Kind::Forgejo(given) => forgejo_machine(forge, given, plan, &bounds)?,
            Kind::GitHub(given) => match plan {
                Plan::Manifest { ref target, .. } => {
                    let state = issue_state(given, Purpose::Convert, now, crypto, states, b"manifest")?;
                    let Ok(machine) = Manifest::new(given, target, &state) else {
                        let _taken = states.take(given.forge, Purpose::Convert, &state, now);
                        return Err(super::about(forge, &plan, Why::Unexpected));
                    };
                    issued_state = Some((Purpose::Convert, state));
                    Machine::Manifest(machine)
                }
                Plan::Convert { ref code, ref state, .. } => {
                    states.take(given.forge, Purpose::Convert, state, now)?;
                    let Ok(machine) = Convert::new(given, code.expose()) else {
                        return Err(super::about(forge, &plan, Why::Unexpected));
                    };
                    // The owner answers the browser only after all four Written replies (5.2).
                    Machine::Convert(machine)
                }
                Plan::Paste { app, .. } => {
                    Machine::Paste(Box::new(Paste::new(given, app).expect("host checks the given")))
                }
                Plan::Install { .. } => {
                    let state = issue_state(given, Purpose::Accept, now, crypto, states, b"install")?;
                    let Ok(machine) = Install::new(given, &state) else {
                        let _taken = states.take(given.forge, Purpose::Accept, &state, now);
                        return Err(super::about(forge, &plan, Why::Unexpected));
                    };
                    issued_state = Some((Purpose::Accept, state));
                    Machine::Install(machine)
                }
                Plan::Accept { installation, ref state, .. } => {
                    if let Some(state) = state {
                        states.take(given.forge, Purpose::Accept, state, now)?;
                    }
                    Machine::Accept(Accept::new(given, installation).expect("host checks the given"))
                }
                Plan::Reachable { installation, page, .. } => {
                    let Some(id) = installation else {
                        return Err(Failure {
                            forge: given.forge,
                            step: Stage::Installation,
                            subject: b"none".as_slice().into(),
                            why: Why::Unexpected,
                        });
                    };
                    match Reachable::github(given, id, page.get()) {
                        Ok(machine) => Machine::Reachable(machine),
                        Err(_) => return Err(super::about(forge, &plan, Why::Unexpected)),
                    }
                }
                Plan::Reconcile { ref adopted, .. } => {
                    if !adopted.is_empty() {
                        return Err(super::about(forge, &plan, Why::Unexpected));
                    }
                    Machine::GitHubReconcile(Box::new(
                        github_reconcile::Reconcile::new(given, env.wall).expect("host checks the given"),
                    ))
                }
                Plan::Check { .. } | Plan::TakeOver { .. } | Plan::Adopt { .. } | Plan::Create { .. } => {
                    return Err(super::about(forge, &plan, Why::Unexpected));
                }
            },
        };
        let times = forge.times();
        let mut run = Self {
            bounds,
            issued_state,
            forge: forge.number(),
            writes,
            machine,
            deadline: now.saturating_add(times.deadline),
            pending: None,
            owner: None,
            send: None,
            connection: None,
            retry: false,
            retry_at: None,
            backoff: times.backoff,
            backoff_max: times.backoff_max,
            answered: false,
        };
        run.step(Event::Begin, out);
        Ok(run)
    }
    pub fn need_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn step(&mut self, event: Event, out: &mut Queue<reconcile::Need>) {
        self.machine.step(event, out);
        self.pending = out.pop();
    }
    pub fn halt(&mut self, why: Why, out: &mut Queue<reconcile::Need>) {
        self.machine.halt(why, out);
        if let Some(need) = out.pop() {
            self.pending = Some(need);
        }
        self.owner = None;
        self.send = None;
        self.retry_at = None;
        self.retry = false;
    }
    pub fn answer(&mut self, answer: Answer, out: &mut Queue<reconcile::Need>) {
        let Some(owner) = self.owner.take() else {
            return;
        };
        let event = match answer {
            Answer::Written => Event::Written,
            Answer::Read(mut value) => {
                match owner {
                    Owner::Read(item) => {
                        if let Some(bytes) = &value
                            && !item::fits(item, bytes.expose(), &self.bounds)
                        {
                            value = None;
                        }
                    }
                    Owner::Write => (),
                }
                Event::Read(value)
            }
        };
        self.step(event, out);
    }
    pub fn reply(&mut self, reply: Reply, out: &mut Queue<reconcile::Need>) {
        let retry = match &reply {
            Reply::Down => true,
            Reply::Status(status) => (500..600).contains(status),
            Reply::Document(_)
            | Reply::Rows(_)
            | Reply::Empty
            | Reply::GithubDocument(_)
            | Reply::GithubRows(_)
            | Reply::Unsigned => false,
        };
        if !self.answered && retry {
            self.retry = true;
            return;
        }
        self.answered = true;
        self.send = None;
        self.step(Event::Reply(reply), out);
    }
    pub fn closed(&mut self, now: Time) {
        self.connection = None;
        if self.retry {
            self.retry = false;
            self.retry_at = Some(now.saturating_add(self.backoff));
            self.backoff = self.backoff.saturating_mul(2).min(self.backoff_max);
        }
    }
    pub fn owner_need(&mut self, need: Need) -> Need {
        self.owner = Some(match &need {
            Need::Read(item) => Owner::Read(*item),
            Need::Write { .. } => Owner::Write,
        });
        need
    }
}

fn issue_state(
    given: &crate::github::given::Given,
    purpose: Purpose,
    now: Time,
    crypto: &mut CryptoRng,
    states: &mut States,
    subject: &[u8],
) -> Result<Box<[u8]>, Failure> {
    let mut random = [0_u8; 16];
    crypto.fill(&mut random);
    states.issue(given.forge, purpose, &random, now, given.limits.state_life, subject)
}
