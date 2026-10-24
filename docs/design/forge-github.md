# GitHub's dialect

Provisional, 2026-10-20. `skein_forge::github`: GitHub's documents and
call plans, beside Forgejo's (`skein_forge::forgejo`), holding what
forge.md 4.1 says a dialect holds. It renders forge.md's vocabulary on
github.com's REST API at one API version, with three GraphQL documents
for what REST lacks, and adds to the vocabulary what GitHub needs
(forge.md, sections 3 and 10). The component that runs it is
forge-connection.md's; the setup plans that run its setup documents are
forge-setup.md's; the fake's GitHub layer and GitHub's conformance are
fake-forge.md's, sections 10 and 11.

## 1. In one page

- **A module of `skein-forge`,** `skein_forge::github`, with no io, no
  clock and no connection: encoders, decoders, call plans, the
  failures' map, deliveries' decoders, and the App's documents (the
  JSON Web Token's bytes, the mint, installations, the delivery log).
- **Pinned to an API version,** `X-GitHub-Api-Version: 2026-03-10`, on
  every request (section 3). GitHub is a service, not a release: the
  pin is the version and the date the conformance last held, and a fact
  that changes is a finding (section 13).
- **REST, and three GraphQL documents:** the bot's identity (`viewer`),
  entering the merge queue (`enqueuePullRequest`) and a pull request's
  landed commit and queue entry (`mergeCommit`, `mergeQueueEntry`).
- **Every call of the vocabulary is answered** (section 4), with
  GitHub's shapes told in each decoder: pull requests among issues,
  `merged` absent from listings, `mergeable` null until computed, pages
  by `Link`, CI from check runs and statuses read apart.
- **A merge tries REST first** (section 5): `PUT .../merge` with `sha`,
  and on GitHub's merge-queue refusal it enters the queue with
  `expectedHeadOid` and answers `Queued`. A moved head is `Stale` on
  either route. The plan keeps no state, so it holds when a branch's
  rules change after adoption.
- **Protection is read from rulesets and classic protection,** both,
  and neither's absence is taken for the other's; it says whether the
  branch queues, wants signatures, a linear history or code owners, and
  whether the App may bypass.
- **Refusals are told apart by status and GitHub's whole message,**
  since several share a status (section 7); secondary limits are
  `RateLimited`, and every answer's `x-ratelimit-*` is the rate's
  window (`api::Rate`).
