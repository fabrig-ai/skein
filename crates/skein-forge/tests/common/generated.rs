//! Generated dialect values reuse the moved family tables.
#![expect(
    clippy::arithmetic_side_effects,
    reason = "fixture lengths and seeds are bounded by the literal documents and the prescribed seed range"
)]
#![expect(clippy::disallowed_macros, reason = "the document harness formats fixture values and literal expected rows")]
#![expect(
    clippy::disallowed_methods,
    reason = "the document harness transforms finite fixture rows with ordinary Rust iterators"
)]
#![expect(
    clippy::disallowed_types,
    reason = "the document test harness retains finite fixture rows and mutation buffers in ordinary Rust collections"
)]
#![expect(
    clippy::must_use_candidate,
    reason = "fixture-returning helpers are only consumed by assertions or other fixture construction"
)]
#![expect(clippy::similar_names, reason = "the request and reply names follow the dialect vocabulary")]
use super::{cases, fixtures, forgejo};
use skein_forge::forgejo::{
    document::{Body, Document, ErrorBody, Row},
    request::{Page, Request},
    server::Answer,
};

#[derive(Debug)]
pub struct Reply {
    pub name: &'static str,
    pub request: Option<Request>,
    pub answer: Answer,
    pub document: Document,
    pub rows: Vec<Row>,
}

/// All request variants, including the two not sent by the recorder's base tables.
pub fn requests() -> Vec<(&'static str, Request)> {
    let mut rows = Vec::new();
    rows.extend(cases::start::cases().into_iter().map(|(name, request, _)| (name, request)));
    rows.extend(cases::items::cases().into_iter().map(|(name, request, _)| (name, request)));
    rows.extend(cases::files::cases().into_iter().map(|(name, request, _, _)| (name, request)));
    rows.extend(cases::reviews::cases().into_iter().map(|(name, request, _)| (name, request)));
    rows.extend(forgejo::write_cases());
    rows.extend(forgejo::setup_cases());
    let repo = || skein_forge::forgejo::request::Repo { owner: Box::from(*b"acme"), name: Box::from(*b"widgets") };
    let hook = || skein_forge::forgejo::request::HookSpec {
        url: Box::from(*b"https://hooks.example/"),
        secret: skein_lib::Secret::new(Box::from(*b"example-hook")),
        events: Box::from([Box::<[u8]>::from(*b"push")]),
        active: true,
    };
    rows.extend([
        (
            "repo-collaborator",
            Request::AddCollaborator {
                at: repo(),
                login: Box::from(*b"alice"),
                permission: skein_forge::forgejo::request::Grant::Write,
            },
        ),
        ("repo-hooks", Request::RepoHooks { at: repo(), page: Page { page: Some(1), limit: Some(2) } }),
        ("repo-hook-create", Request::CreateRepoHook { at: repo(), hook: hook() }),
        ("repo-hook-edit", Request::EditRepoHook { at: repo(), id: 7, hook: hook() }),
        ("repo-hook-delete", Request::DeleteRepoHook { at: repo(), id: 7 }),
    ]);
    rows.push(("pulls", Request::Pulls { at: forgejo::specimen(), page: Page { page: Some(1), limit: Some(2) } }));
    rows.push(("delete-hook", Request::DeleteOrgHook { org: Box::from(*b"ensemble"), id: 1 }));
    let mut seen = Vec::new();
    rows.retain(|row| {
        let kind = core::mem::discriminant(&row.1);
        if seen.contains(&kind) {
            false
        } else {
            seen.push(kind);
            true
        }
    });
    rows
}

