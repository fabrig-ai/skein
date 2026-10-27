use super::fixtures::{self, bytes, decode, limits};
use crate::{
    api,
    github::{
        decode::Malformed,
        document::{self, Document, Kind, Row},
        header::{Head, RateFields},
        plans::{
            Body, Failure, Heard,
            plan::{Bounds, Plan, Step},
            tables::{self, Tables},
        },
        request::Request,
        server,
    },
};
use skein_lib::{Duration, Time, Writer};

const KEY: api::Repository = api::Repository { forge: 3, repository: 7 };
const WORD: &[u8] = b"example-app";
fn bounds() -> Bounds {
    Bounds { inner: 8, page_size: 8, rows: 16, answer_bytes: 4096, body_bytes: 1024 }
}
fn repository() -> document::Repository {
    document::Repository {
        id: 7,
        owner: Box::from(*b"acme"),
        owner_id: 8,
        organization: false,
        name: Box::from(*b"widgets"),
        default_branch: Box::from(*b"main"),
        html_url: Box::from(*b"https://example.invalid/acme/widgets"),
        merge: true,
        squash: true,
        rebase: true,
    }
}
fn pull_request(number: u64) -> Request {
    Request::Pull { owner: Box::from(*b"acme"), name: Box::from(*b"widgets"), number }
}
fn listing_request(open: bool) -> Request {
    Request::PullsFor {
        owner: Box::from(*b"acme"),
        name: Box::from(*b"widgets"),
        head: Box::from(*b"feature"),
        base: Box::from(*b"main"),
        open,
    }
}
fn combined_request(page: u32, per_page: u32) -> Request {
    Request::CombinedStatus {
        owner: Box::from(*b"acme"),
        name: Box::from(*b"widgets"),
        commit: fixtures::pull(5).head_commit,
        page,
        per_page,
    }
}
fn runs_request(page: u32, per_page: u32) -> Request {
    Request::CheckRuns {
        owner: Box::from(*b"acme"),
        name: Box::from(*b"widgets"),
        commit: fixtures::pull(5).head_commit,
        page,
        per_page,
    }
}
fn pull_for() -> api::Read {
    api::Read::PullFor { head: Box::from(*b"feature"), base: Box::from(*b"main") }
}
fn statuses(page: u32) -> api::Read {
    api::Read::Statuses { commit: fixtures::pull(5).head_commit, page }
}
#[expect(clippy::unnecessary_wraps, reason = "expected verdicts retain the heard result shape")]
fn failed(error: api::Error) -> Option<Step> {
    Some(Step::Done(Err(error)))
}
struct Call {
    plan: Plan,
    tables: Tables,
    bounds: Bounds,
}
impl Call {
    fn new(read: api::Read, bounds: Bounds, cached: bool) -> Self {
        let mut tables = Tables::new(2, 2);
        if cached {
            tables.keep_repository(KEY, tables::repository(&repository()));
        }
        Self { plan: Plan::new(KEY, api::Op::Read(read)), tables, bounds }
    }
    fn begin(&mut self) -> Result<Step, api::Error> {
        self.plan.begin(self.bounds, WORD, &mut self.tables)
    }
    fn heard(&mut self, heard: Heard) -> Option<Step> {
        self.plan.heard(heard, self.bounds, WORD, &mut self.tables).unwrap()
    }
    fn head(&mut self, next: bool) {
        let mut head = Head::new();
        head.next = next;
        // These reads do not need a Date header.
        assert_eq!(self.heard(Heard::Head { status: 200, head }), None);
    }
    fn json(&mut self, kind: Kind, out: Writer, next: bool) -> Option<Step> {
        self.head(next);
        let (rows, result) = decode(kind, &bytes(out), 7, &limits());
        for row in rows.into_boxed() {
            if let Some(step) = self.heard(Heard::Row(row)) {
                return Some(step);
            }
        }
        let body = match result {
            Ok(document) => Body::Document(document),
            Err(error) => Body::Malformed(error),
        };
        self.heard(Heard::Done(body))
    }
    fn pull(&mut self, pull: &document::Pull) -> Option<Step> {
        let mut out = Writer::new(4096);
        server::pull(pull, &mut out).unwrap();
        self.json(Kind::Pull, out, false)
    }
    fn listing(&mut self, pulls: &[document::Pull]) -> Option<Step> {
        let mut out = Writer::new(4096);
        server::pulls(pulls, &mut out).unwrap();
        self.json(Kind::PullRows, out, false)
    }
    fn combined(&mut self, check: api::Check, total: u64, contexts: &[document::Context], next: bool) -> Option<Step> {
        let mut out = Writer::new(4096);
        server::combined(&document::Combined { check, total }, contexts, &mut out).unwrap();
        self.json(Kind::CombinedStatus, out, next)
    }
    fn runs(&mut self, checks: &[api::Check], next: bool) -> Option<Step> {
        let mut runs = skein_lib::List::with_capacity(u32::try_from(checks.len()).unwrap());
        for check in checks {
            runs.push(document::Run::new(*check)).unwrap();
        }
        let mut out = Writer::new(4096);
        server::runs(runs.as_slice(), &mut out).unwrap();
        self.json(Kind::CheckRuns, out, next)
    }
}
fn started(read: api::Read) -> Call {
    let mut call = Call::new(read, bounds(), true);
    call.begin().unwrap();
    call
}
fn combined_started() -> Call {
    let mut call = started(api::Read::Pull { number: 5 });
    assert_eq!(call.pull(&fixtures::pull(5)), Some(Step::send(combined_request(1, 1))));
    call
}
fn runs_started() -> Call {
    let mut call = combined_started();
    assert_eq!(call.combined(api::Check::Pending, 0, &[], false), Some(Step::send(runs_request(1, 8))));
    call
}
fn pull_answer(step: Option<Step>) -> api::Pull {
    let Some(Step::Done(Ok(api::Answer::Pull(pull)))) = step else {
        panic!("expected Pull: {step:?}");
    };
    pull
}
fn status_answer(step: Option<Step>) -> (api::Ci, Box<[api::Status]>, bool) {
    let Some(Step::Done(Ok(api::Answer::Statuses { ci, statuses, more }))) = step else {
        panic!("expected Statuses: {step:?}");
    };
    (ci, statuses, more)
}
#[test]
fn github_pull_reads_three_requests() {
    let mut call = Call::new(api::Read::Pull { number: 5 }, bounds(), true);
    assert_eq!(call.begin(), Ok(Step::send(pull_request(5))));
    let pull = fixtures::pull(5);
    assert_eq!(call.pull(&pull), Some(Step::send(combined_request(1, 1))));
    // A failed context on this page is dropped; the envelope controls the fold.
    assert_eq!(
        call.combined(api::Check::Passed, 2, &[fixtures::context(api::Check::Failed)], false),
        Some(Step::send(runs_request(1, 8)))
    );
    assert_eq!(
        pull_answer(call.runs(&[api::Check::Passed], false)),
        api::Pull {
            reviewers: pull.reviewers,
            number: 5,
            state: api::State::Open,
            head: pull.head,
            base: pull.base,
            commit: pull.head_commit,
            base_commit: Some(pull.base_commit),
            merged: None,
            mergeable: true,
            ci: api::Ci::Passed,
            queued: None,
            dequeued: None,
        }
    );
}
#[test]
fn github_pull_null_mergeable_reads_as_false() {
    let body = fixtures::PULL_COMPUTING.as_bytes();
    for chunk in [1, 7, u32::try_from(body.len()).unwrap()] {
        let (rows, decoded) = decode(Kind::Pull, body, chunk, &limits());
        assert!(rows.is_empty());
        let Document::Pull(pull) = decoded.unwrap() else { panic!("pull document") };
        assert_eq!(pull.mergeable, None);
        let mut call = started(api::Read::Pull { number: 5 });
        call.head(false);
        assert_eq!(
            call.heard(Heard::Done(Body::Document(Document::Pull(pull)))),
            Some(Step::send(combined_request(1, 1)))
        );
        assert_eq!(call.combined(api::Check::Pending, 0, &[], false), Some(Step::send(runs_request(1, 8))));
        assert!(!pull_answer(call.runs(&[], false)).mergeable);
    }
}
#[test]
fn github_pull_resolves_absent_repository() {
    for read in [api::Read::Pull { number: 5 }, pull_for(), statuses(3)] {
        let mut call = Call::new(read.clone(), bounds(), false);
        assert_eq!(call.begin(), Ok(Step::send(Request::Repository { id: 7 })));
        let mut out = Writer::new(4096);
        server::repository(&repository(), &mut out).unwrap();
        let request = match read {
            api::Read::Pull { .. } => pull_request(5),
            api::Read::PullFor { .. } => listing_request(true),
            api::Read::Statuses { .. } => combined_request(3, 8),
            api::Read::Locate { .. }
            | api::Read::Items { .. }
            | api::Read::Item { .. }
            | api::Read::Reviews { .. }
            | api::Read::Remarks { .. }
            | api::Read::Branch { .. }
            | api::Read::Branches { .. }
            | api::Read::PullFiles { .. }
            | api::Read::Compare { .. }
            | api::Read::Checks { .. }
            | api::Read::Runs { .. }
            | api::Read::Run { .. }
            | api::Read::Tree { .. }
            | api::Read::Artifact { .. }
            | api::Read::Commits { .. }
            | api::Read::Job { .. }
            | api::Read::File { .. }
            | api::Read::Protection { .. }
            | api::Read::Settings
            | api::Read::Collaborators { .. }
            | api::Read::Permission { .. } => unreachable!(),
        };
        assert_eq!(call.json(Kind::Repository, out, false), Some(Step::send(request)));
        assert!(call.tables.repository(KEY).is_some());
    }
}
#[test]
fn github_pull_ci_folds() {
    for (check, total, runs, expected) in [
        (api::Check::Pending, 0, &[][..], api::Ci::None),
        (api::Check::Passed, 0, &[][..], api::Ci::None),
        (api::Check::Failed, 0, &[][..], api::Ci::None),
        (api::Check::Passed, 1, &[][..], api::Ci::Passed),
        (api::Check::Pending, 1, &[][..], api::Ci::Pending),
        (api::Check::Failed, 1, &[api::Check::Passed][..], api::Ci::Failed),
        (api::Check::Pending, 0, &[api::Check::Passed][..], api::Ci::Passed),
        (api::Check::Passed, 1, &[api::Check::Pending][..], api::Ci::Pending),
        (api::Check::Pending, 1, &[api::Check::Failed][..], api::Ci::Failed),
        (api::Check::Pending, 0, &[api::Check::Pending, api::Check::Failed][..], api::Ci::Failed),
        (api::Check::Pending, 0, &[api::Check::Failed, api::Check::Pending][..], api::Ci::Failed),
    ] {
        let mut call = combined_started();
        assert_eq!(call.combined(check, total, &[], false), Some(Step::send(runs_request(1, 8))));
        assert_eq!(pull_answer(call.runs(runs, false)).ci, expected);
    }
}
#[test]
fn github_pull_runs_page_by_link() {
    let mut call = runs_started();
    assert_eq!(call.runs(&[api::Check::Pending], true), Some(Step::send(runs_request(2, 8))));
    assert_eq!(pull_answer(call.runs(&[api::Check::Failed], false)).ci, api::Ci::Failed);
    let mut call = runs_started();
    for page in 1u32..50 {
        assert_eq!(call.runs(&[], true), Some(Step::send(runs_request(page.checked_add(1).unwrap(), 8))));
    }
    assert_eq!(call.runs(&[], true), failed(api::Error::TooLarge));
    let mut call = runs_started();
    for page in 1u32..50 {
        assert_eq!(call.runs(&[], true), Some(Step::send(runs_request(page.checked_add(1).unwrap(), 8))));
    }
    assert_eq!(pull_answer(call.runs(&[], false)).ci, api::Ci::None);
}
#[test]
fn github_pull_merged_and_closed() {
    for (closed, merged, merge_commit, state, expected) in [
        (true, true, Some(fixtures::pull(5).base_commit), api::State::Closed, Some(fixtures::pull(5).base_commit)),
        (false, true, Some(fixtures::pull(5).base_commit), api::State::Closed, Some(fixtures::pull(5).base_commit)),
        (true, false, None, api::State::Closed, None),
        (false, false, Some(fixtures::pull(5).base_commit), api::State::Open, None),
    ] {
        let mut pull = fixtures::pull(5);
        pull.closed = closed;
        pull.merged = merged;
        pull.merge_commit = merge_commit;
        let mut call = started(api::Read::Pull { number: 5 });
        assert_eq!(call.pull(&pull), Some(Step::send(combined_request(1, 1))));
        assert_eq!(call.combined(api::Check::Pending, 0, &[], false), Some(Step::send(runs_request(1, 8))));
        let answer = call.runs(&[], false);
        let pull = if merged {
            assert_eq!(answer, Some(Step::send(graphql_request())));
            pull_answer(call.graphql(&graphql_row()))
        } else {
            pull_answer(answer)
        };
        assert_eq!((pull.state, pull.merged), (state, expected));
    }
}
#[test]
fn github_pull_wrong_number() {
    assert_eq!(started(api::Read::Pull { number: 5 }).pull(&fixtures::pull(6)), failed(api::Error::InvalidAnswer));
}
#[test]
fn github_pull_failures() {
    for status in [404, 410, 301, 307] {
        for stage in 0u8..3 {
            let mut call = match stage {
                0 => started(api::Read::Pull { number: 5 }),
                1 => combined_started(),
                2 => runs_started(),
                _ => unreachable!(),
            };
            let expected = if status == 404 || status == 410 { api::Error::Missing } else { api::Error::Unavailable };
            assert_eq!(call.heard(Heard::Head { status, head: Head::new() }), failed(expected));
            assert_eq!(call.tables.repository(KEY), None);
        }
    }
    for (failure, error) in [(Failure::Unseen, api::Error::Unavailable), (Failure::Seen, api::Error::Timeout)] {
        for stage in 0u8..3 {
            let mut call = match stage {
                0 => started(api::Read::Pull { number: 5 }),
                1 => combined_started(),
                2 => runs_started(),
                _ => unreachable!(),
            };
            assert_eq!(call.heard(Heard::Failed(failure)), failed(error));
        }
    }
    for status in [403, 429] {
        let mut call = runs_started();
        let mut head = Head::new();
        head.rate = Some(RateFields { limit: 5000, remaining: 0, reset: fixtures::wall() });
        head.retry_after = Some(Duration::from_secs(17));
        assert_eq!(call.heard(Heard::Head { status, head }), None);
        assert_eq!(
            call.heard(Heard::Done(Body::Empty)),
            failed(api::Error::RateLimited { after: Duration::from_secs(17) })
        );
    }
    for (malformed, error) in [
        (Malformed::Value { field: b"state" }, api::Error::InvalidAnswer),
        (Malformed::Bound { field: b"requested_reviewers" }, api::Error::TooLarge),
    ] {
        let mut call = started(api::Read::Pull { number: 5 });
        call.head(false);
        assert_eq!(call.heard(Heard::Done(Body::Malformed(malformed))), failed(error));
    }
    for row in [
        Row::Run(document::Run::new(api::Check::Pending)),
        Row::Pull(fixtures::pull(5)),
        Row::Context(fixtures::context(api::Check::Passed)),
    ] {
        let mut call = started(api::Read::Pull { number: 5 });
        call.head(false);
        assert_eq!(call.heard(Heard::Row(row)), failed(api::Error::InvalidAnswer));
    }
    let mut call = runs_started();
    assert_eq!(call.runs(&[api::Check::Passed; 9], false), failed(api::Error::TooLarge));
}
#[test]
fn github_pull_for_prefers_open() {
    let mut call = Call::new(pull_for(), bounds(), true);
    assert_eq!(call.begin(), Ok(Step::send(listing_request(true))));
    // Only the number is taken from the listing; the full read supplies all fields.
    let mut listed = fixtures::pull(5);
    listed.head = Box::from(*b"ignored");
    listed.merged = true;
    assert_eq!(call.listing(&[listed]), Some(Step::send(pull_request(5))));
    assert_eq!(call.pull(&fixtures::pull(5)), Some(Step::send(combined_request(1, 1))));
    assert_eq!(call.combined(api::Check::Passed, 1, &[], false), Some(Step::send(runs_request(1, 8))));
    let pull = pull_answer(call.runs(&[], false));
    assert_eq!(
        (pull.number, pull.head.as_ref(), pull.base.as_ref(), pull.ci),
        (5, b"feature".as_slice(), b"main".as_slice(), api::Ci::Passed)
    );
    let mut call = started(pull_for());
    assert_eq!(call.listing(&[fixtures::pull(5)]), Some(Step::send(pull_request(5))));
    let mut wrong = fixtures::pull(5);
    wrong.head = Box::from(*b"another");
    assert_eq!(call.pull(&wrong), failed(api::Error::InvalidAnswer));
}
#[test]
fn github_pull_for_falls_back_to_oldest() {
    let mut call = started(pull_for());
    assert_eq!(call.listing(&[]), Some(Step::send(listing_request(false))));
    assert_eq!(call.listing(&[fixtures::pull(3)]), Some(Step::send(pull_request(3))));
    assert_eq!(call.pull(&fixtures::pull(3)), Some(Step::send(combined_request(1, 1))));
    assert_eq!(call.combined(api::Check::Pending, 0, &[], false), Some(Step::send(runs_request(1, 8))));
    assert_eq!(pull_answer(call.runs(&[], false)).number, 3);
    let mut call = started(pull_for());
    assert_eq!(call.listing(&[]), Some(Step::send(listing_request(false))));
    assert_eq!(call.listing(&[]), failed(api::Error::Missing));
    assert!(call.tables.repository(KEY).is_some());
    for fallback in [false, true] {
        let mut call = started(pull_for());
        if fallback {
            assert_eq!(call.listing(&[]), Some(Step::send(listing_request(false))));
        }
        assert_eq!(call.listing(&[fixtures::pull(5), fixtures::pull(6)]), failed(api::Error::InvalidAnswer));
    }
    for open in [false, true] {
        let mut call = started(pull_for());
        if !open {
            assert_eq!(call.listing(&[]), Some(Step::send(listing_request(false))));
        }
        assert_eq!(call.heard(Heard::Head { status: 404, head: Head::new() }), failed(api::Error::Missing));
        assert_eq!(call.tables.repository(KEY), None);
    }
}
#[test]
fn github_statuses_answers_page() {
    let mut call = Call::new(statuses(3), bounds(), true);
    assert_eq!(call.begin(), Ok(Step::send(combined_request(3, 8))));
    let mut absent = fixtures::context(api::Check::Pending);
    absent.updated = None;
    absent.description = Box::from([]);
    absent.url = Box::from([]);
    let contexts = [fixtures::context(api::Check::Passed), fixtures::context(api::Check::Failed), absent];
    let (ci, statuses, more) = status_answer(call.combined(api::Check::Failed, 3, &contexts, false));
    assert_eq!((ci, more, statuses.len()), (api::Ci::Failed, false, 3));
    for (status, context) in statuses.iter().zip(contexts) {
        assert_eq!(status.context, context.context);
        assert_eq!(status.check, context.check);
        assert_eq!(status.description, context.description);
        assert_eq!(status.url, context.url);
        assert_eq!(status.job, None);
        let at = match context.updated {
            Some(wall) => Time::from_nanos(wall.as_nanos()),
            None => Time::ZERO,
        };
        assert_eq!(status.at, at);
    }
}
#[test]
fn github_statuses_more_by_link() {
    for (next, page) in [(true, 7), (false, 2)] {
        let mut call = Call::new(statuses(page), bounds(), true);
        assert_eq!(call.begin(), Ok(Step::send(combined_request(page, 8))));
        assert_eq!(
            status_answer(call.combined(api::Check::Passed, 1, &[fixtures::context(api::Check::Passed)], next)).2,
            next
        );
    }
    // The envelope may put total_count after the rows; rows alone never finish the call.
    let mut call = started(statuses(1));
    call.head(false);
    assert_eq!(call.heard(Heard::Row(Row::Context(fixtures::context(api::Check::Passed)))), None);
    assert_eq!(
        status_answer(call.heard(Heard::Done(Body::Document(Document::Combined(document::Combined {
            check: api::Check::Passed,
            total: 1
        })))))
        .0,
        api::Ci::Passed
    );
}
#[test]
fn github_statuses_bounds() {
    let mut small = bounds();
    small.page_size = 1;
    let mut call = Call::new(statuses(1), small, true);
    assert_eq!(call.begin(), Ok(Step::send(combined_request(1, 1))));
    assert_eq!(
        call.combined(
            api::Check::Passed,
            2,
            &[fixtures::context(api::Check::Passed), fixtures::context(api::Check::Passed)],
            false
        ),
        failed(api::Error::TooLarge)
    );
    let context = fixtures::context(api::Check::Passed);
    let measured = crate::measure::answer(&api::Answer::Statuses {
        ci: api::Ci::Passed,
        statuses: Box::from([api::Status {
            at: Time::ZERO,
            context: context.context.clone(),
            check: context.check,
            description: context.description.clone(),
            url: context.url.clone(),
            job: None,
        }]),
        more: false,
    })
    .unwrap();
    for (answer_bytes, fits) in
        [(u32::try_from(measured).unwrap(), true), (u32::try_from(measured).unwrap().checked_sub(1).unwrap(), false)]
    {
        let mut small = bounds();
        small.answer_bytes = answer_bytes;
        let mut call = Call::new(statuses(1), small, true);
        call.begin().unwrap();
        let result = call.combined(api::Check::Passed, 1, core::slice::from_ref(&context), false);
        if fits {
            assert_eq!(status_answer(result).1.len(), 1);
        } else {
            assert_eq!(result, failed(api::Error::TooLarge));
        }
    }
    for field in [0u8, 1, 2] {
        let mut small = bounds();
        match field {
            0 => small.page_size = 0,
            1 => small.rows = 0,
            2 => small.answer_bytes = 159,
            _ => unreachable!(),
        }
        for read in [statuses(1), api::Read::Pull { number: 5 }, pull_for()] {
            assert_eq!(Call::new(read, small, true).begin(), Err(api::Error::TooLarge));
        }
    }
    let mut small = bounds();
    small.rows = 1;
    small.page_size = 100;
    let mut call = Call::new(statuses(1), small, true);
    assert_eq!(call.begin(), Ok(Step::send(combined_request(1, 1))));
    assert_eq!(call.combined(api::Check::Passed, 2, &[context.clone(), context], false), failed(api::Error::TooLarge));
    let mut call = started(statuses(1));
    assert_eq!(status_answer(call.combined(api::Check::Pending, 0, &[], false)), (api::Ci::None, Box::from([]), false));
    let mut small = bounds();
    small.answer_bytes = 160;
    let mut call = Call::new(api::Read::Pull { number: 5 }, small, true);
    call.begin().unwrap();
    let mut pull = fixtures::pull(5);
    pull.head = Box::from(&[b'x'; 100][..]);
    pull.base = Box::from(&[b'y'; 100][..]);
    assert_eq!(call.pull(&pull), failed(api::Error::TooLarge));
}
#[test]
fn github_statuses_failures() {
    for status in [404, 410] {
        let mut call = started(statuses(1));
        assert_eq!(call.heard(Heard::Head { status, head: Head::new() }), failed(api::Error::Missing));
        assert_eq!(call.tables.repository(KEY), None);
    }
    let mut call = started(statuses(1));
    let mut head = Head::new();
    head.rate = Some(RateFields { limit: 5000, remaining: 0, reset: fixtures::wall() });
    head.retry_after = Some(Duration::from_secs(9));
    assert_eq!(call.heard(Heard::Head { status: 403, head }), None);
    assert_eq!(call.heard(Heard::Done(Body::Empty)), failed(api::Error::RateLimited { after: Duration::from_secs(9) }));
    let mut call = started(statuses(1));
    call.head(false);
    assert_eq!(
        call.heard(Heard::Done(Body::Malformed(Malformed::Value { field: b"state" }))),
        failed(api::Error::InvalidAnswer)
    );
    for document in [Document::Rows, Document::Pull(fixtures::pull(5))] {
        let mut call = started(statuses(1));
        call.head(false);
        assert_eq!(call.heard(Heard::Done(Body::Document(document))), failed(api::Error::InvalidAnswer));
    }
}