- **The App's documents:** the JSON Web Token's signing input and its
  assembly (the signature is the component's, `skein_tls::sign_rs256`),
  the mint of an installation's token, the installations and their
  repositories, and the start's identity read (section 8).
- **The delivery log is the hint feed:** its list page, with a cursor
  and a validator, and one delivery whole; a pushed delivery and a
  logged one decode to the same `github::Delivery` (section 9).
- **A job's log is a redirect,** parsed into `Redirect { origin, target
  }`; the component follows it to an origin it knows (section 10).
- **Setup's documents:** the manifest, the form, the install page, the
  conversion, the App's own reads and its hook (section 11). The plans
  that run them are forge-setup.md's.
- **Ids are exact,** `u64` in decimal; a delivery's id has 19 digits.

## 2. What the module holds

By forge.md 4.1's list:

- **Each request,** as a sized encoder, and **each answer,** as a
  decoder over skein-json's tokens with a bounded stack, keeping only
  its fields (sections 3, 4 and 6).
- **The server's side of each document,** for the fake's GitHub layer:
  the decoder of each request and the encoder of each answer
  (fake-forge.md, section 10).
- **The failures** and the map from a status and a message to an
  `Error` (section 7).
- **The deliveries:** the headers, the events, each payload's decoder
  and encoder, the table to hints, and the log's documents (section 9).
- **The setup's documents** (section 11).
- **Names, times and ids** (section 3).
- **How a credential renders:** `Authorization: Bearer <t>`, for a
  token, an installation's token and the App's JSON Web Token alike.
- **The start's reads:** the identity read (section 8.4); the page cap
  is GitHub's constant 100 (`PAGE_CAP`), so no read is made for it.
- **The renewal:** the mint's request and decoder, and the JSON Web
  Token's bytes (section 8). When it is due, and which installation, is
  the component's (forge-connection.md, section 5).
- **The pin** (section 12), **the call plans** (sections 4 and 5) and
  **its limits,** `github::Limits`, with forge.md 4.1's fields; `check`
  refuses a zero, a `json.string` below `string_bytes` or `body_bytes`,
  and a depth below 10 (a logged delivery wraps its payload in two more
  levels than a pushed one).

The plans hear `github::Head`: the status and the header fields `Date`,
`Retry-After`, `Link` (whether it has `rel="next"`, and that link's
`cursor`), `ETag`, `Location`, `Content-Type` (JSON or not),
`x-ratelimit-limit`, `x-ratelimit-remaining`, `x-ratelimit-reset`,
`x-ratelimit-resource` and `Connection: close`. Each else as forge.md
4.2 says: `Row`, `Done` with the body, or `Failed` (`Unseen`, `Seen`).

The plans' tables are the module's own, which the component keeps and
hands them as it does Forgejo's (forge.md, 4.2):

- **A repository's row:** its `owner` and `name` (its path), its
  owner's account id (by which the component chooses the installation,
  forge-connection.md, section 5), whether that owner is an
  organization, the merge method (5.1) and its `html_url`. Object ids
  are SHA-1's, 40 digits: GitHub has no other format.
- **A user's row:** an id and its login.

## 3. Conventions

- **Base and transport.** Every request goes to the forge's `api`
  address, by default `api.github.com:443` over TLS, at the paths below
  with no prefix; GraphQL is `POST /graphql`. The fake's GitHub layer
  answers the same paths in plaintext on loopback (fake-forge.md,
  section 10). `Host` is made by `skein-forge`'s `host` (forge.md, 5.1).
- **Every request's head** carries `Accept: application/vnd.github+json`
  (a raw file's `application/vnd.github.raw+json`),
  `X-GitHub-Api-Version: 2026-03-10`, the owner's `User-Agent`
  (GitHub refuses a request with none), and `Authorization: Bearer <t>`
  but where a row says no credential. `skein-forge`'s owner gives a
  `User-Agent` that is not empty, checked at start.
- **JSON in and out,** as forge.md 5.1. A GraphQL request is
  `{"query": <document>, "variables": {...}}`, the document a constant
  of the module (section 4.3).
- **Repositories by id.** R is `/repos/{owner}/{name}`, from the
  repository's row. A call whose repository has no row resolves it
  first with `GET /repositories/{id}`, which counts in its cost.
- **Names in paths** are percent-encoded per segment. A branch keeps its
  slashes as the path's: GitHub takes the rest of the path as the
  branch (`branches/{branch}`, `git/refs/heads/{branch}`), recorded as
  `branch-with-slash-reads` (section 13). A query's values are
  percent-encoded whole (`head=<owner>:<branch>`). A bot's login in a
  path is `<slug>%5Bbot%5D`.
- **Times** are RFC 3339 in UTC (`2026-10-20T08:00:00Z`), read to the
  second, as the forge's clock. `since` is written so, up to 2554 as
  forge.md 5.1 says. An answer's time is its `Date`.
- **Commits** are 40 lowercase hexadecimal digits, carried as forge.md's
  32 bytes, zero-padded.
- **Ids** (users, repositories, comments, reviews, check runs, jobs,
  runs, installations, deliveries, the App) are decimal `u64`s, read
  exactly; a number past `u64` is a malformed answer. A repository id
  past the vocabulary's 32 bits fails the call `TooLarge`, as Forgejo's
  (section 15).
- **Paging is explicit:** `per_page=<size>&page=<n>`, the size the
  smallest of the client's `rows`, `PAGE_CAP` (100), and the number of
  the listing's largest rows `answer_bytes` holds (forge.md, 5.1). A
  listing has more when its `Link` has `rel="next"`; a page that comes
  full without one has none. The delivery log pages by cursor (9.3).
- **Users by id,** with the login from the users' table, filled by `GET
  /user/{id}` (`user-by-id-reads`), one request in the cost.

## 4. Each operation

### 4.1 Reads

R as in section 3; `<size>` the page size; `GQ` a GraphQL request
(4.3). Several requests run in turn on one connection, as forge.md 5.2.
"Every page" in Pull's and Checks' rows stops at 50 pages a listing:
the call fails `TooLarge` when a 50th page still has a next.

| Call | Requests | Answer from |
|---|---|---|
| `Read::Items { since, page, kind }` | `GET R/issues?state=all[&since=<since>]&sort=updated&direction=asc&per_page=<size>&page=<page>` | omit `since` at zero: an epoch `since` lists nothing; otherwise send the UTC bound, inclusive. Each row a `Summary`: `number`; a `pull_request` member makes it a pull, and its `merged_at` makes one closed merged; `state`; `user.id`; `labels[].name`; `updated_at`; `title`; `body` (null as empty) and its key. A row of the other kind than the call's is skipped, and counts toward the page. Rows of one second are put in order of number before the page answers. `more` from `Link`; `now` from `Date` |
| `Read::Item { number, after }` | `GET R/issues/<number>`; `GET R/issues/<number>/comments?per_page=<size>&page=<n>` from page 1, until the answer is full or a page has no next | the issue as a `Summary`; its comments with ids above `after`, oldest first, as forge.md 5.2's: `id`, `user.id`, `created_at`, `body` and its key, provenance, revision; `more` when one is left |
| `Read::Pull { number }` | `GET R/pulls/<number>`; `GET R/commits/<head.sha>/status?per_page=1`; `GET R/commits/<head.sha>/check-runs?filter=latest&per_page=<size>&page=<n>`, every page; `GQ pull` when the pull is open and the repository's owner an organization, or merged (`merge_commit_sha` is absent at the pin) | `number`; `state`, `merged` making it merged; `head.ref`, `base.ref`, `head.sha`, `base.sha`; `merged` from `merge_commit_sha`, else `mergeCommit.oid`; `mergeable`, null as false; `requested_reviewers[].id`; `queued` from `mergeQueueEntry.position`; `dequeued` from the last `RemovedFromMergeQueueEvent`'s `reason`; `Ci` from both (4.4) |
| `Read::PullFor { head, base }` | `GET R/pulls?state=open&head=<owner>:<head>&base=<base>&per_page=1`; when none, `GET R/pulls?state=all&head=<owner>:<head>&base=<base>&sort=created&direction=asc&per_page=1`; then as `Pull` for the number found | the open pull request for the head and base, else the oldest; `Missing` when there is none. `<owner>` is the repository's own: a fork's head is not followed (temper's draft 18) |
| `Read::Reviews { number, page }` | `GET R/pulls/<number>/reviews?per_page=<size>&page=<page>` | as forge.md 5.2; `state` mapped `APPROVED` to `Approve`, `CHANGES_REQUESTED` to `RequestChanges`, `COMMENTED` and `DISMISSED` to `Comment`; `PENDING` and a null `submitted_at` skipped, counting toward the page; `commit_id`; `official` when `author_association` is `OWNER`, `MEMBER` or `COLLABORATOR` (section 15) |
| `Read::Statuses { commit, page }` | `GET R/commits/<commit>/status?per_page=<size>&page=<page>` | `Ci` from `state` and `total_count`, 0 being `None`; each `statuses[]` row: `updated_at`, `context`, `state`, `description`, `target_url`; no job; `more` from `Link` |
| `Read::Remarks { number, review, page }` | `GET R/pulls/<number>/reviews/<review>/comments?per_page=<size>&page=<page>` | `id`, `user.id`, `path`, `line`, else `original_line`, else 0, `body`; `more` from `Link` |
| `Read::Branch { branch }` | `GET R/branches/<branch>` | `commit.sha` |
| `Read::Branches` | `GET R/branches?per_page=<size>&page=<n>`, from 1 until no next | every `name` within `string_bytes`; a longer one skipped; past the answer's bound, `TooLarge` |
| `Read::PullFiles { number, head, page }` | `GET R/pulls/<number>`; `GET R/compare/<base.sha>...<head.sha>?per_page=1`, for `merge_base_commit.sha`; `GET R/pulls/<number>/files?per_page=<size>&page=<page>`; for each file, the raw read (below) at the merge base, at `previous_filename` for `renamed`, but for `added`, and at the head but for `removed` | as forge.md 5.2; a `head.sha` other than `head` is `Stale` before any file is read; `more` from `Link` |
| `Read::Compare { before, after }` | `GET R/compare/<before>...<after>?per_page=<size>`; for each file, the raw reads at `before` and `after` | `files[].filename` with its sides; `commits[].sha`; `contains_before` when `behind_by` is 0; `TooLarge` when the listed commits are fewer than `total_commits`, or `files` holds 300, GitHub's cap (`compare-files-capped-300`) |
| `Read::Checks { commit }` | `GET R/commits/<commit>/status?per_page=<size>&page=<n>`, every page; `GET R/commits/<commit>/check-runs?filter=latest&per_page=<size>&page=<n>`, every page; for each failed check run of GitHub Actions, `GET R/actions/jobs/<id>` | each status as for `Statuses`; each check run as a `Status` (4.4), its `job` the commit, the run from `run_id`, the job its `id` and the attempt its `run_attempt` for an Actions run that failed, none otherwise |
| `Read::Job { attempt, max_bytes }` | `GET R/actions/jobs/<job>`; `GET R/actions/jobs/<job>/logs`, answered `302`; the redirect followed (section 10) | the failed step: the first of `steps[]` whose `conclusion` is `failure`, its `number` and `name`; the log's last `max_bytes` bytes (10); `MissingJob` on a `404` or `410`, or when `run_id`, `run_attempt` or `head_sha` is not the attempt's |
| `Read::File { head, path, max_bytes }` | the raw read of `path` at `head` | the first `max_bytes` bytes, `truncated` when more came; `Missing` on a `404`, and on a JSON answer (a directory's listing) |
| `Read::Protection { branch }` | `GET R/rules/branches/<branch>?per_page=100&page=<n>`, every page; for each ruleset named, `GET R/rulesets/<ruleset_id>`; `GET R/branches/<branch>/protection` | section 5.3 |
| `Read::Settings` | `GET /repositories/<id>` | `default_branch`; `allow_merge_commit`, `allow_squash_merge`, `allow_rebase_merge`; the row kept afresh |
| `Read::Collaborators { page }` | `GET R/collaborators?affiliation=all&per_page=<size>&page=<page>` | each `id` and `role_name` as below; `more` from `Link` |
| `Read::Permission { user }` | for the bot (the identity read's `databaseId`, 8.4), none; otherwise `GET R/collaborators/<login>/permission` | the bot's from its installation (8.2): `Write` when the full mint's `permissions` hold `contents: write` and `pull_requests: write`, `Read` when they hold `contents` at `read` or above, `None` otherwise; another user's from `role_name`: `admin` and `maintain` `Admin`, `write` `Write`, `triage` and `read` `Read`, `none` and the empty string `None` |
| `Read::Locate { owner, name }` | `GET /repos/<owner>/<name>` | `id`; the row kept |

The dropped rows are names the client can't hold, such as queue branches
past the bound; the client makes no name past `string_bytes`, so it never
looks for a branch it dropped (`Read::Branches`, 5.3's row).

- **Compare pages commits, not files.** Its page size is the least of
  `rows`, `PAGE_CAP` (100), and `answer_bytes` / 32, the commit id's
  measure. The first page carries the files for the whole comparison,
  capped at 300; later commit pages carry no files. The plan asks for
  one page and refuses fewer commits than `total_commits`, so it never
  answers a partial comparison. The file sides and commit ids together
  must fit the answer bound. `contains_before` comes from `behind_by == 0`,
  including for a diverged comparison; a nonzero `ahead_by` alone does not
  show containment. PullFiles asks for only the merge base and keeps
  `per_page=1`.
- **The raw read** is `GET R/contents/<path>?ref=<commit>` with `Accept:
  application/vnd.github.raw+json`, the path's slashes kept: a `200`
  that is not JSON is the file's bytes, a `404` an absent side.
- **No `Range`.** skein sends none: a file and a log are read through,
  and cut as they stream (section 6). The signed job-log blob honours
  Range; skein requests the whole body.
- **The keep-up listing** is as Forgejo's (forge.md, 5.2), in GitHub's
  words: `sort=updated&direction=asc`. A new issue can be missing from
  the first listing after it (`new-issue-may-miss-listing`): the next
  pass, from the same `since`, finds it, since `since` is inclusive
  (`issues-since-inclusive`).
- **An item's comments are paged by GitHub,** oldest first: the call
  reads pages from the first and keeps those above `after`; each page
  counts in its cost (section 15).

### 4.2 Writes

| Call | Requests | Answer from |
|---|---|---|
| `Write::CreateIssue { key, title, body }` | `POST R/issues` `{title, body}`, the body behind the key's marker | `Created` from `number` (`201`) |
| `Write::OpenPull { title, body, head, base }` | `POST R/pulls` `{title, body, head, base}` | `Created` from `number` (`201`); a `422` "No commits between ..." `NothingToMerge`, and "A pull request already exists for ..." `Exists` (section 7) |
| `Write::Post { number, key, body }` | `POST R/issues/<number>/comments` `{body}`, behind the marker | `Commented` from `id` (`201`) |
| `Write::Review { number, key, verdict, body }` | `POST R/pulls/<number>/reviews` `{event, body}`, `event` `APPROVE`, `REQUEST_CHANGES` or `COMMENT`, behind the marker | `Reviewed` from `id` (`200`) |
| `Write::Edit { number, title, body }` | `PATCH R/issues/<number>` `{title, body}`, each only when given | `Done` (`200`) |
| `Write::SetReviewers { number, reviewers }` | `GET R/pulls/<number>`; `POST R/pulls/<number>/requested_reviewers` `{reviewers: [logins]}` for those missing (`201`); `DELETE` the same with those extra (`200`) | `Done` |
| `Write::Close { number }`, `Write::Reopen { number }` | `PATCH R/issues/<number>` `{state: closed\|open}` | `Done` (`200`) |
| `Write::Merge { number, head }` | section 5.1 | `Merged`, or `Queued` |
| `Write::Update { number }` | `GET R/pulls/<number>`; `PUT R/pulls/<number>/update-branch` `{expected_head_sha: <head.sha>}` | `Done` (`202`); section 5.2 |
| `Write::Status { commit, context, check }` | `POST R/statuses/<commit>` `{state: pending\|success\|failure, context}` | `Done` (`201`); `Forbidden` where the App has no `statuses` write (below) |
| `Write::CreateBranch { branch, commit }` | `POST R/git/refs` `{ref: refs/heads/<branch>, sha}` | `Branch(Created)` (`201`); a `422` "Reference already exists" `Branch(Exists)` |
| `Write::DeleteBranch { branch }` | `DELETE R/git/refs/heads/<branch>` | `Done` (`204`); a `422` "Reference does not exist" `Missing` |

- **Opening asks GitHub,** which refuses a head with nothing to merge
  and a second open pull request itself, each with its own message: one
  request, where Forgejo's compares first (forge.md, 5.2).
- **A status needs `statuses` write.** The fixture App's installation token
  receives `Forbidden`, GitHub's `403` "Resource not accessible by integration";
  the fixture person tokens receive `403` "Resource not accessible by personal
  access token". The queue cases use the fixture workflow's check run instead
  (section 16). This is the call GitHub answers otherwise than Forgejo
  (forge.md, 3.7).
- **Keys and markers** are as Forgejo's (forge.md, section 6).

### 4.3 The GraphQL documents

Constants of the module, each sent as `POST /graphql` with its
variables, on the installation's token. A GraphQL answer is `200` with
`data`, or with `errors[]`, each with a `type` and a `message`; an
answer with `errors` is a refusal (section 7).

- **`viewer`:** `query { viewer { login databaseId } }`, the identity
  (8.4).
- **`pull`:** `query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) { pullRequest(number: $number)
  { id mergeCommit { oid } mergeQueueEntry { position } timelineItems(last:
  1, itemTypes: [REMOVED_FROM_MERGE_QUEUE_EVENT]) { nodes { ... on
  RemovedFromMergeQueueEvent { reason } } } } } }`, a pull request's node
  id, its landed commit, its queue position and why the queue last
  ejected it.
- **`enqueue`:** `mutation($id: ID!, $head: GitObjectID!) {
  enqueuePullRequest(input: {pullRequestId: $id, expectedHeadOid:
  $head}) { mergeQueueEntry { position } } }`.

Batched reads are later (temper's draft 18).

### 4.4 CI

- **A check run as a `Status`:** `completed_at`, else `started_at`, as
  its time; `name` as its context; `output.title` as its description;
  `html_url` as its link. Its `Check`: a `conclusion` of `success`,
  `neutral` or `skipped` is passed; `failure`, `timed_out`,
  `cancelled`, `action_required`, `startup_failure` or `stale` failed;
  a run not `completed` (`queued`, `in_progress`, `waiting`,
  `requested`, `pending`) pending.
- **A status's state:** `success` passed, `failure` and `error` failed,
  `pending` pending.
- **`Ci` on a pull request** is both read apart: failed when any status
  or check run failed, else pending when any is pending, else passed
  when any is there, else `None`. The combined status's `pending` with
  `total_count` 0 is no status (`combined-pending-with-no-status`), and
  another App's suite with no run lists no check run.
- **Which checks are CI** is the adoption's, the client's: the branch's
  required checks when its rules name them, else the Actions workflows'
  (temper's draft, 9). The dialect reads them all.
- **An Actions job** is the check run whose `app.slug` is
  `github-actions`; its id is the job's.

## 5. Merging, updating, and the rules

### 5.1 The merge

1. `PUT R/pulls/<number>/merge` `{sha: <head>, merge_method: <the
   row's method>}`. A `200` answers `Merged` from `sha`.
2. A `405` whose message names the merge queue ("Changes must be made
   through the merge queue", section 7): `GQ pull`, for the node id;
   then `GQ enqueue` with `expectedHeadOid` the call's `head`. Its entry
   answers `Queued`.
3. Every other refusal is section 7's.

- **A row with no merge method** makes `Merge` answer `Refused` before
  anything is sent.
- **A lost enqueue is never resent.** When nothing is heard from `GQ
  enqueue` the call is a `Timeout`, and the client reads the pull: an
  observed `queued` entry is adopted as the call's `Queued`.

- **REST first, always.** The plan keeps nothing between calls, so it
  learns of a queue by GitHub's refusal, which costs one request on a
  queued branch and holds when the rules change after adoption: a
  branch that came to require the queue is entered, and one that
  stopped is merged. The latest `Protection` saying `queue` is the
  client's to show; the plan never takes it for GitHub's answer.
- **A repeat merge on a merged pull** answers `200`, `merged: true`, with
  the same merge sha, even when the supplied head sha differs. A REST pull
  read omits `merge_commit_sha` at the pinned 2026-03-10 version, for open
  and merged pulls alike; 2022-11-28 sends it. Every merged `Read::Pull`
  therefore costs the additional `GQ pull` read for `mergeCommit.oid`.
  The conformance compares that plan answer with both PUTs' sha.
- **An open pull's moved head is `Stale`** on both routes: `409` "Head branch was
  modified. Review and try the merge again." at the merge, and
  `UNPROCESSABLE` "... expected head oid does not match the current head
  oid" at the queue.
- **`Queued` is final for the call.** The landing, or the ejection,
  comes later: the log's `pull_request` `dequeued`, with its reason, and
  `merge_group` `destroyed` (section 9) hint the item; the client reads
  the pull, whose `merged` is the group's commit, from `mergeCommit`
  (4.1). An ejected pull is open with no `queued`, and its `dequeued`
  holds the queue's reason, as GitHub words it: a hint carries no
  payload, so the reason is read with the pull.
- **The method** is the row's: the first of merge, squash and rebase
  that the repository allows (`allow_merge_commit`,
  `allow_squash_merge`, `allow_rebase_merge`). A queue merges by its own
  method.
- **The merge's answer carries its commit** (`sha`): no read follows,
  as Forgejo's must (forge.md, 5.2).

### 5.2 The update

`Update` reads the pull's head, then sends it as `expected_head_sha`, so
a push between the two is refused (`422` "expected head sha didn’t
match current head ref.", `Stale`, the apostrophe U+2019 as GitHub
writes it). GitHub merges the base into the head later (`202`), and the
move is heard as any push. A head the client saw would need `Update` to
carry it (section 15).

### 5.3 The protection

`Protection` reads both sources GitHub applies, and takes neither's
absence for the other's (`classic-protection-404-under-rulesets`):

- **Rulesets:** `rules/branches/<branch>` lists the rules that apply,
  each with its `ruleset_id`; each ruleset is read once for its
  `current_user_can_bypass` (`always`, `pull_requests_only`, `never`),
  the App's own, since the bypass list is not readable
  (`rules-branches-current-user-can-bypass`). The rules kept:
  `pull_request` (`required_approving_review_count`,
  `dismiss_stale_reviews_on_push`, `require_code_owner_review`),
  `required_status_checks` (each `context`), `merge_queue`,
  `required_signatures`, `required_linear_history`. Others are skipped.
- **Classic protection:** `branches/<branch>/protection`, a `404`
  "Branch not protected" being none: `required_status_checks.contexts`,
  `required_pull_request_reviews` (`required_approving_review_count`,
  `dismiss_stale_reviews`, `require_code_owner_reviews`),
  `required_signatures.enabled`, `required_linear_history.enabled`.
- **One `Protection`,** none when neither source has a rule kept:
  `branch` the call's; `contexts` the union, in order of first seen;
  `approvals` the largest; `dismiss_stale`, `code_owners`, `signatures`,
  `linear` and `queue` each true when any source says so; `bypass` the
  least of every ruleset with a rule kept, classic protection counting
  as `Never`.
- **Its cost:** two requests and one per ruleset named, bounded by
  `inner`; a branch under more rulesets than `inner` is `TooLarge`.

## 6. Decoding answers

As forge.md 5.3: streams, strings kept whole or not, the answer's
bound, revisions, markers and malformed answers; and GitHub's:

- **Provenance** from `created_at` and `updated_at` (a review's
  `submitted_at` and none: a review is `Original` unless its body was
  not kept), at a second's resolution (`comment-updated-at-on-edit`).
- **Raw bodies cut as they stream.** A file's first `max_bytes` bytes
  and a log's last are kept within what is left of `answer_bytes`, and
  the rest read through within `body_bytes_in`
  (forge-connection.md, section 7); `truncated` says so.
- **Absent headers.** An `Items` answer without `Date` is malformed. A
  listing without `Link` has no next. An answer without
  `x-ratelimit-*` carries no `Rate`, as the JSON Web Token's never do.
- **Nulls** read as absent: a body, a fork's head repository, a
  `mergeable`, an `action`, a `line`.
- **GraphQL's ids** (`databaseId`, `position`) are JSON numbers, and its
  node ids opaque strings within `string_bytes`, sent back as they came.

## 7. Failures

| What happened | `Error` |
|---|---|
| nothing written, as forge.md 5.4 | `Unavailable` |
| a `301` or `307` on a repository's route (renamed or moved): the row dropped | `Unavailable` |
| a `403` or `429` with `x-ratelimit-remaining` 0, before any write succeeded | `RateLimited { after }`: `Retry-After` when given, else `x-ratelimit-reset` less `Date` |
| a `403` or `429` "You have exceeded a secondary rate limit", or with `Retry-After`, before any write succeeded | `RateLimited { after }`: `Retry-After`, else 60 seconds |
| on `SetReviewers`, once its addition succeeded: nothing heard, a `5xx`, a malformed answer or a rate limit on the removal | `Timeout`, no rollback; a typed refusal stays final |
| after a byte was written, as forge.md 5.4, a `5xx` included | `Timeout` |
| a malformed answer (section 6) | `Timeout` on a write; `InvalidAnswer` on a read |
| `401`, on a token or the JSON Web Token | `Forbidden` |
| `403` "Resource not accessible by integration", or any other `403` | `Forbidden` |
| a `404` or `410` on a branch leaf (`Read::Branch`, `Stage::Read`) or contents leaf (`Read::File`, `Stage::Raw`), classified as `Asked::Leaf` | `Missing`, `Drop::Nothing`: the repository row stays |
| every other `404` or `410` that ends the call, including repository resolution by id, its own GET and listings (`Asked::Read`) | `Missing`, the row dropped (`Drop::Row`); `MissingJob` on `Job`'s |
| `409` on `Merge`, "Head branch was modified. Review and try the merge again." | `Stale` |
| `405` on `Merge`, "Pull Request is not mergeable" | `Conflict` |
| `405` on `Merge`, "Repository rule violations found" and the merge queue's line | the queue (5.1) |
| `405` on `Merge`, "Repository rule violations found" and any other line | `Protected` |
| `405` on `Merge`, "... merges are not allowed on this repository." | `Refused`, the row dropped |
| GraphQL `UNPROCESSABLE`, "... expected head oid does not match the current head oid" | `Stale` |
| GraphQL `UNPROCESSABLE`, "Pull request Required status check ..." on `enqueue` | `Protected` |
| any other GraphQL error on `enqueue` | `Refused` |
| `422` on `Update`, "expected head sha didn’t match current head ref." | `Stale` |
| `422` on `Update`, "merge conflict between base and head" | `Conflict` |
| `422` on `OpenPull`, "No commits between ..." | `NothingToMerge` |
| `422` on `OpenPull`, "A pull request already exists for ..." | `Exists` |
| `422` on `Review`, "Can not approve your own pull request" | `Refused` |
| `422` on a write whose title or body is empty | `Empty` |
| `422` on `CreateBranch`, "Reference already exists" | `Branch(Exists)`, an answer |
| `422` on `DeleteBranch`, "Reference does not exist" | `Missing` |
| a pull request's head other than `PullFiles` names | `Stale` |
| an answer past the component's bounds; a write's text not UTF-8 | as forge.md 5.4 |
| any other `4xx` | `Refused` on a write; `InvalidAnswer` on a read |

- **The message is matched whole,** by line: GitHub's `message`, and a
  `422`'s string entries in `errors` or objects' `errors[].message`,
  against the texts the conformance records
  (section 13). Enqueue's required-check refusal matches the prefix
  "Pull request Required status check " only for `UNPROCESSABLE`.
  A rule violation's message is a first line,
  "Repository rule violations found", then one line per rule. An
  unknown message is `Refused` on a write, `InvalidAnswer` on a read.
- **A merge just after a move** may answer `405` "Pull Request is not
  mergeable" while GitHub computes, or `409` "Head branch was modified.
  Review and try the merge again." (`merge-after-move-not-merged`). The
  dialect says `Conflict` for the former and `Stale` for the latter; the
  owner reads the pull again. The client reads again within `settle`
  before taking the former for a conflict (forge-client.md, section 5).
- **A `401` on the JSON Web Token** is the App's key refused (deleted,
  or a clock more than a minute off): `Forbidden`, which the component
  takes to stop the forge (forge-connection.md, section 5).
- **Repeating a successful merge with the same sha** answers 200 with the
  same merge sha and merged true. A different sha on that merged pull is
  inferred to be 409; the fake keeps the stale-head check before mergeability.
- **A `304`** is not a failure: it answers a conditional read (8.5).

A repository gone or outside the installation loses its row at its next
repository-level call. An absent leaf alone does not invalidate the
repository that answered it.

## 8. The App's documents

### 8.1 The JSON Web Token

- **`github::jwt::signing_input(app, issued, expires) ->
  Option<Box<[u8]>>`**: the base64url, unpadded, of the header
  `{"alg":"RS256","typ":"JWT"}`, a `.`, and the base64url of the claims
  `{"iat":<issued>,"exp":<expires>,"iss":"<app>"}`, the times in whole
  seconds since the epoch, rounded down, and the App's id in decimal.
  None when `expires` is not after `issued` or is more than 600 seconds
  after it, which GitHub refuses.
- **`github::jwt::token(input, signature: &[u8; 256]) -> Secret`**: the
  input, a `.`, and the signature's base64url.
- **`github::jwt::JWT_BYTES`,** 512, bounds a token's length: the
  header's 36 bytes, the claims' at most 96 (two times of 11 digits, an
  id of 20), the signature's 342 and the two dots, with room. The
  component bounds a request's head by it (forge-connection.md,
  section 7).
