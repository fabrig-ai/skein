use super::fixtures::{boxed, fake_config};
use crate::observations::{self, Reason};
use alloc::boxed::Box;
use skein_fake_forge_domain::{Branches, Observation, Operation, api as a, promises::Recorded};
use skein_lib::{List, Time};

fn assert_record(actual: Recorded, expected: &Recorded) {
    match (actual, expected) {
        (Recorded::Lost(actual), Recorded::Lost(expected)) => assert_eq!(actual, *expected),
        (
            Recorded::Observed { observation, forward },
            Recorded::Observed { observation: expected, forward: expected_forward },
        ) => {
            assert_eq!(&observation, expected);
            assert_eq!(&forward, expected_forward);
        }
        (Recorded::Lost(_), Recorded::Observed { .. }) | (Recorded::Observed { .. }, Recorded::Lost(_)) => {
            panic!("record kind")
        }
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one table specifies every variant and enum spelling and its exact JSON line"
)]
fn observation_lines_round_trip() {
    let rows = [
        (
            "Moved",
            Recorded::Observed {
                observation: Observation::Moved {
                    repository: boxed(b"org/repo"),
                    branch: boxed(b"work"),
                    from: Some(3),
                    to: 7,
                    by: 2,
                },
                forward: Some(true),
            },
            r#"{"observation":"moved","repository":"org/repo","branch":"work","from":3,"to":7,"by":2,"forward":true}"#,
        ),
        (
            "Rebased",
            Recorded::Observed {
                observation: Observation::Rebased {
                    repository: boxed(b"org/repo"),
                    branch: boxed(b"work"),
                    from: 3,
                    to: 7,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"rebased","repository":"org/repo","branch":"work","from":3,"to":7,"by":2}"#,
        ),
        (
            "Deleted",
            Recorded::Observed {
                observation: Observation::Deleted {
                    repository: boxed(b"org/repo"),
                    branch: boxed(b"work"),
                    at: 7,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"deleted","repository":"org/repo","branch":"work","at":7,"by":2}"#,
        ),
        (
            "Opened",
            Recorded::Observed {
                observation: Observation::Opened {
                    repository: boxed(b"org/repo"),
                    number: 7,
                    kind: a::Kind::Pull,
                    title: boxed(b"title"),
                    body: boxed(b"body"),
                    labels: Box::new([boxed(b"bug"), boxed(b"ready")]),
                    branches: Some(Branches { head: boxed(b"work"), base: boxed(b"main"), commit: 7 }),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"opened","repository":"org/repo","number":7,"kind":"pull","title":"dGl0bGU=","body":"Ym9keQ==","labels":["bug","ready"],"branches":{"head":"work","base":"main","commit":7},"by":2}"#,
        ),
        (
            "Closed",
            Recorded::Observed {
                observation: Observation::Closed { repository: boxed(b"org/repo"), number: 7, by: 2 },
                forward: None,
            },
            r#"{"observation":"closed","repository":"org/repo","number":7,"by":2}"#,
        ),
        (
            "Reopened",
            Recorded::Observed {
                observation: Observation::Reopened { repository: boxed(b"org/repo"), number: 7, by: 2 },
                forward: None,
            },
            r#"{"observation":"reopened","repository":"org/repo","number":7,"by":2}"#,
        ),
        (
            "Labelled",
            Recorded::Observed {
                observation: Observation::Labelled {
                    repository: boxed(b"org/repo"),
                    number: 7,
                    labels: Box::new([boxed(b"bug"), boxed(b"ready")]),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"labelled","repository":"org/repo","number":7,"labels":["bug","ready"],"by":2}"#,
        ),
        (
            "Revised",
            Recorded::Observed {
                observation: Observation::Revised {
                    repository: boxed(b"org/repo"),
                    number: 7,
                    title: boxed(b"title"),
                    body: boxed(b"body"),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"revised","repository":"org/repo","number":7,"title":"dGl0bGU=","body":"Ym9keQ==","by":2}"#,
        ),
        (
            "Depends",
            Recorded::Observed {
                observation: Observation::Depends {
                    repository: boxed(b"org/repo"),
                    number: 7,
                    dependencies: Box::new([3, 4]),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"depends","repository":"org/repo","number":7,"dependencies":[3,4],"by":2}"#,
        ),
        (
            "Requested",
            Recorded::Observed {
                observation: Observation::Requested {
                    repository: boxed(b"org/repo"),
                    number: 7,
                    reviewers: Box::new([3, 4]),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"requested","repository":"org/repo","number":7,"reviewers":[3,4],"by":2}"#,
        ),
        (
            "Defined",
            Recorded::Observed {
                observation: Observation::Defined { repository: boxed(b"org/repo"), label: boxed(b"bug"), by: 2 },
                forward: None,
            },
            r#"{"observation":"defined","repository":"org/repo","label":"bug","by":2}"#,
        ),
        (
            "Commented",
            Recorded::Observed {
                observation: Observation::Commented {
                    repository: boxed(b"org/repo"),
                    number: 7,
                    id: 7,
                    body: boxed(b"body"),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"commented","repository":"org/repo","number":7,"id":7,"body":"Ym9keQ==","by":2}"#,
        ),
        (
            "Edited",
            Recorded::Observed {
                observation: Observation::Edited {
                    repository: boxed(b"org/repo"),
                    number: 7,
                    id: 7,
                    body: boxed(b"body"),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"edited","repository":"org/repo","number":7,"id":7,"body":"Ym9keQ==","by":2}"#,
        ),
        (
            "Removed",
            Recorded::Observed {
                observation: Observation::Removed { repository: boxed(b"org/repo"), number: 7, id: 7, by: 2 },
                forward: None,
            },
            r#"{"observation":"removed","repository":"org/repo","number":7,"id":7,"by":2}"#,
        ),
        (
            "Reviewed",
            Recorded::Observed {
                observation: Observation::Reviewed {
                    repository: boxed(b"org/repo"),
                    number: 7,
                    id: 7,
                    commit: 7,
                    verdict: a::Verdict::RequestChanges,
                    body: boxed(b"body"),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"reviewed","repository":"org/repo","number":7,"id":7,"commit":7,"verdict":"request_changes","body":"Ym9keQ==","by":2}"#,
        ),
        (
            "Reported",
            Recorded::Observed {
                observation: Observation::Reported {
                    repository: boxed(b"org/repo"),
                    commit: 7,
                    context: boxed(b"ci"),
                    state: a::Check::Passed,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"reported","repository":"org/repo","commit":7,"context":"ci","state":"passed","by":2}"#,
        ),
        (
            "Merged",
            Recorded::Observed {
                observation: Observation::Merged {
                    repository: boxed(b"org/repo"),
                    number: 7,
                    base: boxed(b"main"),
                    head: 7,
                    commit: 7,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"merged","repository":"org/repo","number":7,"base":"main","head":7,"commit":7,"by":2}"#,
        ),
        (
            "Enqueued",
            Recorded::Observed {
                observation: Observation::Enqueued {
                    repository: boxed(b"org/repo"),
                    number: 7,
                    head: 7,
                    position: 7,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"enqueued","repository":"org/repo","number":7,"head":7,"position":7,"by":2}"#,
        ),
        (
            "Group",
            Recorded::Observed {
                observation: Observation::Group {
                    repository: boxed(b"org/repo"),
                    number: 7,
                    commit: 7,
                    ended: true,
                    landed: false,
                },
                forward: None,
            },
            r#"{"observation":"group","repository":"org/repo","number":7,"commit":7,"ended":true,"landed":false}"#,
        ),
        (
            "Dequeued",
            Recorded::Observed {
                observation: Observation::Dequeued {
                    repository: boxed(b"org/repo"),
                    number: 7,
                    reason: a::QueueRemoval::MergeConflict,
                },
                forward: None,
            },
            r#"{"observation":"dequeued","repository":"org/repo","number":7,"reason":"merge_conflict"}"#,
        ),
        (
            "Refused",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: Some(3),
                    commit: Some(3),
                    error: a::Error::Unavailable,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":3,"commit":3,"error":{"name":"unavailable"},"by":2}"#,
        ),
        (
            "Rejected",
            Recorded::Observed {
                observation: Observation::Rejected {
                    repository: boxed(b"org/repo"),
                    branch: boxed(b"work"),
                    commit: 7,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"rejected","repository":"org/repo","branch":"work","commit":7,"by":2}"#,
        ),
        (
            "Wiki",
            Recorded::Observed {
                observation: Observation::Wiki {
                    repository: boxed(b"org/repo"),
                    name: boxed(b"page"),
                    content: Some(Box::new([0, 255, 10])),
                    revision: 7,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"wiki","repository":"org/repo","name":"page","content":"AP8K","revision":7,"by":2}"#,
        ),
        (
            "error Unavailable",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Unavailable,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"unavailable"},"by":2}"#,
        ),
        (
            "error Timeout",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Timeout,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"timeout"},"by":2}"#,
        ),
        (
            "error RateLimited",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::RateLimited { reset: Time::from_nanos(u64::MAX) },
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"rate_limited","reset":18446744073709551615},"by":2}"#,
        ),
        (
            "error Forbidden",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "error Missing",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Missing(a::What::Repository),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"missing","what":"repository"},"by":2}"#,
        ),
        (
            "error TooLarge",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::TooLarge,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"too_large"},"by":2}"#,
        ),
        (
            "error Full",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Full,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"full"},"by":2}"#,
        ),
        (
            "error Exists",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Exists,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"exists"},"by":2}"#,
        ),
        (
            "error AlreadyQueued",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::AlreadyQueued(a::QueueEntry { number: 7, head: 8, position: u32::MAX }),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"already_queued","number":7,"head":8,"position":4294967295},"by":2}"#,
        ),
        (
            "error Circular",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Circular,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"circular"},"by":2}"#,
        ),
        (
            "error NothingToMerge",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::NothingToMerge,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"nothing_to_merge"},"by":2}"#,
        ),
        (
            "error Empty",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Empty,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"empty"},"by":2}"#,
        ),
        (
            "error Closed",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Closed,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"closed"},"by":2}"#,
        ),
        (
            "error Stale",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Stale,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"stale"},"by":2}"#,
        ),
        (
            "error Conflict",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Conflict,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"conflict"},"by":2}"#,
        ),
        (
            "error Protected",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Protected,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"protected"},"by":2}"#,
        ),
        (
            "error Unreachable",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Unreachable,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"unreachable"},"by":2}"#,
        ),
        (
            "error Refused",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Refused,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"refused"},"by":2}"#,
        ),
        (
            "missing Item",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Missing(a::What::Item),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"missing","what":"item"},"by":2}"#,
        ),
        (
            "missing Pull",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Missing(a::What::Pull),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"missing","what":"pull"},"by":2}"#,
        ),
        (
            "missing Comment",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Missing(a::What::Comment),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"missing","what":"comment"},"by":2}"#,
        ),
        (
            "missing Label",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Missing(a::What::Label),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"missing","what":"label"},"by":2}"#,
        ),
        (
            "missing Branch",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Missing(a::What::Branch),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"missing","what":"branch"},"by":2}"#,
        ),
        (
            "missing Commit",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Missing(a::What::Commit),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"missing","what":"commit"},"by":2}"#,
        ),
        (
            "missing File",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Missing(a::What::File),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"missing","what":"file"},"by":2}"#,
        ),
        (
            "missing Page",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Missing(a::What::Page),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"missing","what":"page"},"by":2}"#,
        ),
        (
            "missing Review",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Missing(a::What::Review),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"missing","what":"review"},"by":2}"#,
        ),
        (
            "missing Job",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::CreateIssue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Missing(a::What::Job),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create_issue","number":null,"commit":18446744073709551615,"error":{"name":"missing","what":"job"},"by":2}"#,
        ),
        (
            "operation EditItem",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::EditItem,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"edit_item","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation SetDependencies",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::SetDependencies,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"set_dependencies","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation Comment",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::Comment,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"comment","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation EditComment",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::EditComment,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"edit_comment","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation DeleteComment",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::DeleteComment,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"delete_comment","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation SetLabels",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::SetLabels,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"set_labels","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation AddLabels",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::AddLabels,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"add_labels","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation RemoveLabels",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::RemoveLabels,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"remove_labels","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation DefineLabel",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::DefineLabel,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"define_label","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation OpenPull",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::OpenPull,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"open_pull","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation SetReviewers",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::SetReviewers,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"set_reviewers","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation Review",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::Review,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"review","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation Submit",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::Submit,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"submit","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation Merge",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::Merge,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"merge","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation Enqueue",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::Enqueue,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"enqueue","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation Update",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::Update,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"update","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation Close",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::Close,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"close","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation Reopen",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::Reopen,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"reopen","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation DeleteBranch",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::DeleteBranch,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"delete_branch","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation Status",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::Status,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"status","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation PutPage",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::PutPage,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"put_page","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation DeletePage",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::DeletePage,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"delete_page","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation Clone",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::Clone,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"clone","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation Fetch",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::Fetch,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"fetch","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation Push",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::Push,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"push","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "operation Create",
            Recorded::Observed {
                observation: Observation::Refused {
                    repository: boxed(b"org/repo"),
                    what: Operation::Create,
                    number: None,
                    commit: Some(u64::MAX),
                    error: a::Error::Forbidden,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"refused","repository":"org/repo","what":"create","number":null,"commit":18446744073709551615,"error":{"name":"forbidden"},"by":2}"#,
        ),
        (
            "first creation",
            Recorded::Observed {
                observation: Observation::Moved {
                    repository: boxed(b"org/repo"),
                    branch: boxed(b"work"),
                    from: None,
                    to: 1,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"moved","repository":"org/repo","branch":"work","from":null,"to":1,"by":2,"forward":null}"#,
        ),
        (
            "backward",
            Recorded::Observed {
                observation: Observation::Moved {
                    repository: boxed(b"org/repo"),
                    branch: boxed(b"work"),
                    from: Some(2),
                    to: 1,
                    by: 2,
                },
                forward: Some(false),
            },
            r#"{"observation":"moved","repository":"org/repo","branch":"work","from":2,"to":1,"by":2,"forward":false}"#,
        ),
        (
            "issue",
            Recorded::Observed {
                observation: Observation::Opened {
                    repository: boxed(b"org/repo"),
                    number: 1,
                    kind: a::Kind::Issue,
                    title: boxed(b""),
                    body: Box::new([255]),
                    labels: Box::new([]),
                    branches: None,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"opened","repository":"org/repo","number":1,"kind":"issue","title":"","body":"/w==","labels":[],"branches":null,"by":2}"#,
        ),
        (
            "wiki deletion",
            Recorded::Observed {
                observation: Observation::Wiki {
                    repository: boxed(b"org/repo"),
                    name: boxed(b"page"),
                    content: None,
                    revision: 9,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"wiki","repository":"org/repo","name":"page","content":null,"revision":9,"by":2}"#,
        ),
        ("checkpoint", Recorded::Lost(u64::MAX), r#"{"observations_lost":18446744073709551615}"#),
        (
            "Approve",
            Recorded::Observed {
                observation: Observation::Reviewed {
                    repository: boxed(b"org/repo"),
                    number: 7,
                    id: 7,
                    commit: 7,
                    verdict: a::Verdict::Approve,
                    body: boxed(b"body"),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"reviewed","repository":"org/repo","number":7,"id":7,"commit":7,"verdict":"approve","body":"Ym9keQ==","by":2}"#,
        ),
        (
            "Comment",
            Recorded::Observed {
                observation: Observation::Reviewed {
                    repository: boxed(b"org/repo"),
                    number: 7,
                    id: 7,
                    commit: 7,
                    verdict: a::Verdict::Comment,
                    body: boxed(b"body"),
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"reviewed","repository":"org/repo","number":7,"id":7,"commit":7,"verdict":"comment","body":"Ym9keQ==","by":2}"#,
        ),
        (
            "Pending",
            Recorded::Observed {
                observation: Observation::Reported {
                    repository: boxed(b"org/repo"),
                    commit: 7,
                    context: boxed(b"ci"),
                    state: a::Check::Pending,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"reported","repository":"org/repo","commit":7,"context":"ci","state":"pending","by":2}"#,
        ),
        (
            "Failed",
            Recorded::Observed {
                observation: Observation::Reported {
                    repository: boxed(b"org/repo"),
                    commit: 7,
                    context: boxed(b"ci"),
                    state: a::Check::Failed,
                    by: 2,
                },
                forward: None,
            },
            r#"{"observation":"reported","repository":"org/repo","commit":7,"context":"ci","state":"failed","by":2}"#,
        ),
        (
            "CiFailure",
            Recorded::Observed {
                observation: Observation::Dequeued {
                    repository: boxed(b"org/repo"),
                    number: 7,
                    reason: a::QueueRemoval::CiFailure,
                },
                forward: None,
            },
            r#"{"observation":"dequeued","repository":"org/repo","number":7,"reason":"ci_failure"}"#,
        ),
    ];
    for (name, record, line) in rows {
        let size = observations::encoded_len(&record).expect("UTF-8 row");
        let mut output = List::with_capacity(size);
        assert_eq!(observations::encode(&record, &mut output), Some(()), "{name}");
        let output = output.into_boxed();
        assert_eq!(output.last(), Some(&b'\n'), "{name}");
        assert_eq!(&output[..output.len().checked_sub(1).expect("newline")], line.as_bytes(), "{name}");
        assert_record(observations::decode(line.as_bytes()).expect(name), &record);
        assert_record(observations::decode_with_limits(line.as_bytes(), &fake_config().limits).expect(name), &record);
        let mut short = List::with_capacity(size.checked_sub(1).expect("nonempty line"));
        short.push(b'!').expect("prefix");
        assert_eq!(observations::encode(&record, &mut short), None, "{name}");
        assert_eq!(short.into_boxed().as_ref(), b"!", "{name}");
    }
}

#[test]
fn non_utf8_names_have_no_line() {
    let rows = [
        Observation::Moved { repository: boxed(b"repo"), branch: Box::new([255]), from: None, to: 1, by: 2 },
        Observation::Defined { repository: boxed(b"repo"), label: Box::new([255]), by: 2 },
        Observation::Closed { repository: Box::new([255]), number: 1, by: 2 },
        Observation::Opened {
            repository: boxed(b"repo"),
            number: 1,
            kind: a::Kind::Pull,
            title: boxed(b""),
            body: boxed(b""),
            labels: Box::new([Box::new([255])]),
            branches: None,
            by: 2,
        },
        Observation::Opened {
            repository: boxed(b"repo"),
            number: 1,
            kind: a::Kind::Pull,
            title: boxed(b""),
            body: boxed(b""),
            labels: Box::new([]),
            branches: Some(Branches { head: Box::new([255]), base: boxed(b"main"), commit: 1 }),
            by: 2,
        },
        Observation::Reported {
            repository: boxed(b"repo"),
            commit: 1,
            context: Box::new([255]),
            state: a::Check::Pending,
            by: 2,
        },
        Observation::Wiki { repository: boxed(b"repo"), name: Box::new([255]), content: None, revision: 1, by: 2 },
    ];
    for observation in rows {
        let record = Recorded::Observed { observation, forward: None };
        let mut output = List::with_capacity(4096);
        for byte in b"prefix" {
            output.push(*byte).expect("prefix fits");
        }
        assert_eq!(observations::encode(&record, &mut output), None);
        assert_eq!(observations::encoded_len(&record), None);
        assert_eq!(output.into_boxed().as_ref(), b"prefix");
    }
}

#[test]
fn refused_lines_name_the_member() {
    let rows = [
        (r#"{"observation":"closed","repository":"r","number":1,"by":2,"unknown":0}"#, "unknown", Reason::Unknown),
        (
            r#"{"repository":"r","repository":"s","number":1,"by":2,"observation":"closed"}"#,
            "repository",
            Reason::Repeated,
        ),
        (r#"{"repository":"r","number":1,"observation":"closed"}"#, "by", Reason::Missing),
        (r#"{"observation":"closed","repository":"r","number":1,"by":2,"forward":null}"#, "forward", Reason::Unknown),
        (
            r#"{"observation":"revised","repository":"r","number":1,"title":"%","body":"","by":2}"#,
            "title",
            Reason::Value,
        ),
        (r#"{"observation":"closed","repository":"r","number":18446744073709551616,"by":2}"#, "number", Reason::Value),
        (r#"{"observation":"closed","repository":4,"number":1,"by":2}"#, "repository", Reason::Type),
        (
            r#"{"observation":"opened","repository":"r","number":1,"kind":"pull","title":"","body":"","labels":[],"branches":{"head":"a","base":"b","commit":1,"extra":0},"by":2}"#,
            "extra",
            Reason::Unknown,
        ),
        (
            r#"{"observation":"refused","repository":"r","what":"push","number":null,"commit":null,"error":{"name":"rate_limited"},"by":2}"#,
            "reset",
            Reason::Missing,
        ),
        (
            r#"{"observation":"refused","repository":"r","what":"push","number":null,"commit":null,"error":{"name":"missing","what":"branch","what":"commit"},"by":2}"#,
            "what",
            Reason::Repeated,
        ),
        (r#"{"observation":"closed","repository":"r","number":-1,"by":2}"#, "number", Reason::Value),
        (r#"{"observation":"closed","repository":"r","number":1.0,"by":2}"#, "number", Reason::Value),
        (r#"{"observation":"closed","repository":"r","number":1e2,"by":2}"#, "number", Reason::Value),
        (r#"{"observation":"closed","repository":"r","number":1,"by":2,"by":3}"#, "by", Reason::Repeated),
        (
            r#"{"observation":"moved","repository":"r","branch":"b","from":null,"to":1,"by":2}"#,
            "forward",
            Reason::Missing,
        ),
        (
            r#"{"observation":"closed","repository":"r","number":1,"by":2,"k0":0,"k1":0,"k2":0,"k3":0,"k4":0,"k5":0,"k6":0,"k7":0,"k8":0}"#,
            "k8",
            Reason::Limit,
        ),
    ];
    for (line, member, reason) in rows {
        let refusal = observations::decode(line.as_bytes()).expect_err("refused row");
        assert_eq!(refusal.member.as_ref(), member.as_bytes(), "{line}");
        assert_eq!(refusal.reason, reason, "{line}");
        assert!(refusal.variant.is_some(), "known variant");
    }
}

#[test]
fn reordered_members_and_escaped_names_decode() {
    let line = br#"{"by":2,"branches":{"commit":7,"base":"main","head":"w\u006frk"},"labels":["b\u0075g"],"body":"AP8=","title":"dA==","kind":"pull","number":1,"repository":"o\u0072g/\"repo","observation":"opened"}"#;
    let expected = Recorded::Observed {
        observation: Observation::Opened {
            repository: boxed(b"org/\"repo"),
            number: 1,
            kind: a::Kind::Pull,
            title: boxed(b"t"),
            body: Box::new([0, 255]),
            labels: Box::new([boxed(b"bug")]),
            branches: Some(Branches { head: boxed(b"work"), base: boxed(b"main"), commit: 7 }),
            by: 2,
        },
        forward: None,
    };
    assert_record(observations::decode(line).expect("reordered object"), &expected);
}

#[test]
fn domain_limits_bound_the_reader() {
    let rows = [
        (r#"{"observation":"closed","repository":"ab","number":1,"by":2}"#, 2, 1, 1, 1, None),
        (r#"{"observation":"closed","repository":"abc","number":1,"by":2}"#, 2, 1, 1, 1, Some("repository")),
        (r#"{"observation":"labelled","repository":"r","number":1,"labels":["a"],"by":2}"#, 2, 1, 1, 1, None),
        (
            r#"{"observation":"labelled","repository":"r","number":1,"labels":["a","b"],"by":2}"#,
            2,
            1,
            1,
            1,
            Some("labels"),
        ),
        (r#"{"observation":"revised","repository":"r","number":1,"title":"","body":"/w==","by":2}"#, 2, 1, 1, 1, None),
        (
            r#"{"observation":"revised","repository":"r","number":1,"title":"","body":"//8=","by":2}"#,
            2,
            1,
            1,
            1,
            Some("body"),
        ),
        (r#"{"observation":"depends","repository":"r","number":1,"dependencies":[1],"by":2}"#, 2, 1, 1, 1, None),
        (
            r#"{"observation":"depends","repository":"r","number":1,"dependencies":[1,2],"by":2}"#,
            2,
            1,
            1,
            1,
            Some("dependencies"),
        ),
        (
            r#"{"observation":"opened","repository":"r","number":1,"kind":"pull","title":"","body":"","labels":[],"branches":{"head":"a","base":"b","commit":[[1]]},"by":2}"#,
            2,
            1,
            1,
            1,
            Some("commit"),
        ),
    ];
    for (line, name_bytes, labels, body_bytes, dependencies, refused) in rows {
        let mut limits = fake_config().limits;
        limits.name_bytes = name_bytes;
        limits.labels = labels;
        limits.body_bytes = body_bytes;
        limits.dependencies = dependencies;
        let result = observations::decode_with_limits(line.as_bytes(), &limits);
        match refused {
            Some(member) => assert_eq!(result.expect_err("past limit").member.as_ref(), member.as_bytes()),
            None => {
                result.expect("within limit");
            }
        }
    }
}

use skein_fake_forge_domain::{self as fake, Domain, promises};
use skein_lib::{Duration, Env, Queue, ReplyTo, Token, Wall};

struct World {
    domain: Domain,
    env: Env<fake::Config>,
    replies: Queue<fake::Request>,
}
impl World {
    fn new(branches: u32, observations: u32, repositories: u32) -> World {
        let mut config = fake_config();
        config.limits.branches = branches;
        config.limits.observations = observations;
        config.limits.repositories = repositories;
        config.latency_min = Duration::ZERO;
        config.latency_max = Duration::ZERO;
        let domain = Domain::new(&config, 1);
        World {
            domain,
            env: Env { now: Time::ZERO, wall: Wall::EPOCH, limits: config },
            replies: Queue::with_capacity(fake::MAX_OUT),
        }
    }
    fn repository(&mut self, name: &[u8], id: u32) -> u64 {
        let first = fake::repository(
            &mut self.domain,
            &self.env.limits,
            a::Setup {
                workflows: Box::new([]),
                id,
                name: boxed(name),
                default: boxed(b"main"),
                tree: Box::new([]),
                labels: Box::new([]),
                checks: a::Checks {
                    contexts: Box::new([]),
                    latency_min: Duration::ZERO,
                    latency_max: Duration::ZERO,
                    silent: 0,
                    passes: 1000,
                    reruns: 0,
                    cue: None,
                },
                protection: None,
                hooked: false,
            },
        );
        fake::grant(&mut self.domain, name, 2, a::Permission::Admin);
        first
    }
    fn call(&mut self, repository: &[u8], op: a::Op) {
        fake::step(
            &mut self.domain,
            &self.env,
            fake::Event::Call { reply_to: ReplyTo::new(Token::new(1)), user: 2, repository: boxed(repository), op },
            &mut self.replies,
        );
        for _ in 0..16_u32 {
            if let Some(fake::Request::Reply { result, .. }) = self.replies.pop() {
                result.expect("fixture write admitted");
                self.domain.reclaim();
                return;
            }
            fake::fire(&mut self.domain, &self.env, &mut self.replies);
        }
        panic!("fixture reply");
    }
    fn create(&mut self, repository: &[u8], branch: &[u8], commit: u64) {
        self.call(repository, a::Op::Git(a::Git::Push { branch: boxed(branch), commit, expected: None }));
    }
    fn delete(&mut self, repository: &[u8], branch: &[u8]) {
        self.call(repository, a::Op::Write(a::Write::DeleteBranch { branch: boxed(branch) }));
    }
    fn move_to(&mut self, repository: &[u8], branch: &[u8], commit: u64) {
        self.call(repository, a::Op::Git(a::Git::Push { branch: boxed(branch), commit, expected: None }));
    }
    fn commit(&mut self, parent: u64) -> u64 {
        fake::commit_tree(&mut self.domain, &self.env.limits, parent, None, Box::new([]), b"").expect("commit room")
    }
}

#[derive(Clone, Copy)]
enum MoveCase {
    Forward,
    Backward,
    Remake,
    Creation,
    Forgotten,
    RemakeBackward,
    Consumed,
    RepositoryIsolation,
    ZeroHistory,
}

#[test]
fn writer_computes_forward_from_the_domain_and_deletions() {
    let rows = [
        (MoveCase::Forward, Some(true)),
        (MoveCase::Backward, Some(false)),
        (MoveCase::Remake, Some(true)),
        (MoveCase::Creation, None),
        (MoveCase::Forgotten, None),
        (MoveCase::RemakeBackward, Some(false)),
        (MoveCase::Consumed, None),
        (MoveCase::RepositoryIsolation, Some(true)),
        (MoveCase::ZeroHistory, None),
    ];
    for (case, expected) in rows {
        let mut world = World::new(2, 64, 2);
        let first = world.repository(b"r", 1);
        let child = world.commit(first);
        let mut limits = world.env.limits.limits;
        // The zero-history row deliberately gives the writer zero retention;
        // the domain still needs branch room to produce the stream.
        if let MoveCase::ZeroHistory = case {
            limits.branches = 0;
        }
        let mut writer = observations::Writer::new(&limits);
        let mut discarded = List::with_capacity(16384);
        if let MoveCase::Backward = case {
            // Git refuses a non-fast-forward, so exercise the writer's
            // computation with the moved record over the real commit graph.
            let observation = Observation::Moved {
                repository: boxed(b"r"),
                branch: boxed(b"main"),
                from: Some(child),
                to: first,
                by: 2,
            };
            assert_eq!(writer.forward(&world.domain, &observation), expected);
            continue;
        }
        match case {
            MoveCase::Forward => world.move_to(b"r", b"main", child),
            MoveCase::Backward => unreachable!("backward row checked above"),
            MoveCase::Creation => world.create(b"r", b"work", child),
            MoveCase::Remake
            | MoveCase::RemakeBackward
            | MoveCase::Forgotten
            | MoveCase::Consumed
            | MoveCase::RepositoryIsolation
            | MoveCase::ZeroHistory => {
                world.create(b"r", b"work", child);
                world.delete(b"r", b"work");
                writer.drain(&mut world.domain, &mut discarded).expect("deletion drain");
                match case {
                    MoveCase::Forgotten => {
                        for branch in [b"other".as_slice(), b"last"] {
                            world.create(b"r", branch, first);
                            world.delete(b"r", branch);
                        }
                    }
                    MoveCase::Consumed => {
                        world.create(b"r", b"work", child);
                        writer.drain(&mut world.domain, &mut discarded).expect("consume deletion");
                        world.delete(b"r", b"work");
                        drop(world.domain.pop_observation());
                    }
                    MoveCase::RepositoryIsolation => {
                        let other = world.repository(b"s", 2);
                        for branch in [b"work".as_slice(), b"last"] {
                            world.create(b"s", branch, other);
                            world.delete(b"s", branch);
                        }
                    }
                    MoveCase::Forward
                    | MoveCase::Backward
                    | MoveCase::Remake
                    | MoveCase::Creation
                    | MoveCase::RemakeBackward
                    | MoveCase::ZeroHistory => {}
                }
                let target = match case {
                    MoveCase::RemakeBackward => first,
                    MoveCase::Remake => world.commit(child),
                    MoveCase::Forward
                    | MoveCase::Backward
                    | MoveCase::Creation
                    | MoveCase::Forgotten
                    | MoveCase::Consumed
                    | MoveCase::RepositoryIsolation
                    | MoveCase::ZeroHistory => child,
                };
                world.create(b"r", b"work", target);
            }
        }
        let mut output = List::with_capacity(16384);
        writer.drain(&mut world.domain, &mut output).expect("drain");
        let mut found = None;
        for record in decoded_lines(&output).into_boxed() {
            if let Recorded::Observed { observation: Observation::Moved { repository, .. }, forward } = record
                && repository.as_ref() == b"r"
            {
                found = Some(forward);
            }
        }
        assert_eq!(found, Some(expected));
    }
}

#[derive(Clone, Copy)]
enum CheckpointCase {
    Observations,
    Empty,
    LostOnly,
    NonUtf8,
    Stop,
    Cumulative,
}

#[test]
fn writer_checkpoints_are_cumulative_and_close_each_drain() {
    let rows = [
        (CheckpointCase::Observations, 1, Some(0)),
        (CheckpointCase::Empty, 0, None),
        (CheckpointCase::LostOnly, 0, Some(1)),
        (CheckpointCase::NonUtf8, 0, Some(1)),
        (CheckpointCase::Stop, 0, Some(0)),
        (CheckpointCase::Cumulative, 0, Some(2)),
    ];
    for (case, observations, checkpoint) in rows {
        let mut world = World::new(4, 1, 1);
        let first = world.repository(b"r", 1);
        let child = world.commit(first);
        let mut writer = observations::Writer::new(&world.env.limits.limits);
        match case {
            CheckpointCase::Observations => world.move_to(b"r", b"main", child),
            CheckpointCase::Empty | CheckpointCase::Stop => {}
            CheckpointCase::LostOnly => {
                world.move_to(b"r", b"main", child);
                world.move_to(b"r", b"main", first);
                drop(world.domain.pop_observation());
            }
            CheckpointCase::NonUtf8 => world.create(b"r", &[255], child),
            CheckpointCase::Cumulative => {
                world.create(b"r", &[255], child);
                let mut discarded = List::with_capacity(1024);
                writer.drain(&mut world.domain, &mut discarded).expect("first omission");
                world.move_to(b"r", b"main", child);
                world.move_to(b"r", b"main", first);
                drop(world.domain.pop_observation());
            }
        }
        let mut output = List::with_capacity(4096);
        if let CheckpointCase::Stop = case {
            writer.stop(&mut world.domain, &mut output).expect("stop");
        } else {
            writer.drain(&mut world.domain, &mut output).expect("drain");
        }
        let mut observed = 0_u32;
        let mut last = None;
        let mut checkpoints = 0_u32;
        for record in decoded_lines(&output).into_boxed() {
            match record {
                Recorded::Observed { .. } => {
                    assert!(last.is_none(), "checkpoint ends the drain");
                    observed = observed.checked_add(1).expect("test observations");
                }
                Recorded::Lost(n) => {
                    checkpoints = checkpoints.checked_add(1).expect("test checkpoints");
                    last = Some(n);
                }
            }
        }
        assert_eq!(observed, observations);
        assert_eq!(last, checkpoint);
        assert_eq!(checkpoints, u32::from(checkpoint.is_some()));
        if checkpoint.is_some() {
            assert_eq!(output.as_slice().last(), Some(&b'\n'));
        }
        let mut idle = List::with_capacity(1024);
        writer.drain(&mut world.domain, &mut idle).expect("idle drain");
        assert!(idle.is_empty(), "idle drain appends nothing");
    }
}

#[test]
fn writer_output_is_the_promises_judges_input() {
    for forced in [false, true] {
        let mut world = World::new(2, 64, 1);
        let first = world.repository(b"r", 1);
        let child = world.commit(first);
        world.create(b"r", b"owned/work", child);
        if forced {
            world.delete(b"r", b"owned/work");
            world.create(b"r", b"owned/work", first);
        } else {
            let grandchild = world.commit(child);
            world.move_to(b"r", b"owned/work", grandchild);
        }
        let mut writer = observations::Writer::new(&world.env.limits.limits);
        let mut output = List::with_capacity(4096);
        writer.stop(&mut world.domain, &mut output).expect("complete output");
        let mut judge = promises::Judge::new(promises::Profile {
            client: 2,
            marker: (boxed(b"<!--key:"), boxed(b"-->")),
            owned_branches: Box::new([boxed(b"owned/")]),
            allowed: Box::new([]),
            bounds: promises::Bounds { objects: 4, decisions: 1 },
        });
        judge.decided();
        for record in decoded_lines(&output).into_boxed() {
            judge.observe(&record);
        }
        let expected = if forced {
            promises::Verdict::Fail(Box::new([promises::Breach {
                promise: promises::Promise::Forced,
                subject: boxed(b"r/owned%2Fwork"),
            }]))
        } else {
            promises::Verdict::Pass
        };
        assert_eq!(judge.verdict(), expected);
    }
}

#[test]
fn writer_resumes_without_losing_a_line_when_the_buffer_fills() {
    let line = b"{\"observation\":\"moved\",\"repository\":\"r\",\"branch\":\"main\",\"from\":1,\"to\":2,\"by\":2,\"forward\":true}\n";
    for capacity in [0, u32::try_from(line.len()).expect("line length")] {
        let mut world = World::new(2, 64, 1);
        let first = world.repository(b"r", 1);
        let child = world.commit(first);
        world.move_to(b"r", b"main", child);
        let mut writer = observations::Writer::new(&world.env.limits.limits);
        let mut full = List::with_capacity(capacity);
        assert_eq!(writer.drain(&mut world.domain, &mut full), Err(skein_lib::Overflow));
        let mut output = List::with_capacity(4096);
        for byte in &full {
            output.push(*byte).expect("prefix room");
        }
        writer.drain(&mut world.domain, &mut output).expect("resume");
        let mut expected = List::with_capacity(4096);
        for part in [line.as_slice(), b"{\"observations_lost\":0}\n"] {
            for byte in part {
                expected.push(*byte).expect("expected room");
            }
        }
        assert_eq!(output, expected);
    }
}

fn decoded_lines(output: &List<u8>) -> List<Recorded> {
    let mut records = List::with_capacity(output.len());
    let mut start = 0_usize;
    for (end, byte) in output.as_slice().iter().enumerate() {
        if *byte == b'\n' {
            let line = output.as_slice().get(start..end).expect("line range");
            records.push(observations::decode(line).expect("writer line")).expect("one record per newline");
            start = end.checked_add(1).expect("bounded output");
        }
    }
    assert_eq!(start, output.as_slice().len(), "every line has its newline");
    records
}

#[test]
fn rebase_provenance_reaches_the_judge_across_encoded_checkpoints() {
    use skein_fake_forge_domain::promises::{Bounds, Breach, Judge, Profile, Promise, Verdict};
    for caller in [2, 3] {
        let mut judge = Judge::new(Profile {
            client: 2,
            marker: (boxed(b"<key>"), boxed(b"</key>")),
            owned_branches: Box::new([]),
            allowed: Box::new([]),
            bounds: Bounds { objects: 4, decisions: 1 },
        });
        for record in [
            Recorded::Observed {
                observation: Observation::Moved {
                    repository: boxed(b"org/repo"),
                    branch: boxed(b"work"),
                    from: None,
                    to: 3,
                    by: 2,
                },
                forward: None,
            },
            Recorded::Observed {
                observation: Observation::Rebased {
                    repository: boxed(b"org/repo"),
                    branch: boxed(b"work"),
                    from: 3,
                    to: 7,
                    by: caller,
                },
                forward: None,
            },
            Recorded::Lost(0),
            Recorded::Observed {
                observation: Observation::Moved {
                    repository: boxed(b"org/repo"),
                    branch: boxed(b"work"),
                    from: Some(3),
                    to: 7,
                    by: caller,
                },
                forward: Some(false),
            },
            Recorded::Lost(0),
        ] {
            let mut line = List::with_capacity(512);
            observations::encode(&record, &mut line).unwrap();
            let decoded = observations::decode(line.as_slice().strip_suffix(b"\n").unwrap()).unwrap();
            judge.observe(&decoded);
        }
        judge.decided();
        let expected = if caller == 2 {
            Verdict::Pass
        } else {
            Verdict::Fail(Box::new([Breach { promise: Promise::Foreign, subject: boxed(b"org%2Frepo/branch/work") }]))
        };
        assert_eq!(judge.verdict(), expected, "caller {caller}");
    }
}
