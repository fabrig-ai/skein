//! The domain's CI jobs are check runs; people's reports stay classic statuses.
use alloc::boxed::Box;
use skein_fake_forge_domain::{Domain, api as a};
use skein_forge::{api, github::document as d};
use skein_lib::{List, Wall, bytes::copy_of};

use super::commit;
use crate::{
    answers::{self, Body, Page},
    service::{Limits, Response},
};

fn check(state: a::Check) -> api::Check {
    match state {
        a::Check::Pending => api::Check::Pending,
        a::Check::Passed => api::Check::Passed,
        a::Check::Failed => api::Check::Failed,
    }
}

#[expect(clippy::too_many_arguments, reason = "CI rendering carries the page and its repository/commit identity")]
pub(super) fn render(
    answer: a::Answer,
    repository: &[u8],
    count: u64,
    page: u32,
    size: u32,
    target: &[u8],
    runs: bool,
    limits: &Limits,
    domain: &Domain,
) -> Response {
    let a::Answer::Checks(checks) = answer else {
        return answers::failed();
    };
    let mut total = 0_u64;
    let mut combined = api::Check::Passed;
    for summary in &checks {
        if summary.link.is_empty() != runs {
            total = total.saturating_add(1);
            match check(summary.status.state) {
                api::Check::Failed => combined = api::Check::Failed,
                api::Check::Pending if combined != api::Check::Failed => combined = api::Check::Pending,
                api::Check::Pending | api::Check::Passed => {}
            }
        }
    }
    if total == 0 {
        combined = api::Check::Pending;
    }
    let Some(page) = Page::new(page, size, total) else {
        return answers::failed();
    };
    let response = if runs {
        run_page(checks, repository, count, &page, size, limits, domain)
    } else {
        context_page(checks, &page, size, d::Combined { check: combined, total }, limits)
    };
    page.link_next(response, target, limits)
}

fn run_page(
    checks: Box<[a::CheckSummary]>,
    repository: &[u8],
    count: u64,
    page: &Page,
    size: u32,
    limits: &Limits,
    domain: &Domain,
) -> Response {
    let mut rows = List::with_capacity(size);
    let mut index = 0_u64;
    for summary in checks {
        if summary.link.is_empty() {
            continue;
        }
        let keep = page.contains(index);
        index = index.saturating_add(1);
        if !keep {
            continue;
        }
        let Some(job) = domain.check_run(repository, count, &summary.status.context) else {
            return answers::failed();
        };
        let check = check(summary.status.state);
        let at = Wall::from_nanos(summary.status.at.as_nanos());
        let completed = match check {
            api::Check::Pending => None,
            api::Check::Passed | api::Check::Failed => Some(at),
        };
        rows.push(d::Run {
            check,
            id: Some(job.job),
            head: Some(commit(count)),
            completed,
            started: Some(at),
            name: summary.status.context,
            title: summary.description,
            url: summary.link,
            app: Some(copy_of(b"github-actions")),
        })
        .expect("one descriptor per page row");
    }
    answers::response(200, &Body::Runs(rows.as_slice()), limits)
}

fn context_page(
    checks: Box<[a::CheckSummary]>,
    page: &Page,
    size: u32,
    combined: d::Combined,
    limits: &Limits,
) -> Response {
    let mut rows = List::with_capacity(size);
    let mut index = 0_u64;
    for summary in checks {
        if !summary.link.is_empty() {
            continue;
        }
        let keep = page.contains(index);
        index = index.saturating_add(1);
        if !keep {
            continue;
        }
        rows.push(d::Context {
            updated: Some(Wall::from_nanos(summary.status.at.as_nanos())),
            context: summary.status.context,
            check: check(summary.status.state),
            description: summary.description,
            url: summary.link,
        })
        .expect("one descriptor per page row");
    }
    answers::response(200, &Body::Combined(&combined, rows.as_slice()), limits)
}

pub(super) fn job(answer: a::Answer, repository: &[u8], limits: &Limits, domain: &Domain) -> Response {
    let a::Answer::Job { job, .. } = answer else {
        return answers::failed();
    };
    let Some(count) = domain.job_commit(repository, job.id) else {
        return answers::missing(limits);
    };
    answers::response(
        200,
        &Body::Job(&d::Job { id: job.id, run: job.run, attempt: job.attempt, head: commit(count), step: None }),
        limits,
    )
}