- **The component signs** (`skein_tls::sign_rs256`, the App's 2048-bit
  key), and chooses the times: `issued` a minute before its clock, and
  `expires` ten minutes after `issued` (forge-connection.md, section 5).

### 8.2 The mint

- **`POST /app/installations/<id>/access_tokens`,** on the JSON Web
  Token, `201`, either `Full`, with no body, or `Contents`, with
  `{"permissions":{"contents":"write"}}`, the git token of a forge's
  `Event::GitToken`.
- **Its decoder** keeps `token`, as a `Secret` within `string_bytes`,
  `expires_at`, and the `contents` and `pull_requests` levels of
  `permissions` (`read`, `write`, absent or another word as none). The
  full mint's levels are the installation's: the connection keeps them
  beside the token, each mint replacing them, and answers the bot's
  `Read::Permission` from them (4.1). GitHub answers a bot's
  collaborator permission `{"permission":"none","role_name":""}`
  (`bot-permission-is-not-a-role`): an App's access is its
  installation's, never a role. A repository outside a selected
  installation is refused at its read (`404`), before any permission.
- **A token minted for Contents creates issues but cannot edit them:** the
  observed mint permissions are `contents: write`, `metadata: read`, with
  `repository_selection: selected`. Issue creation answers `201`, with
  `x-accepted-github-permissions: issues=write`; a title PATCH with that
  token answers `403` "Resource not accessible by integration", with
  `x-accepted-github-permissions: issues=write; pull_requests=write`
  (`token-narrowed-creates-not-edits`). The dialect mints the same tokens;
  its mint decoder reads token, expiry and the contents and pull_requests
  levels (8.2).