pub fn replies(seed: u64) -> Vec<Reply> {
    make(seed, false)
}
/// One short generated value per decoder family; the golden fixtures stay full.
pub fn documents(seed: u64) -> Vec<Reply> {
    let mut rows = make(seed, true);
    // One valid generated value per client DecoderState; goldens and inherited
    // transcript sweeps separately retain every Answer variant and recorded case.
    rows.retain(|row| match row.answer {
        Answer::User(_)
        | Answer::ApiSettings(_)
        | Answer::Issue(_)
        | Answer::Files(_)
        | Answer::Run(_)
        | Answer::Reviews(_)
        | Answer::Branch(_)
        | Answer::Org(_)
        | Answer::CreatedRepository(_)
        | Answer::CreatedFile { .. }
        | Answer::Hook(_)
        | Answer::Error(_) => true,
        Answer::Token(_)
        | Answer::Tokens(_)
        | Answer::Hooks(_)
        | Answer::Team(_)
        | Answer::Teams(_)
        | Answer::Members(_)
        | Answer::Repository(_)
        | Answer::Reachable(_)
        | Answer::RepoSearch { .. }
        | Answer::Search { .. }
        | Answer::Issues(_)
        | Answer::Comments(_)
        | Answer::Comment(_)
        | Answer::Status(_)
        | Answer::Review(_)
        | Answer::Pull(_)
        | Answer::Compare { .. }
        | Answer::Runs { .. }
        | Answer::Job { .. }
        | Answer::Jobs(_)
        | Answer::Pulls(_)
        | Answer::ReviewComments(_)
        | Answer::BranchRefs(_)
        | Answer::Branches(_)
        | Answer::Protection(_)
        | Answer::Protections(_)
        | Answer::Collaborators(_)
        | Answer::Tree { .. }
        | Answer::Permission(_)
        | Answer::CombinedStatus { .. } => false,
    });
    rows
}
fn make(seed: u64, compact: bool) -> Vec<Reply> {
    let mut rows = Vec::new();
    let setup = forgejo::setup_cases();
    let writes = forgejo::write_cases();
    for (name, request, mut answer) in cases::start::cases() {
        if known(&rows, &answer) {
            continue;
        }
        if compact {
            shorten(&mut answer);
        }
        vary(&mut answer, seed);
        let (document, values) = cases::start::expected(answer.clone());
        rows.push(Reply { name, request: Some(request), answer, document, rows: values });
    }
    for (name, request, mut answer) in cases::items::cases() {
        if known(&rows, &answer) {
            continue;
        }
        if compact {
            shorten(&mut answer);
        }
        vary(&mut answer, seed);
        let (document, values) = cases::items::expected(answer.clone());
        rows.push(Reply { name, request: Some(request), answer, document, rows: values });
    }
    for (name, request, answer, _) in cases::files::cases() {
        if let Some(mut answer) = answer {
            if known(&rows, &answer) {
                continue;
            }
            if compact {
                shorten(&mut answer);
            }
            vary(&mut answer, seed);
            let (document, values) = cases::files::expected(answer.clone());
            rows.push(Reply { name, request: Some(request), answer, document, rows: values });
        }
    }
    for (name, request, answer) in cases::reviews::cases() {
        if let Some(mut answer) = answer {
            if known(&rows, &answer) {
                continue;
            }
            if compact {
                shorten(&mut answer);
            }
            vary(&mut answer, seed);
            let (document, values) = cases::reviews::expected(answer.clone());
            rows.push(Reply { name, request: Some(request), answer, document, rows: values });
        }
    }
    for (name, mut answer) in cases::writes::answers() {
        if known(&rows, &answer) {
            continue;
        }
        if compact {
            shorten(&mut answer);
        }
        vary(&mut answer, seed);
        let document = cases::writes::expected(answer.clone());
        let request = writes.iter().find(|(case, _)| *case == name).expect("write row").1.clone();
        rows.push(Reply { name, request: Some(request), answer, document, rows: vec![] });
    }
    for (name, mut answer) in cases::setup::answers() {
        if known(&rows, &answer) {
            continue;
        }
        if compact {
            shorten(&mut answer);
        }
        vary(&mut answer, seed);
        let (document, values) = cases::setup::expected(answer.clone());
        rows.push(Reply {
            name,
            request: Some(setup.iter().find(|(case, _)| *case == name).expect("setup row").1.clone()),
            answer,
            document,
            rows: values,
        });
    }
    for (name, answer, document, values) in setup_replies(seed, compact) {
        let request = setup.iter().find(|(case, _)| *case == name).expect("setup row").1.clone();
        rows.push(Reply { name, request: Some(request), answer, document, rows: values });
    }
    rows.extend(extra(seed, compact));
    rows
}