fn graphql_request() -> Request {
    Request::PullGraphql { owner: Box::from(*b"acme"), name: Box::from(*b"widgets"), number: 5 }
}
fn graphql_row() -> document::PullGraphql {
    document::PullGraphql {
        id: Box::from(*b"opaque+/=node"),
        merge_commit: Some(fixtures::pull(5).base_commit),
        queued: Some(2),
        dequeued: Some(Box::from(*b"checks failed")),
    }
}
impl Call {
    fn graphql(&mut self, row: &document::PullGraphql) -> Option<Step> {
        let mut out = Writer::new(4096);
        server::pull_graphql(row, &mut out).unwrap();
        self.json(Kind::PullGraphql, out, false)
    }
}
fn graphql_started(read: api::Read, organization: bool, merged: bool, bounds: Bounds, cached: bool) -> Call {
    let mut call = Call::new(read.clone(), bounds, false);
    let mut repository = repository();
    repository.organization = organization;
    if cached {
        call.tables.keep_repository(KEY, tables::repository(&repository));
        call.begin().unwrap();
    } else {
        assert_eq!(call.begin(), Ok(Step::send(Request::Repository { id: 7 })));
        let mut out = Writer::new(4096);
        server::repository(&repository, &mut out).unwrap();
        let step = if read == pull_for() { listing_request(true) } else { pull_request(5) };
        assert_eq!(call.json(Kind::Repository, out, false), Some(Step::send(step)));
    }
    if read == pull_for() {
        assert_eq!(call.listing(&[fixtures::pull(5)]), Some(Step::send(pull_request(5))));
    }
    let mut pull = fixtures::pull(5);
    pull.merged = merged;
    assert_eq!(call.pull(&pull), Some(Step::send(combined_request(1, 1))));
    assert_eq!(call.combined(api::Check::Passed, 1, &[], false), Some(Step::send(runs_request(1, 8))));
    assert_eq!(call.runs(&[], false), Some(Step::send(graphql_request())));
    call
}
#[test]
fn github_pull_queue_graphql() {
    for read in [api::Read::Pull { number: 5 }, pull_for()] {
        for cached in [false, true] {
            for queued in [Some(2), None] {
                let mut call = graphql_started(read.clone(), true, false, bounds(), cached);
                let row = document::PullGraphql { queued, ..graphql_row() };
                let pull = pull_answer(call.graphql(&row));
                assert_eq!(pull.queued, queued);
                assert_eq!(pull.dequeued, row.dequeued);
                assert_eq!(pull.merged, None);
                assert_eq!(pull.ci, api::Ci::Passed);
                assert_eq!(pull.state, api::State::Open);
            }
        }
    }
}
#[test]
fn github_pull_merged_absent_graphql_fallback() {
    for organization in [false, true] {
        for read in [api::Read::Pull { number: 5 }, pull_for()] {
            let row = document::PullGraphql { queued: None, dequeued: None, ..graphql_row() };
            let mut call = graphql_started(read.clone(), organization, true, bounds(), true);
            let pull = pull_answer(call.graphql(&row));
            assert_eq!(pull.merged, row.merge_commit);
            assert_eq!(pull.state, api::State::Closed);
            let mut call = graphql_started(read, organization, true, bounds(), true);
            let row = document::PullGraphql { merge_commit: None, ..row };
            assert_eq!(call.graphql(&row), failed(api::Error::InvalidAnswer));
        }
    }
}
#[test]
fn github_pull_graphql_answer_bounds() {
    let mut call = graphql_started(api::Read::Pull { number: 5 }, true, false, bounds(), true);
    let answer = api::Answer::Pull(pull_answer(call.graphql(&graphql_row())));
    let bytes = u32::try_from(crate::measure::answer(&answer).unwrap()).unwrap();
    for (answer_bytes, fits) in [(bytes, true), (bytes - 1, false)] {
        let mut call = graphql_started(api::Read::Pull { number: 5 }, true, false, bounds(), true);
        call.bounds.answer_bytes = answer_bytes;
        if fits {
            assert_eq!(call.graphql(&graphql_row()), Some(Step::Done(Ok(answer.clone()))));
        } else {
            assert_eq!(call.graphql(&graphql_row()), failed(api::Error::TooLarge));
        }
    }
}
#[test]
fn github_pull_graphql_plan_failures() {
    for (status, expected) in [(401, api::Error::Forbidden), (404, api::Error::Missing), (500, api::Error::Timeout)] {
        let mut call = graphql_started(api::Read::Pull { number: 5 }, true, false, bounds(), true);
        assert_eq!(call.heard(Heard::Head { status, head: Head::new() }), failed(expected));
    }
    for body in [
        Body::Document(Document::Graphql(document::GraphqlRefusal {
            kind: Box::from(*b"FORBIDDEN"),
            message: Box::from(*b"refused"),
        })),
        Body::Document(Document::Rows),
        Body::Malformed(Malformed::Missing { field: b"id" }),
        Body::Empty,
    ] {
        let mut call = graphql_started(api::Read::Pull { number: 5 }, true, false, bounds(), true);
        call.head(false);
        assert_eq!(call.heard(Heard::Done(body)), failed(api::Error::InvalidAnswer));
    }
    let mut call = started(api::Read::Pull { number: 5 });
    call.head(false);
    assert_eq!(
        call.heard(Heard::Done(Body::Document(Document::PullGraphql(graphql_row())))),
        failed(api::Error::InvalidAnswer)
    );
    let mut call = graphql_started(api::Read::Pull { number: 5 }, true, false, bounds(), true);
    assert_eq!(call.heard(Heard::Failed(Failure::Unseen)), failed(api::Error::Unavailable));
}

#[test]
fn github_pull_closed_organization_uses_graphql_only_when_merged() {
    for merged in [false, true] {
        let mut call = Call::new(api::Read::Pull { number: 5 }, bounds(), false);
        let mut repository = repository();
        repository.organization = true;
        call.tables.keep_repository(KEY, tables::repository(&repository));
        call.begin().unwrap();
        let mut pull = fixtures::pull(5);
        pull.closed = true;
        pull.merged = merged;
        pull.merge_commit = Some(pull.base_commit);
        assert_eq!(call.pull(&pull), Some(Step::send(combined_request(1, 1))));
        assert_eq!(call.combined(api::Check::Pending, 0, &[], false), Some(Step::send(runs_request(1, 8))));
        let step = call.runs(&[], false);
        let answer = if merged {
            assert_eq!(step, Some(Step::send(graphql_request())));
            let row = document::PullGraphql { queued: None, dequeued: None, ..graphql_row() };
            pull_answer(call.graphql(&row))
        } else {
            pull_answer(step)
        };
        assert_eq!(answer.merged, if merged { pull.merge_commit } else { None });
        assert_eq!(answer.queued, None);
        assert_eq!(answer.dequeued, None);
    }
}