- **So the git token is not a boundary:** for its hour it can open
  issues as the bot, though it is refused an edit (observed 2026-10-20).
  The narrowing stays; a second App would close the gap, and that is the
  user's call (forge-connection.md, section 5.3).

### 8.3 Installations

- **`GET /app/installations?per_page=100&page=<n>`,** on the JSON Web
  Token, every page: each row's `id`, `account.login`, `account.id`,
  `account.type` (`User` or `Organization`), `created_at`, and
  `suspended_at` not null as suspended.
- **An installation's repositories:** `GET
  /installation/repositories?per_page=100&page=<n>`, on that
  installation's token, every page: each `id`, `owner.id`,
  `owner.login`, `name` and `private` (`github::Reachable`), which
  temper's picker shows.
- **The App's uninstall** is `DELETE /app/installations/<id>`, on the
  JSON Web Token, `204` (section 11).

### 8.4 The identity read

`GQ viewer` on an installation's token: `databaseId` the bot user's id,
and `login`. REST's `GET /user` is a `403` on that token
(`user-on-installation-403`). Counted in no call's cost, as Forgejo's.

### 8.5 The rate and validators

- **`api::Rate { limit, remaining, reset_at }`,** from
  `x-ratelimit-limit`, `-remaining` and `-reset` (seconds since the
  epoch), on every answer that has all three; the client's budget takes
  it in place of its own (forge-client.md, section 6). GitHub's core
  allows an installation 5000 an hour (`rate-core-5000`); the App's own
  calls carry none (`jwt-calls-no-rate`).
- **`api::Validator`,** an answer's `ETag`, at most 128 bytes of
  printable ASCII, else none kept. A request sent with it
  (`If-None-Match`) is answered `304`, with no body, as
  `Answered.free`: GitHub does not count it (`etag-304-free`). The
  fact uses an installation token's two reads of an unchanging repository,
  with the first ETag on the second and the same remaining rate. A changing
  delivery log may answer a new body instead. The dialect sends its
  validator on the delivery log's first page alone (9.3).

## 9. Deliveries

### 9.1 Pushed

As forge.md section 7, in GitHub's words:

- **At `/hooks/<number>`,** signed `X-Hub-Signature-256: sha256=<hex>`,
  HMAC-SHA256 of the body with the App's webhook secret, verified as it
  streams; the event in `X-GitHub-Event`, the delivery's guid in
  `X-GitHub-Delivery`. The verdicts are forge.md 7's.
- **Not yet reached:** GitHub delivers from the internet, and temper's
  App points its hook at an address that does not resolve (temper's
  draft, 5). Every delivery fails and is listed in the log, which the
  component reads (9.3). A public address is later (temper's draft 18);
  the fake's layer pushes to a listener when a test asks (fake-forge.md,
  section 10).

### 9.2 One decoder, two ways in

`github::delivery::decode(event, body)` reads one payload into a
`github::Delivery`; a pushed body and a logged delivery's
`request.payload` give the same. The guid is a UUID's text, read into
`[u8; 16]`; another text is malformed. The App receives `installation`,
`installation_repositories` and `installation_target` without
subscribing; the others are temper's draft section 5's, and no more.

| `X-GitHub-Event` | `github::Delivery` | `Hint` |
|---|---|---|
| `issues` | `Issue` | `Item(issue.number)`, the key of the issue's body |
| `issue_comment` | `Comment` | `Item(issue.number)`, the key of the comment's body |
| `pull_request` (each action, `enqueued` and `dequeued` with its `reason` among them) | `Pull` | `Item(number)`, no key |
| `pull_request_review` | `Review` | `Item(pull_request.number)`, the key of the review's body |
| `pull_request_review_comment` | `Remark` | `Item(pull_request.number)`, no key |
| `push`, of a branch (`refs/heads/`) | `Push` | `BranchMoved { branch, head: after, actor: sender.id }`; `deleted` true, `Branch(branch)` |
| `create`, `delete`, `ref_type` `branch` | `Ref` | `Branch(ref)` |
| `check_run` and `check_suite` `completed`, `status`, `workflow_job` `completed`, `workflow_run` `completed` | `Ci` | `Commit(head_sha)`; a status's `sha` |
| `merge_group` (`checks_requested`, `destroyed`) | `Ci` | `Commit(merge_group.head_sha)` |
| `installation` (`created`, `deleted`, `suspend`, `unsuspend`) | `Installation` | none: the component's (below) |
| `installation_repositories` (`added`, `removed`) | `Installation`, `Repositories` | none |
| `installation_target` `renamed` | `Installation`, `Renamed { login }` | none |
| `ping`; a tag's `push`, `create` or `delete`; any other | `Other` | none |

- **The repository** is the payload's `repository.id`, past 32 bits no
  hint; the forge, the component's number for it.
- **`Installation { installation, account, change }`**: the
  installation's id, its account's login and id, and the change:
  `Created`, `Deleted`, `Suspended`, `Unsuspended`, `Repositories`
  (with no names: the owner reads them again, 8.3) or `Renamed {
  login }`. The component makes them `Event::Installation`
  (forge-connection.md, section 5).
- **Bounded.** A push's `commits`, a pull's diff fields and every
  repository field but its id, owner's login and name are skipped as
  they pass. The login is at most 39 bytes and the name at most 100;
  either past its bound is malformed. The decoder can hand out their
  path when both are present; id-only bodies remain valid.

### 9.3 The delivery log

- **A list page:** `GET /app/hook/deliveries?per_page=<n>`, with
  `&cursor=<c>` past the first, on the JSON Web Token, newest first.
  Each row, `github::log::Row`: `id` (19 digits), `guid`, `event`,
  `action`, `installation_id` and `repository_id` (none for `ping`,
  `installation` and `installation_repositories`), `status_code`,
  `delivered_at` and `redelivery`. The next page's cursor is the
  `cursor` of `Link`'s `rel="next"`, a `github::log::Cursor` of at most
  128 printable bytes; none when there is no next.
- **The first page is conditional:** sent with the last page's
  `Validator` as `If-None-Match`, a `304` says nothing new, at no cost.
- **One delivery whole:** `GET /app/hook/deliveries/<id>`, on the JSON
  Web Token. The plan carries the listed row's bounded event word and
  starts the payload decoder from it, whatever the JSON member order.
  Its `event` and `request.headers.X-GitHub-Event` must both be present
  and equal the row's event, or the read is malformed. The decoder reads
  `request.headers` (`X-GitHub-Event`, `X-GitHub-Delivery`) and
  `request.payload` as a pushed body is (9.2); its `response` is skipped.
  The logged delivery carries the payload's repository path when both
  its owner's login and name are present, and none for a kind with no
  repository.
- **Trusted by its channel,** not its signature: the payload comes back
  re-encoded, so no HMAC is checked; the read is the App's own, over
  TLS, on its token.
- **The puller** (which guid is newest, `log_poll`, `log_catch_up`) is
  the component's (forge-connection.md, section 6).

## 10. A job's log

- **`GET R/actions/jobs/<job>/logs`** answers `302` with `Location`, a
  signed address on another host, valid about ten minutes, which wants
  no credential (`job-log-302-blob`). It honours `Range: bytes=0-0`
  with `206` and `Content-Range` (`job-log-range-honoured`). Skein sends
  no Range, and rejects an unsolicited 206 before the log body decoder.
- **`github::Redirect { origin: { tls, host, port }, target }`,** parsed
  from `Location`: `https` over TLS, `http` in plaintext (the fake's,
  fake-forge.md, 10.1), with its host and port (443 or 80 when none),
  the target its path and query. Another scheme, or none, is
  `InvalidAnswer`. A plaintext origin is never among temper's defaults.
- **The component follows it** to an origin among its `log_origins`,
  sending nothing of the forge's, and an origin it does not know is
  `Refused` and a diagnostic (forge-connection.md, section 4).
- **The log is read whole** as it streams, its last `max_bytes` kept
  within the answer's bound, `truncated` when bytes before them were
  read. GitHub's log is the job's, not a step's.

## 11. The setup's documents

Pure functions and decoders; forge-setup.md's plans run them.

- **`github::setup::manifest(values)`,** the manifest's JSON: `name`,
  `url`, `public` true, `redirect_url`, `setup_url`, `setup_on_update`
  true, `hook_attributes { url, active: true }`, `default_permissions`
  and `default_events`. The values are the name, the url, the hook's
  url and the two return addresses; the permissions and events are the
  module's constants, `PERMISSIONS` and `EVENTS` (temper's draft 5:
  contents, pull requests, issues and merge queues write; checks,
  statuses, actions, administration and metadata read; the fourteen
  events).
- **The form's target:** `<web>/settings/apps/new?state=<s>` for a
  person or `<web>/organizations/<org>/settings/apps/new?state=<s>` for
  an organization, `<web>` being `https://github.com` or the fake's
  origin. The page whose form posts the manifest there, in its
  `manifest` field, is the owner's (forge-setup.md, 5.1): the dialect
  makes no HTML.