fn extra(seed: u64, compact: bool) -> Vec<Reply> {
    let mut rows = Vec::new();
    let pulls: Box<[skein_forge::forgejo::document::PullRequest]> =
        if compact { Box::from([]) } else { Box::from([cases::items::pull(false)]) };
    rows.push(Reply {
        name: "pulls",
        request: Some(Request::Pulls { at: forgejo::specimen(), page: forgejo::page(1, 2) }),
        document: Document::Listed { skipped: 0 },
        rows: pulls.iter().cloned().map(Row::Pull).collect(),
        answer: Answer::Pulls(pulls),
    });
    let error = ErrorBody { message: Body::Kept(format!("generated error {seed}").into_bytes().into_boxed_slice()) };
    rows.push(Reply {
        name: "error",
        request: None,
        document: Document::Error(error.clone()),
        rows: vec![],
        answer: Answer::Error(error),
    });
    rows
}

// Each scalar stays in its admitted range; seed zero is exactly the moved fixture.
fn vary(answer: &mut Answer, seed: u64) {
    if seed == 0 {
        return;
    }
    match answer {
        Answer::User(v) => v.id += seed,
        Answer::ApiSettings(v) => v.max_response_items += u32::try_from(seed).expect("bounded seed"),
        Answer::Repository(v) => v.id += seed,
        Answer::Search { data, .. } | Answer::Collaborators(data) | Answer::Members(data) => {
            for user in data {
                user.id += seed;
            }
        }
        Answer::Issue(v) => v.number += seed,
        Answer::Issues(values) => {
            for value in values {
                value.number += seed;
            }
        }
        Answer::Comment(v) => v.id += seed,
        Answer::Comments(values) => {
            for value in values {
                value.id += seed;
            }
        }
        Answer::Status(v) => v.context = format!("generated/{seed}").into_bytes().into_boxed_slice(),
        Answer::CombinedStatus { state, .. } => state.total_count += seed,
        Answer::Review(v) => v.id += seed,
        Answer::Reviews(values) => {
            for value in values {
                value.id += seed;
            }
        }
        Answer::ReviewComments(values) => {
            for value in values {
                value.id += seed;
            }
        }
        Answer::Pull(v) => v.number += seed,
        Answer::Pulls(values) => {
            for value in values {
                value.number += seed;
            }
        }
        Answer::Files(values) => {
            for value in values {
                value.filename = format!("file-{seed}.txt").into_bytes().into_boxed_slice();
            }
        }
        Answer::Compare { compare, .. } => compare.total_commits += seed,
        Answer::Runs { total_count, .. } => *total_count += seed,
        Answer::Run(v) => v.id += seed,
        Answer::Job { job, .. } => job.id += seed,
        Answer::Jobs(values) => {
            for value in values {
                value.id += seed;
            }
        }
        Answer::Branch(v) => v.name = format!("branch-{seed}").into_bytes().into_boxed_slice(),
        Answer::BranchRefs(values) | Answer::Branches(values) => {
            for value in values {
                value.name = format!("branch-{seed}").into_bytes().into_boxed_slice();
            }
        }
        Answer::Protection(v) => v.required_approvals += u32::try_from(seed).expect("bounded seed"),
        Answer::Protections(values) => {
            for value in values {
                value.required_approvals += u32::try_from(seed).expect("bounded seed");
            }
        }
        Answer::Permission(v) => v.user.id += seed,
        Answer::Token(v) => v.id += seed,
        Answer::Tokens(values) => {
            for value in values {
                value.id += seed;
            }
        }
        Answer::CreatedRepository(v) => v.id += seed,
        Answer::Org(v) => v.id += seed,
        Answer::Team(v) => v.id += seed,
        Answer::Teams(values) => {
            for value in values {
                value.id += seed;
            }
        }
        Answer::Hook(v) => v.id += seed,
        Answer::Hooks(values) => {
            for value in values {
                value.id += seed;
            }
        }
        Answer::Tree { .. }
        | Answer::Reachable(_)
        | Answer::RepoSearch { .. }
        | Answer::Error(_)
        | Answer::CreatedFile { .. } => {}
    }
}