- **The install page:** `<web>/apps/<slug>/installations/new?state=<s>`.
- **The conversion:** `POST /app-manifests/<code>/conversions`, with no
  credential, `201`: `id`, `slug`, `owner.login`, `owner.id`, `pem` and
  `webhook_secret` as `Secret`s; the OAuth client's id and secret are
  skipped.
- **The bot user:** `GET /users/<slug>%5Bbot%5D`, no credential: `id`.
- **The App's own reads,** on the JSON Web Token:
  - `GET /app` for `id`, `slug`, `permissions` and `events`: reading
    `id` and `slug` back proves a conversion's key, and checks a pasted
    App, which the plan takes for another App when they differ
    (forge-setup.md, section 5). Permissions retain every name and its
    `read`, `write` or `admin` access; another access is `InvalidAnswer`.
    Permission and event names fit `string_bytes`, each list fits `inner`,
    and a name or list past its bound is `TooLarge`. Missing permissions
    or events read as empty lists; id and slug remain required. Unknown
    fields are skipped. `setup::lacks(&App)` names the wanted permissions
    and events absent from these lists, in manifest order; admin covers
    write and read, and write covers read;
  - `GET /app/hook/config` for `url`;
  - `GET /app/installations/<id>`, one installation's row as 8.3's,
    which the plan's accept reads; strays are found from 8.3's
    listing, by `created_at`.
- **The App's writes,** on the JSON Web Token: `PATCH /app/hook/config`
  `{url}` (`200`); `DELETE /app/installations/<id>` (`204`). The
  conformance sends neither live (fake-forge.md, section 11).
- **The key** is the conversion's PEM; the setup's host converts it to
  the DER that `Credential::App` holds (forge.md, section 8).

## 12. The pin

- **`API_VERSION`,** `2026-03-10`, sent on every request. A request
  without it gets 2022-11-28, and GitHub supports a version for 24
  months after its next.
- **`CHECKED`,** the date the live conformance last held on github.com
  (the probes', 2026-10-20, until its first run).
- **Readers:** the conformance (fake-forge.md, section 11), which
  records each fact of section 13 and finds a change.

## 13. What it relies on of GitHub

Each fact by its conformance name, checked on the fake and, opt-in, on
github.com (fake-forge.md, section 11); the probe that found it in
temper's draft section 11.

- **Tokens and identity:** `token-expires-in-an-hour`,
  `token-narrowed-creates-not-edits` (P1); `rate-core-5000`, `jwt-calls-no-rate`
  (P1); `viewer-is-bot`, `user-on-installation-403`,
  `app-routes-401-on-installation-token` (P2); `jwt-iss-string`;
  `bot-permission-is-not-a-role` (the App's bot's collaborator
  permission is `none` with an empty `role_name`, and the full mint's
  `permissions` name the installation's levels; observed 2026-10-23).
  Contents narrowing returns mint permissions contents: write,
  metadata: read. Issue creation is 201 with the accepted permissions
  header; a title PATCH of that issue with the same token is 403,
  "Resource not accessible by integration".
- **Pull requests and CI:** `mergeable-null-then-settles`,
  `combined-pending-with-no-status`, `job-log-302-blob`,
  `job-log-range-honoured` (P4); `merge-stale-409`,
  `merge-after-move-not-merged`, `merge-unheld-by-ci`,
  `update-stale-422`, `update-202-later` (P5); `etag-304-free` (P6).
  Mergeability is null in the create response or a read soon after a head
  move, then boolean within the wait; the first read may already be boolean.
  The move fact requires either the recorded 405 or 409 and a pull read
  that is not merged. The Range fact expects 206 and Content-Range.
  The ETag fact reads an unchanging repository with an installation token, and expects 304 with
  the same remaining rate. `merge-repeat-idempotent` expects 200, merged
  true and the same merge sha for a repeat with the same sha, then compares
  both PUTs with the merged commit of the dialect's pull answer. At the pin,
  REST omits merge_commit_sha, so this answer reads mergeCommit.oid through GQ.
  `merge-repeat-any-sha-200` expects 200 and merged true for a repeat with a
  different sha on a merged pull; an open pull's stale sha remains 409.
- **The log:** `log-lists-failed-deliveries`, `log-cursor-in-link`,
  `log-ids-19-digits`, `log-delivery-whole`,
  `log-installation-events-unscoped` (P7);
  `installation-target-in-log` (an App not subscribed to it). The fake checks
  an account rename; live records it unchecked because the run renames no
  account, so the event remains unobserved live.
- **Rules and the queue:** `approve-own-422`, `ruleset-approval-405`,
  `bypass-always-merges` (P8); `queue-direct-merge-405`,
  `enqueue-stale-unprocessable`, `enqueue-pending-check-protected`,
  `queue-merged-no-merge-commit-sha`,
  `queue-dequeued-delivered` (P9); `signatures-push-gh013` (P10);
  `rules-branches-current-user-can-bypass`,
  `classic-protection-404-under-rulesets` (P11). Own approval is 422 with
  message Unprocessable Entity and a string error, Review Can not approve
  your own pull request. Pull request rules carry all five parameters:
  the approval count and the four booleans false, with an empty ref exclusion.
  Required status checks include strict_required_status_checks_policy: false
  and context ci. Merge queues carry all seven parameters: timeout 60
  minutes, grouping ALLGREEN, maximum build and merge entries 5 each,
  method MERGE, minimum entries 1, and minimum-entry wait 0 minutes.
  One entry and no batching wait keep single-pull cases immediate; the
  check timeout is a ceiling, not a mandatory delay. Queue cases and their
  rulesets use an organization's repository and its matching tokens.
  The pending-check fact approves the pull while its head workflow is still
  in progress, tests Merge as Protected, and tests raw enqueue as UNPROCESSABLE
  with the required-check prefix. No case posts commit statuses. The merge
  case waits up to 180 seconds (90 seconds before the user's doubling of 10-22, provisional) for the head's ci to pass, admits through Merge
  as Queued, and polls the pull up to 300 seconds (150 seconds before the user's doubling of 10-22, provisional) while the group's own ci
  runs. REST omits merge_commit_sha and the dialect reads the merged commit
  through GQ. The dequeue case sends a SQUASH ruleset and creates two commits
  with passing messages, putting FAIL only in the pull title. It records the
  repository's squash title setting and the group commit's message. The group
  branch appears asynchronously after enqueue; the case polls its branch
  listing at most twenty-five times, five seconds apart (120 seconds; sixty seconds before the user's doubling of 10-22, provisional), and
  records the number of reads. A listing with 100 or more branches fails the
  fact as queue branches listing full; an exhausted poll fails it as queue
  group branch absent. A setting other than COMMIT_OR_PR_TITLE, or a passing
  group check, makes that fact
  unchecked: the title-driven group failure remains unconfirmed live. A failed
  group check must produce a dequeued delivery. Head and group checks and
  merge reads are polled every 15 seconds. Each wait expiry names the head ci,
  group ci, or merge wait in a failed fact. Approval, admission, branch and
  check read refusals fail their fact; a raw enqueue refusal includes its GQ
  message. Later cases still run.
- **Repositories and issues:** `installation-repositories-at-once`
  (P12); `issues-list-pulls`, `new-issue-may-miss-listing` (P13).
  The pull listing gets three tries, two seconds apart, without since.
  The new-issue fact records immediate visibility in an unfiltered listing;
  either visibility is valid. `issues-since-epoch-empty` first waits for its
  made issue in the unfiltered listing, with at most three reads two seconds
  apart, and records the read count. Exhaustion fails the fact as made issue
  absent from listing and sends no later reads. Once listed, raw epoch since
  must answer 200 with an empty array, then the dialect's zero-time Items
  read must contain the issue.
  `installation-repository-removal-refusals` expects 422, "Cannot remove the
  last repository from this installation.", from each fixture installation.
- **Additional protocol facts:**
  `branch-with-slash-reads`, `issues-since-inclusive`,
  `user-by-id-reads`, `repository-allows-shown`,
  `pulls-422-no-commits`, `pulls-422-exists`, `refs-422-exists`,
  `refs-delete-missing-422`, `update-conflict-422`,
  `compare-files-capped-300`,
  `comment-updated-at-on-edit`. The inclusive-since case takes a settled
  row's updated_at from an unfiltered listing and finds that row when since
  equals it. Both pull-creation 422 cases read errors[].message beneath
  "Validation Failed", in a PullRequest resource object with code custom:
  "No commits between main and <head>" or "A pull request already exists for
  <owner>:<head>." The comment edit uses the App's installation token and
  requires updated_at to advance. For setup and the component
  (forge-setup.md and forge-connection.md cite them):
  `inactive-hook-visible`, and `manifest-hook-invalid-host-accepted`
  (an App hook at `https://<name>.invalid/`; the draft's Setup probe
  saw `https://hooks.example-app.invalid/github` taken), both
  `unchecked` on github.com and held on the fake alone, since a live
  run changes nothing of the App (fake-forge.md, section 11); and
  `job-log-host-per-owner` (whether the log's redirect host differs
  across the two owners' repositories).

A fact's texts (each refusal's message) are the conformance's
transcripts, and the decoders' tests take them (section 14). A response
framing, length or parse error preserves the masked request and received
response head and fails that fact; later cases run, while guard, budget,
journal and cleanup errors stop the run. A missing input from an earlier
response fails only its own fact; later cases run.
The runner re-signs the App JWT at half its 600-second life before an App
request. Close-delimited responses are read to stream end within the 4 MiB
bound on a connection used for one request, and each records its case and
status in provenance.

## 14. Verification

The module's step tests, in `skein-forge`, with no io, by forge.md
section 11, and GitHub's:

- **Recorded answers:** each decoder against the conformance's
  transcripts (the fake's, and github.com's once a live run has held),
  answers generated from a seed, and mutations of both.
- **Each refusal of section 7** told apart from its neighbours of one
  status, by its message, and an unknown message on a read and a write.
- **The merge plan's routes:** merged; refused for the queue and
  queued; stale at the merge and at the queue; each other `405`.
- **Protection:** rulesets alone, classic alone, both, neither, a
  ruleset's bypass of each value, more rulesets than `inner`.
- **The log:** a page with a next and without, a cursor at 128 bytes
  and one past, a `304`, 19-digit ids and `u64::MAX`, a row with no
  installation, a whole delivery decoded as its pushed body is.
- **The App:** the JSON Web Token's bytes, golden, and a span of 600
  seconds and 601; the mint's two bodies; a guid as a UUID, upper-case,
  and not one.
- **The redirect:** a `Location` with a port, without, `http`, `ftp`,
  none.
- **Fuzzing:** one target per decoder, as Forgejo's.

The fake's GitHub layer is tested by the conformance against it, and the
component's worlds run it (fake-forge.md, sections 10 and 11).

## 15. Open questions

- **Conditional reads beyond the log.** GitHub answers `304`, free, on a
  stable repository (`etag-304-free`); broader use would keep validators
  for other resources too. Default: the log alone.
- **The update's head.** `Update { number }` carries none, so the plan
  reads it first; `Update { number, head }` would make it conditional on
  the head the client saw, as `Merge` is. Default: read first.
- **The queue's read on every pull.** An open pull request of an
  organization's repository costs a GraphQL read for its position.
  Default: so; a repository known to have no queue could skip it.
- **Long threads.** An item's comments are read from page 1 each time.
  Default: so; a `since` the client named would skip pages.
- **Official reviews** by `author_association`. Default: owner, member
  and collaborator; the rule's required reviewers would be exact, at a
  cost.
- **Repository ids past 32 bits** are `TooLarge`, as Forgejo's.
  github.com's are about a third of the way there. Default: so, until
  the vocabulary widens `Repository`.
- **The blob host's address,** resolved at start (io.md, section 4).
  Default: so; a host that moves is a finding.
- **A classic rule's merge queue** is not read by `Protection`: the
  merge's refusal finds it (5.1). Default: so.

## 16. The live conformance's organization selection

The live fixture format is described in fake-forge.md, section 11, and
`tests/forge/github/README.md`. At startup, before minting tokens or making
repository changes, the runner reads each installation's probe repository
with that owner's person token. The first probe whose `owner.type` is
`Organization` selects the installation for every merge-queue case,
starting with `queue-direct-merge-405`, and their rulesets. Those cases
use that installation's token and its owner's person token where a person
acts. The fixture document needs no account-type field. If none qualifies,
startup stops with "no organization installation for the merge-queue cases"
before any write. The fake's `acme` is a user and `globex` an organization.

The plan's marker word is the tool's own `b"conformance"` constant, checked
with `marker::valid_word` in `Runner::start` before any request. It is
independent of the App's slug, which names the bot and appears in provenance.

Each live fixture repository must carry `.github/workflows/ci.yml`, triggered
by `push`, `pull_request`, and `merge_group`, with job and check run named
`ci`. It runs for about 45 seconds. It reads the message at
`pull_request.head.sha`, else `merge_group.head_sha`, else the pushed sha,
and fails when that message contains `FAIL`; otherwise it succeeds. The
queue ruleset requires this check. The conformance fake uses the shared
domain's CI, runs, jobs and message cue with 45 fake seconds of latency.

The repository's `squash_merge_commit_title` must be `COMMIT_OR_PR_TITLE`
for the title-driven dequeue probe: one commit uses that commit's message,
more than one uses the pull's title. The case reads and records the setting;
it changes no repository setting. Its two head commits pass, while its
`FAIL` pull title is intended to fail the squash group's workflow.