pub fn deliveries(seed: u64) -> Vec<(&'static str, skein_forge::forgejo::delivery::Delivery)> {
    use skein_forge::forgejo::delivery::Delivery;
    let mut rows = forgejo::delivery_cases();
    let mut seen = Vec::new();
    rows.retain(|(_, value)| {
        let kind = core::mem::discriminant(value);
        if seen.contains(&kind) {
            false
        } else {
            seen.push(kind);
            true
        }
    });
    for (_, delivery) in &mut rows {
        let (Delivery::Issue { repository, .. }
        | Delivery::Pull { repository, .. }
        | Delivery::Comment { repository, .. }
        | Delivery::Review { repository, .. }
        | Delivery::Push { repository, .. }
        | Delivery::Ref { repository, .. }
        | Delivery::Job { repository, .. }) = delivery;
        *repository += seed;
    }
    rows
}

/// The generated documents' largest text is a hook secret (64 bytes), the
/// body marker fits 64 bytes, and the largest nested list is 22 hook events.
pub fn limits() -> skein_forge::forgejo::limits::Limits {
    let mut limits = forgejo::profile_limits();
    limits.string_bytes = 64;
    limits.body_bytes = 64;
    limits.inner = 22;
    limits.json.string = 64;
    limits.json.chunk = 64;
    limits.json.length = 2_048;
    limits.writer.length = 2_048;
    limits
}

fn known(rows: &[Reply], answer: &Answer) -> bool {
    rows.iter().any(|row| core::mem::discriminant(&row.answer) == core::mem::discriminant(answer))
}

// Reuse the moved setup values without constructing every recorded request for
// each reply; a single setup request table above supplies all lookup rows.
fn setup_replies(seed: u64, compact: bool) -> Vec<(&'static str, Answer, Document, Vec<Row>)> {
    let mut created = cases::setup::created_repository(true);
    created.id += seed;
    if compact {
        created.full_name = Box::from(*b"o/r");
        created.default_branch = Box::from(*b"m");
        created.clone_url = Box::from(*b"http://h/o/r.git");
        created.ssh_url = Box::from(*b"ssh://h/o/r.git");
    }
    let mut org = fixtures::org();
    org.id += seed;
    let mut hook = fixtures::hook(1, false, false);
    hook.id += seed;
    if compact {
        hook.events = Box::from([Box::from(*b"push")]);
    }
    vec![
        (
            "setup-repository-created",
            Answer::CreatedRepository(created.clone()),
            Document::CreatedRepository(created),
            vec![],
        ),
        ("setup-org-made", Answer::Org(org.clone()), Document::Org(org), vec![]),
        ("setup-team-made", Answer::Team(fixtures::team()), Document::Team(fixtures::team()), vec![]),
        (
            "setup-team-list-new-org",
            Answer::Teams(Box::from([fixtures::owners()])),
            Document::Listed { skipped: 0 },
            vec![Row::Team(fixtures::owners())],
        ),
        ("setup-team-members-none", Answer::Members(Box::from([])), Document::Listed { skipped: 0 }, vec![]),
        ("setup-hook-made", Answer::Hook(hook.clone()), Document::Hook(hook), vec![]),
        ("setup-hook-list-none", Answer::Hooks(Box::from([])), Document::Listed { skipped: 0 }, vec![]),
    ]
}

fn shorten(answer: &mut Answer) {
    match answer {
        Answer::User(value) => {
            value.login = Box::from(*b"x");
            value.full_name = Box::from([]);
        }
        Answer::Repository(value) => {
            value.owner.login = Box::from(*b"x");
            value.name = Box::from(*b"x");
            value.default_branch = Box::from(*b"x");
            value.html_url = Box::from(*b"x");
        }
        Answer::Search { data, .. } | Answer::Collaborators(data) | Answer::Members(data) => *data = Box::from([]),
        Answer::Issues(values) => *values = Box::from([]),
        Answer::Issue(value) => {
            value.title = Box::from(*b"x");
            value.body = Body::Kept(Box::from(*b"x"));
            value.user.login = Box::from(*b"x");
        }
        Answer::Comments(values) => *values = Box::from([]),
        Answer::Comment(value) => {
            value.body = Body::Kept(Box::from(*b"x"));
            value.user.login = Box::from(*b"x");
            value.created_at = None;
            value.updated_at = None;
        }
        Answer::Review(value) => {
            value.body = Body::Kept(Box::from(*b"x"));
            value.user.login = Box::from(*b"x");
            value.submitted_at = None;
            value.updated_at = None;
        }
        Answer::Reviews(values) => {
            *values = values.iter().take(1).cloned().collect();
            for value in values {
                value.body = Body::Kept(Box::from(*b"x"));
                value.user.login = Box::from(*b"x");
                value.submitted_at = None;
                value.updated_at = None;
            }
        }
        Answer::ReviewComments(values) => *values = Box::from([]),
        Answer::Files(values) => *values = values.iter().take(1).cloned().collect(),
        Answer::Compare { commits, files, .. } => {
            *commits = Box::from([]);
            *files = Box::from([]);
        }
        Answer::Job { steps, .. } => *steps = Box::from([]),
        Answer::Jobs(values) => *values = Box::from([]),
        Answer::BranchRefs(values) | Answer::Branches(values) => *values = Box::from([]),
        Answer::Protection(value) => {
            value.rule_name = Box::from(*b"x");
            value.status_check_contexts = Box::from([]);
        }
        Answer::Protections(values) => *values = Box::from([]),
        Answer::Status(value) => {
            value.context = Box::from(*b"x");
            value.description = Box::from([]);
            value.target_url = Box::from([]);
            value.creator = None;
        }
        Answer::CombinedStatus { statuses, sha, .. } => {
            *statuses = Box::from([]);
            *sha = None;
        }
        Answer::Pull(value) => {
            value.head.name = Box::from(*b"x");
            value.base.name = Box::from(*b"x");
            value.merge_base = None;
            value.merge_commit_sha = None;
            value.requested_reviewers = Box::from([]);
        }
        Answer::Pulls(values) => *values = Box::from([]),
        Answer::Runs { runs, .. } => *runs = Box::from([]),
        Answer::Permission(value) => value.user.login = Box::from(*b"x"),
        Answer::Token(value) => {
            value.name = Box::from(*b"x");
            value.scopes = Box::from([]);
            value.last_eight = Box::from([]);
            value.value = None;
        }
        Answer::Tokens(values) => *values = Box::from([]),
        Answer::Hook(value) => {
            value.url = Box::from(*b"x");
            value.events = Box::from([Box::from(*b"push")]);
        }
        Answer::Teams(values) => *values = Box::from([]),
        Answer::Hooks(values) => *values = Box::from([]),
        Answer::Reachable(_)
        | Answer::RepoSearch { .. }
        | Answer::ApiSettings(_)
        | Answer::Run(_)
        | Answer::Branch(_)
        | Answer::Org(_)
        | Answer::CreatedRepository(_)
        | Answer::CreatedFile { .. }
        | Answer::Team(_)
        | Answer::Tree { .. }
        | Answer::Error(_) => {}
    }
}

/// Short JSON request fixtures; the inherited and golden tables stay complete.
pub fn request_documents() -> Vec<(&'static str, Request)> {
    let mut rows = requests();
    for (_, request) in &mut rows {
        if let Request::CreateOrgHook { hook, .. }
        | Request::EditOrgHook { hook, .. }
        | Request::CreateRepoHook { hook, .. }
        | Request::EditRepoHook { hook, .. } = request
        {
            hook.events = Box::from([Box::from(*b"push")]);
            hook.url = Box::from(*b"x");
            hook.secret = skein_lib::Secret::new(Box::from(*b"x"));
        }
        if let Request::CreateUser { login, email, full_name, password, .. } = request {
            *login = Box::from(*b"x");
            *email = Box::from(*b"x");
            *full_name = Box::from([]);
            *password = skein_lib::Secret::new(Box::from(*b"x"));
        }
        if let Request::CreateToken { name, scopes, .. } = request {
            *name = Box::from(*b"x");
            *scopes = Box::from([]);
        }
        if let Request::CreateIssue { title, body, .. } | Request::OpenPull { title, body, .. } = request {
            *title = Box::from(*b"x");
            *body = Box::from(*b"x");
        }
        if let Request::PostComment { body, .. } | Request::PostReview { body, .. } = request {
            *body = Box::from(*b"x");
        }
    }
    rows
}
