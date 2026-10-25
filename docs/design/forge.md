# The forge kit

Provisional, 2026-10-20. What skein gives a service that works on a code
forge. Forgejo's dialect is built, and GitHub's is designed in
forge-github.md (section 10). The kit is:

- a neutral vocabulary of forge calls, rendered by a dialect;
- a component that runs those calls and receives the forge's webhooks;
- a client domain that keeps a bounded view of a forge and writes to it
  at most once;
- a setup machine that makes a forge ready for a service;
- the fake forge and the conformance tooling that a service's worlds and
  tests need.

This document is the kit as a whole and its first crate, `skein-forge`:
the vocabulary, the dialects, keys' markers, deliveries and credentials.
The others are forge-connection.md (the component), forge-client.md (the
client), forge-setup.md (the setup) and fake-forge.md (the fakes, the
throwaway Forgejo and the conformance).

## 1. In one page

- **What a connector stands on, not the connector.** The kit is the
  layers a forge connector is built on: the documents, the call plans,
  the component, the client's keep and write lanes, the setup's machines
  and the fakes. It holds nothing a connector decides: what to watch,
  which effects exist and what each is for, when to write, and what
  people see (section 2.3). The kit names no service.
- **Shaped like skein's LLM crates.** `skein-forge` is to forges what
  `skein-llm` is to providers: a neutral vocabulary rendered by
  dialects. Each call is answered once, and nothing is retried.
  `skein-forge-connection` is to it what `skein-llm-connection` is to
  `skein-llm`: a component that an owner's protocol layer owns, and that
  connects, runs calls and keeps connections (llm.md;
  llm-connection.md).
- **One call, one answer, and its cost.** Each `Read` or `Write` is
  answered by exactly one `Answer` or `Error`, made of one or a few HTTP
  exchanges. The answer carries the count of requests the call cost.
  Budgeting the forge's rate is the client's job (forge-client.md,
  section 6).
- **Repositories by the forge's id.** A repository is named by the
  forge's number and the forge's own id, which survives a rename or a
  transfer. A repository is first found by its owner and name, with a
  locate.
- **The owner names its marks.** A creation's key travels in a marker
  whose word the owner gives (`<!-- <word>:key … -->`; temper's word is
  `temper`). The user agent is the owner's, and setup marks its bot with
  the same word.
- **Dialects hold the documents and the plans.** Forgejo's dialect holds
  both sides of each document, its deliveries, the setup's documents and
  its pin. Each call is run by a call plan of the dialect: a small pure
  machine that does no io (section 4).
- **Credentials have a kind, set by the dialect.** A token lasts until
  revoked (Forgejo's, or a GitHub fine-grained token). A GitHub App
  installation's token is minted from the App's key and lasts an hour.
  The component holds the current credential, renews it when the dialect
  says, and gives it to no one (section 8).
- **Webhooks for latency, polling for liveness.** Deliveries are
  verified as they stream and become hints. The client's keep polls
  whether or not they arrive.
- **The client keeps and writes, and decides nothing.** Its working set,
  request budget and write lanes are bounded. Each write carries a key
  that its owner made, so a write whose answer was lost is found after a
  restart, not made twice. What an effect is for is opaque bytes of the
  owner's.
- **Setup brings a forge to one kind of result: an access.** On any
  forge that result is the same:
  - the user it acts as;
  - its API and git credentials;
  - the repositories it may write;
  - a webhook secret and its hooks.

  Each forge has its own plan to reach it. Forgejo's is reconcile, with
  a site admin's token. GitHub's is the App manifest flow
  (forge-setup.md). The kit supplies only the machines; the host is the service's.
- **The fake forge is shared, with a layer per dialect.** Its domain is
  one; Forgejo's protocol layer and GitHub's follow their dialects, and
  skein-fake-peers hosts them. Forgejo's conformance runs against a
  throwaway Forgejo and records the facts that section 9 lists;
  GitHub's runs against the fake, and opt-in against github.com
  (fake-forge.md, sections 10 and 11).
- **GitHub is a second dialect,** `skein_forge::github`, beside
  `forgejo` (forge-github.md). It adds to the vocabulary validators, the
  rate's window, `Queued`, the protection's queue and bypass,
  `NotConnected`, `NotInstalled` and the App's credential (section 10).
  Forgejo's dialect ignores each addition or answers its neutral value.

## 2. The kit's crates

### 2.1 The crates

```
skein-forge                     the vocabulary (api, locate); the dialects: Forgejo's documents, both
│                                 sides, its call plans, its deliveries, the setup's documents, the
│                                 pin; GitHub's likewise (forge-github.md); keys' markers;
│                                 credentials' kinds
├── skein-lib, skein-json
skein-forge-client              the keep (working set), the request budget, the write lanes and their
│                                 recovery, keys
├── skein-lib, skein-forge (its vocabulary only)
skein-forge-connection          the component: calls, connections, each forge's start, repositories'
│                                 paths, users, credentials and their renewal; the webhooks' listener
├── skein-lib, skein-io, skein-http, skein-tls, skein-json, skein-forge
skein-forge-setup               the access and each forge's plan: Forgejo's reconcile machine;
│                                 GitHub's manifest flow
├── skein-lib, skein-io, skein-http, skein-json, skein-forge
crates/skein-fake-forge-domain     the fake forge's domain
crates/skein-fake-forge-protocol   Forgejo's API over it on skein's HTTP server; the setup's routes;
                                     signed deliveries; the tally of requests served
testing/skein-fake-forge-github    GitHub's API over it, at its pin (fake-forge.md, section 10)
crates/skein-fake-peers            hosts the forge peer, beside the LLM and OAuth peers
testing/skein-throwaway-forgejo    a real Forgejo and its runner, at the pin, for the conformance
tests/forge/machine, protocol,     the kit's worlds, one crate each (section 11)
  client, setup
tests/forge/conformance            a tool outside the suites: records the dialect's facts
tests/forge/github                 GitHub's conformance: on the fake, and opt-in live (fake-forge.md,
                                     section 11)
```

- **`skein-forge` is `no_std` (with `alloc`) and lib-only,** as the
  client is, whose vocabulary it carries. It has no io and no clock.
- **The client and the component do not depend on each other.** Both
  stand on the vocabulary. The owner carries calls to the component, and
  answers and hints back to the client (forge-client.md, section 2;
  forge-connection.md, section 2).
- **Each crate has its document:** forge-connection.md for the
  component, forge-client.md for the client, forge-setup.md for the
  setup, and fake-forge.md for the fakes, the forge peer, the throwaway
  and the conformance.

### 2.2 What the owner gives

The owner is the service code that owns a part: its protocol layer owns
the component, its forge connector (a domain) owns the client, and its
setup shell hosts reconcile. The kit reads no file and no configuration
(forge-connection.md, section 3). The owner hands it values:

- **The word.** The marker's and the bot's (section 6).
- **The user agent,** on every request (section 5.1).
- **The namespace and the identity.** The namespace is the owner's keys'
  namespace. The identity is the owner's bytes for an effect, opaque to
  the kit (forge-client.md, section 7).
- **For each forge:** its number, its dialect, its API's address and
  transport, the value of its credential, the forge user that credential
  must stand for, and its webhook secret when it has one.
- **The listener's address,** when webhooks are used.
- **The limits** of each crate it owns.

### 2.3 What stays with the service

- **The connector's top.** It is what the client's owner builds on the
  client:
  - what it watches;
  - which effects exist, and what each is for;
  - policy: gates, merges and landings;
  - adoption's checks;
  - what people see.

  It is not in skein: it is the service's.
- **The protocol layer's translation,** between its own boundary and the
  kit's calls, answers and hints; and the store's records of the
  client's rows (the `Stored` rows that the client hands its owner to
  keep).
- **The configuration and its files.** The kit reads none.
- **The setup's host:** its command, its files, the forge's lock, its
  exit status. Owned Forgejo's preparation (`app.ini`, migrations, the
  runner's registration, upgrades' dumps) is the service's.
- **The service's git component,** and that component's fakes.
- **The service's worlds' stories,** and its cost ceilings. They run on
  the kit's fakes.

## 3. The vocabulary

### 3.1 What it is

`skein-forge`'s `api` and `locate` modules are the forge's neutral
vocabulary. It describes bounded requests and answers between the
client and a dialect, and retains no state. It names no task, no grant
and no policy. Every type is a plain value.

- **Calls:** `Op`, which is a `Read` or a `Write`.
- **Their result:** `Answer`, `Error`, and the enums they use (`Kind`,
  `State`, `Ci`, `Check`, `Verdict`, `Permission`, `BranchCreation`,
  `Provenance`, `Bypass`).
- **What an answer says of the forge** (3.3): `Rate`, the rate's window,
  and `Validator`, a read's validator.
- **A hint:** `Hint`, with its `Change`.
- **The items** that answers carry (3.6).
- **Addressing:** `Repository { forge, repository }`, `forge` being the
  owner's number for a forge (16 bits) and `repository` the forge's own
  id (32 bits). A commit is 32 bytes (a SHA-1 id zero-padded). Times
  are `skein_lib::Time` and `Duration`.

### 3.2 The calls

20 reads and 13 writes, in families. Each call has one answer.

| Family | Calls | Answer |
|---|---|---|
| Find a repository | `Locate` | `Located` |
| The change feed | `Items` | `Items` |
| One item | `Item`, `Pull`, `PullFor` | `Item`; `Pull`; `Pull` |
| Review | `Reviews`, `Remarks` | `Reviews`; `Remarks` |
| CI | `Statuses`, `Checks`, `Job` | `Statuses`; `Checks`; `Job` |
| Branches | `Branch`, `Branches` | `Commit`; `Branches` |
| Contents | `PullFiles`, `Compare`, `File` | `PullFiles`; `Compare`; `File` |
| The repository | `Protection`, `Settings`, `CiSetup`, `Collaborators`, `Permission` | the same names |
| Creations | `CreateIssue` (keyed), `OpenPull` | `Created` |
| Comments, keyed | `Post`, `Review` | `Commented`; `Reviewed` |
| Edits | `Edit`, `SetReviewers`, `Close`, `Reopen` | `Done` |
| Merging | `Merge`, `Update` | `Merged` or `Queued`; `Done` |
| Marks and branches | `Status`, `CreateBranch`, `DeleteBranch` | `Done`; `Branch`; `Done` |

- **The change feed** is one listing of items since a time, least
  recently updated first, in pages from 1. It is the only listing of
  items (5.2).
- **A keyed write** takes the key the owner made. The dialect writes it
  into the marker (section 6). A pull request's key is its branches, so
  `OpenPull` takes none.
- **`PullFor { head, base }`** asks for the open pull request of a head
  and a base, else the oldest.
- **`Job { attempt, max_bytes }`** asks for the end of one failed job
  attempt's log; `File { head, path, max_bytes }` for the start of one
  file at a commit. Both are cut to `max_bytes`, or to what the answer
  has left (5.3), and say when they cut.
- **`Merge { number, head }`** is conditional on the head. A merge's
  base is the client's precondition, checked by reading the pull
  request afresh (forge-client.md, section 7). Where a merge queue
  guards the base, it answers `Queued`: the pull request was put in the
  queue at that head, and the landing comes later, as a hint
  (forge-github.md, section 5.1).
- **`Protection`** needs an admin to read an absent rule. A writer who
  is not an admin gets `Forbidden` (5.2).

### 3.3 The answers

26 answers: `Located`, `Items`, `Item`, `Pull`, `Reviews`, `Statuses`,
`Remarks`, `Commit`, `Branches`, `PullFiles`, `Compare`, `Checks`,
`Job`, `File`, `Protection`, `Settings`, `CiSetup`, `Collaborators`, `Permission`,
`Created`, `Commented`, `Reviewed`, `Merged`, `Queued`, `Branch`,
`Done`.

- **A paged answer says whether more follows** (`more`): `Items`,
  `Item` (its comments), `Reviews`, `Statuses`, `Remarks`, `PullFiles`
  and `Collaborators`. `Branches` and `Checks` carry none. The answer to
  `Items` also carries `now`, the forge's clock when it answered.
- **`CiSetup`** answers `Read::CiSetup` with `actions: Option<bool>`
  and `workflows: Option<bool>`. For Actions, true is enabled; for
  workflows, true is present and false is proven absent. None means the
  fact is unknown or unavailable. A denied, incomplete or malformed
  inventory never proves absence. These facts describe setup, not a
  head's CI verdict. Required checks come from `Protection`. A dialect
  that cannot establish a fact answers it unknown.
- **`Created`, `Commented` and `Reviewed`** carry the new object's
  number or id. `Merged` carries the merge's commit. `Branch` carries
  `BranchCreation`: `Created`, or `Exists`.
- **`Done`** answers `Update`, `Edit`, `SetReviewers`, `Close`,
  `Reopen`, `Status` and `DeleteBranch`.
- **`Pull` answers both `Pull` and `PullFor`;** the `Commit` answer is a
  branch's tip.
- **`Queued` is final for its call.** It answers `Merge` alone, and
  says the merge is the queue's now: neither a merge nor a refusal. The
  client keeps the write open until a read finds the pull request merged
  or out of the queue (forge-client.md, section 7). Forgejo's dialect
  never answers it.
- **What an answer says of the forge,** beside its cost, travels with
  it in the component's envelope (forge-connection.md, section 4), and
  each dialect fills what its forge sends:
  - **`Rate { limit, remaining, reset_at }`,** the rate's window: the
    requests the window allows, those left, and when it resets, in
    seconds since the epoch. The client's budget takes it in place of
    its configured window (forge-client.md, section 6). Forgejo's
    answers carry none.
  - **`Validator`,** a read's validator (an `ETag`), at most 128
    printable bytes (`VALIDATOR_BYTES`). A read sent with one that the
    forge answers `304` is answered as not modified, at no cost
    (`free`). Forgejo's dialect sends none.

### 3.4 The errors

20 errors. A call's `Error` is the first failure of its requests, and
5.4 says which failure gives which.

| Error | Means |
|---|---|
| `Unavailable` | the forge could not be reached, or the request provably was not read |
| `Timeout` | the request may have reached the forge: a write's outcome is unknown |
| `RateLimited { after }` | the forge refuses calls for `after` |
| `Forbidden` | the writer lacks the permission (a bad or lacking credential included) |
| `Missing` | the object is absent |
| `MissingJob` | the job attempt is absent, or is no longer the attempt asked for |
| `TooLarge` | a request or an answer passes a bound |
| `Empty` | the write has no content |
| `Full` | the forge has no room for the write (a quota) |
| `Exists` | the object already exists |
| `NothingToMerge` | the pull request would add nothing to its base |
| `Closed` | the target is closed or merged |
| `Stale` | the head the call required has moved |
| `Conflict` | the change conflicts with its base |
| `Protected` | a protection rule refused the write |
| `Refused` | the repository's settings, or the forge's policy, refuse the write |
| `InvalidAnswer` | a decoded answer does not match its call |
| `Busy` | the client's own admission refused the call; no API call was made |
| `NotConnected` | the forge is not connected, or stopped; no API call was made |
| `NotInstalled` | no installation of the App reaches the repository's owner; no API call was made |

- **`Busy` is never a dialect's.** It is the client's own refusal
  (forge-client.md, section 2.3). The component never produces it
  (forge-connection.md, section 7).
- **`NotConnected` and `NotInstalled` are the component's,** never a
  dialect's, and cost nothing (forge-connection.md, section 7).
  `NotInstalled` is GitHub's alone: the component chooses an
  installation by the repository's owner (section 8).
- **A pause is `RateLimited { after }`:** `Retry-After` when the forge
  gives it; else, when the window has none left, its reset less the
  forge's clock; else, for GitHub's secondary limits, 60 seconds
  (forge-github.md, section 7).
- **`Timeout` is the error that leaves a write's outcome unknown.** The
  client finds the outcome by the write's key, or by reading
  (forge-client.md, section 7).

### 3.5 The hint

`Hint { repository, change, key }` says that something may have changed.
It is not a fact.

- **`Change`:**
  - `Item(number)`: an issue or a pull request;
  - `Branch(branch)`: a branch was made or deleted;
  - `BranchMoved { branch, head, actor }`: a branch moved to a commit,
    pushed by the user `actor`;
  - `Commit(commit)`: CI moved at a commit.
- **`key`** is the key of the body that caused the hint, when the
  delivery carried one. The owner drops its own echoes by `key` and by
  `actor` (the pusher); the client reads on every hint it holds a
  resource for, and uses `actor` for drift alone (forge-client.md,
  section 4).
- **A hint decides nothing.** It brings a read sooner, never a decision
  (forge-client.md, section 4).
- **No request causes a hint.** It comes from a verified delivery
  (section 7).

### 3.6 The items

By name; the code holds the fields.

- **`Summary`:** one row of the change feed: number, kind, state,
  author, labels (for display), key, title, body and updated time.
- **`Comment`:** a comment, with its id, author, created time, its key,
  its `Provenance`, its `revision` and its body.
- **`Pull`:** a pull request's head and base branches and commits, its
  merge commit, whether it is mergeable, its reviewers, its `Ci`, and
  `queued`, its position in a merge queue when it is in one, and
  `dequeued`, the forge's words for why a queue last ejected it, at most
  `DEQUEUED_BYTES` (256) bytes.
- **`Review`:** a review, with its `Verdict`, its commit, its key, its
  `Provenance`, its `revision`, its time and whether it is official.
- **`Remark`:** a comment attached to a review at a path and a line.
- **`Status`:** a CI context: its time, context, `Check`, description,
  link and, for an Actions job, its `JobAttempt`.
- **`JobAttempt`:** a job's head, run, job and attempt.
- **`Step`:** a job's failed step, by number and name.
- **`File`:** a changed file: its path and its bytes before and after,
  either absent when the file was added or deleted.
- **`Protection`:** a rule's branch, required contexts, required
  approvals and whether it dismisses stale approvals; whether a merge
  queue guards it, and whether it requires signatures, linear history
  or code owners' reviews; and `Bypass`, whether the credential may pass
  it: `Never`, `PullsOnly` or `Always`. Forgejo's dialect reads signatures
  from `require_signed_commits`, and answers no queue, no linear history,
  no code owners and `Never`.
- **`Settings`:** a repository's default branch and the merge styles it
  allows.
- **`Collaborator`:** a user's id and `Permission`.

`Provenance` is `Original`, `Revised` or `Unknown`. A dialect that
cannot tell whether a historical creation was edited answers `Unknown`,
never `Original` from a missing field. A `revision` changes whenever the
body the forge keeps changes (5.3).

### 3.7 The rules on them

- **One answer per call,** and its cost: the number of requests the call
  wrote, the fills of the component's tables included, and 0 when it
  wrote none. The component reports the cost with the answer
  (forge-connection.md, section 4), and the budget charges it
  (forge-client.md, section 6).
- **Repositories by the forge's id.** A call names `Repository`; the
  dialect addresses the forge by the repository's path, which the
  component keeps for the id (forge-connection.md, section 4).
- **Locate by owner and name.** `Locate { owner, name }` names a
  repository's account and name, and is addressed with the forge's number
  and repository 0. It answers `Located` with the forge's id, which is
  never 0. The call's bytes are the lengths of its `owner` and `name`.
- **Every dialect answers every call.** A call that a dialect cannot
  answer as Forgejo does is named when that dialect is designed (section
  10). GitHub's one is `Status`, which answers `Forbidden` where the
  App lacks `statuses` write (forge-github.md, section 4.2).
- **An answer fits its call.** The client checks each answer against
  its call before it takes it (forge-client.md, section 2.3):

  | Call | An answer fits when |
  |---|---|
  | any | it has the shape in 3.2's table; a read's answer past `answer_bytes` or `rows` is `TooLarge`, a write's `InvalidAnswer`, a lost answer (forge-client.md, 7.4); any other mismatch is `InvalidAnswer` |
  | `Items` | `updated` never decreases; rows of one time rise strictly by number; numbers are nonzero and unique; each row's kind is the call's, when it names one |
  | `Item` | the number is the call's; comment ids rise strictly and are above `after` |
  | `Reviews` | ids rise strictly |
  | `Pull`, `PullFor` | the number is the call's; for `PullFor`, the head and base are the call's |
  | `PullFiles` | the head is the call's |
  | `Compare` | `before` and `after` are the call's |
  | `Checks` | each job's head is the call's commit |
  | `Job` | the attempt is the call's, and the log is within `max_bytes` |
  | `File` | the head and path are the call's, and the bytes are within `max_bytes` |
  | `Locate` | the id is nonzero |

- **The measure.** A call's or an answer's bytes are the lengths of the
  byte strings it holds, plus a fixed width for each element of each
  list in it; a value outside a list adds no width. `skein-forge`'s
  `measure::op(&Op)` and `measure::answer(&Answer)` give them, none when
  the sum passes a `u64`. The widths:

  | Element | Width |
  |---|---|
  | a `Summary`, a `Review` | 128 |
  | a `Status` | 160 |
  | a `Comment` | 96 |
  | a `Remark`, a `File` | 64 |
  | a `Commit` | 32 |
  | a `Collaborator`; a string in a list (a label, a branch, a context) | 16 |
  | an id in a list (a reviewer) | 8 |

  Each width is at least its type's size on the target, asserted when
  the crate is built, so the measure bounds memory and is one on every
  target. A row count is no part of it: `rows` is checked beside it.
  The page size (5.1), the client's checks (forge-client.md, section 2.3)
  and the component's limits (forge-connection.md, section 7) all use
  it.
- **A call is admitted within bounds.** The client admits an op only
  when its bytes are at most `op_bytes`, a `Job` or `File`'s `max_bytes`
  is between 1 and `answer_bytes`, a page is at least 1 and a file's
  path is not empty. The component and the plans never see one that is
  not (forge-client.md, section 2.3).
- **The vocabulary retains no state.** A row is a value, and the owner
  stores it as it likes.

## 4. Dialects and their call plans

### 4.1 What a dialect holds

A dialect renders the vocabulary on one forge's API. Forgejo's, at its
pin (4.3), holds:

- **Each request,** as a sized encoder: its bytes are counted before any
  is written, so a request's head and body are within the bounds the
  component gives.
- **Each answer,** as a decoder: a state machine over skein-json's
  tokens with a bounded stack, which keeps the few fields its answer
  needs and skips the rest as they pass (5.3).
- **The server's side of each document,** for the fake forge: the
  decoder of each request, and the encoder of each answer
  (fake-forge.md, section 4).
- **The failures:** error bodies, the refusals whose message tells two
  failures of one status apart, and the map from a status to an `Error`
  (5.4).
- **The deliveries:** the headers, the events subscribed, each payload's
  decoder and encoder, the table to hints and the listener's path for a
  forge (section 7).
- **The setup's documents:** the admin's reads, the bot, its tokens,
  orgs, a repository's read by id, collaborator and hooks, and the
  repositories the admin can see (forge-setup.md, section 4).
- **Names, times and ids:** percent-encoding of a path's segments, RFC
  3339 times in both directions, and object ids in 40 or 64 hexadecimal
  digits.
- **How a credential renders on a request** (section 8).
- **The start's reads,** two call plans that the component runs when a
  forge starts and decides from, counted in no call's cost
  (forge-connection.md, section 3):
  - **the identity read,** which says who the credential stands for:
    Forgejo's is `GET /api/v1/user`, for the user's `id`;
  - **the page cap read,** which gives the forge's page cap: Forgejo's
    is `GET /api/v1/settings/api`, for `max_response_items`, which must
    be above 0 (5.1).
- **The renewal,** for a renewed kind of credential: the plan that
  renews it, and how long before expiry it is due (section 8).
- **The pin** (4.3).
- **The call plans** (4.2).
- **Its limits,** `Limits`, which the component's owner gives:
  - `json` and `writer`: skein-json's tokenizer and sized writer;
  - `string_bytes`: the largest name, title, context, description, link
    or path kept;
  - `body_bytes`: the largest body kept whole;
  - `inner`: the most elements kept of a list inside one document;
  - `target_bytes`: the longest request target, its path and query.

  `check` refuses a zero, a `json.string` below `string_bytes` or
  `body_bytes`, and a depth below 8 (Forgejo's documents need about 8).
  The component's own limits (the connections, the bytes of a head or of
  a body read, the deadline) are forge-connection.md's, section 7.

A dialect holds no io, no clock and no connection.

GitHub's dialect, `skein_forge::github`, holds the same at its own pin,
with the App's documents (the JSON Web Token's bytes, the mint and the
installations' reads) where Forgejo's has the setup's admin reads
(forge-github.md, section 2).

### 4.2 Call plans

A call plan is a dialect's small pure machine for one call. The
component runs any dialect's plans on its connections, and the only
dialect knowledge it keeps is in the plans it is handed.

- **Its steps.** `begin` takes the call and returns a step. `heard`
  takes what the plan heard and returns the next. A step is one of:
  - `Send`: an exchange, to be made on the call's connection. It holds
    the request and a range: the head or the tail bytes of a raw answer
    to keep (a job's log is kept from its end, a file from its start);
  - `Done`: the call's `Answer` or `Error`.
- **What a plan hears is the dialect's type.** Forgejo's plans hear,
  for an exchange:
  - `Head`, with the status and the few header fields they read
    (`Date`, `Retry-After`, `X-HasMore`, `Content-Range` and
    `Connection: close`);
  - `Row`, for each element of a listing that the exchange's decoder
    keeps;
  - `Done`, with the body: a document the decoder read, raw bytes (and
    whether more follow), an empty body, or a malformed one;
  - or `Failed`: the exchange failed with the server never having seen
    the request (`Unseen`), or possibly having seen it (`Seen`).
- **It is handed its bounds as plain values,** not the client's
  `Limits`: the page size (5.1), `rows`, `answer_bytes` and `body_bytes`.
  The component takes them from the client's limits and its own, and
  checks them at start (forge-connection.md, section 7).
- **It does no io and keeps no connection.** The component makes each
  exchange, and tells the plan what it heard.
- **It resolves a repository's path and a user's login** in two tables
  that the component keeps and hands it: it reads a row, and fills one
  from the answers it hears (forge-connection.md, section 4). A row of
  a repository holds its `owner` and `name` (its path), the object
  format, the merge style and the `html_url`. The tables are bounded and
  evict the row
  used longest ago.
- **It ends with the call's result,** its `Done`, and counts nothing:
  the call's cost is the component's, one for each request whose first
  byte it wrote (forge-connection.md, section 4.1). A plan ends on the
  first failure, except where 5.2 says otherwise.
- **It maps a failure to an `Error`.** `classify` maps a status, a
  refusal's message and `Retry-After` to an `Error`, and says whether
  the path or the row is dropped (5.4).
- **It knows its dialect and its call,** and nothing of the budget or of
  the other calls in flight.

### 4.3 The pin

The dialect is pinned to one release of the forge, the one whose
behaviour the conformance checked.

- **Constants:** `VERSION`, Forgejo's release (17.0.0), and
  `RUNNER_VERSION`, forgejo-runner's (13.2.0).
- **Checks:** `names_forgejo` is true for the first line of
  `forgejo --version` when it names `VERSION`, ended by nothing, a `+`
  or a space; `names_runner` is true for the first line of
  `forgejo-runner --version` when it is `forgejo-runner version v`
  and `RUNNER_VERSION`, and nothing more.
- **Readers:** the throwaway Forgejo, the conformance, and a service's
  owned Forgejo check their binaries against the pin (fake-forge.md,
  section 6).
- **Digests:** the SHA-256 of each release's Linux binary, per
  architecture, as lowercase hex:

  | Constant | Of | Bytes | SHA-256 |
  |---|---|---|---|
  | `FORGEJO_SHA256_LINUX_AMD64` | `forgejo-17.0.0-linux-amd64` | 122,053,192 | `5a3b07e124ed70b4e46ee3939515f003bd3cc85352257a9a4607a527251042ca` |
  | `FORGEJO_SHA256_LINUX_ARM64` | `forgejo-17.0.0-linux-arm64` | 113,760,224 | `e5ec437cd6357e8d1b4580cb47ab1daa58a32b33971a63db63446a535d33adb7` |
  | `RUNNER_SHA256_LINUX_AMD64` | `forgejo-runner-13.2.0-linux-amd64` | 22,290,594 | `fadaec897f5e6641c363f87ecaf75f866f99c81b7097ec6b433e4c48123fcad1` |
  | `RUNNER_SHA256_LINUX_ARM64` | `forgejo-runner-13.2.0-linux-arm64` | 20,775,074 | `9d3d462ee83e4e629f959faeacceb20eed2f25290bddea402e65ace46a31afd9` |

  The sizes are not constants: they are what the digests were taken
  over, and what a fetch's bound must admit.
- **Sources:** each binary's release file, named by the constants and
  the architecture:
  - Forgejo's on codeberg.org,
    `https://codeberg.org/forgejo/forgejo/releases/download/v17.0.0/forgejo-17.0.0-linux-{amd64,arm64}`;
  - forgejo-runner's on code.forgejo.org,
    `https://code.forgejo.org/forgejo/runner/releases/download/v13.2.0/forgejo-runner-13.2.0-linux-{amd64,arm64}`.
    Codeberg serves no runner release (404).

  Each answers a single `200` with the file, and no redirect. A fetcher
  follows none, and takes any other status as the source failing.
- **TLS:** with the machine's roots. The digest, not the certificate, is
  what makes the file the pinned one.
- **An architecture without a digest** is refused: a fetcher names the
  architecture and fetches nothing. The pins are Linux on amd64 and
  arm64 alone.
- **Readers of the digests:** a service that fetches the pinned binaries
  (temper's `forge fetch`) checks each file it streams, and each file it
  finds already in place, against the digest of its own architecture.
  skein-forge does no io for this: it holds the constants and a
  `digest(binary, arch)` that answers the pinned digest, or none for an
  architecture without one.
- **Moving the pin** moves the digests with it: a new `VERSION` or
  `RUNNER_VERSION` comes with the new release files' digests, taken as
  above, in the same change.

## 5. Forgejo's API

How the Forgejo dialect uses the API, at its pin. The owner's values
are the word, the user agent, the credential and the limits (section
2.2). The operations are the calls of 3.2 and the start's reads (4.1);
the setup's requests are the dialect's too, and forge-setup.md,
sections 4.3 and 4.5, list them.

### 5.1 Conventions

- **Base, transport and authentication.** Every request goes to its
  forge's `api` address, under `/api/v1`, over HTTP/1.1: over TLS with
  the configured server name and trust, or in plaintext to a loopback
  address. Each carries `Authorization: token <t>`, the forge's API
  credential (section 8), which the component holds from start and never
  logs. `Host` is the transport's name, the TLS server name or the name
  a plaintext forge was configured by, else the address's IP, an IPv6
  one in brackets; either with `:port` when the port is not the scheme's
  default (443, 80). A TLS server name that is an IP address is written
  as the address's IP is, with the address's port, whichever IP the
  address holds: IPv4 in dotted decimal, IPv6 in brackets as its eight
  groups in lowercase hexadecimal without leading zeros, never
  compressed (`[2001:db8:0:0:0:0:0:7]`); a DNS name is written as it
  was given. `skein-forge`'s `host` makes it from the address and the
  name, for the component and a setup's host alike; no owner writes
  it. `User-Agent` is the owner's, given at start.
- **JSON in, JSON out.** Bodies are written with skein-json's sized
  writer (`Content-Type: application/json`), and answers decoded from
  its tokens. Raw files and job logs are read as bytes. Strings are
  UTF-8; text a call carries is bytes, and a write whose text is not
  UTF-8 is refused before it is sent (5.4).
- **Repositories by id.** A call names `Repository { forge, repository }`,
  `repository` being Forgejo's repository id. Forgejo's routes name a
  repository by path, so each request is made at
  `/api/v1/repos/{owner}/{name}`, from the path the component keeps for
  the id (forge-connection.md, section 4). A call whose repository has
  no path kept first resolves it with `GET /api/v1/repositories/{id}`,
  and that request counts in its cost. A locate names an owner and a
  name itself, and is addressed with the forge's number and repository 0
  (3.7).
- **Names in paths** (a repository's owner and name, a branch, a login,
  a file's path) are percent-encoded, each as one path segment; a slash
  inside a branch's name is `%2F`. A file's path keeps its slashes
  between segments.
- **Times.** Forgejo writes RFC 3339 times with an offset; they are read
  as the forge's clock, in nanoseconds since the epoch, at its
  resolution of a second. `since` is written in RFC 3339, in UTC, for
  any time the vocabulary names, up to the largest (`u64::MAX`
  nanoseconds, in 2554), with which the client reads the forge's clock
  alone: the answer is an empty page with its `Date`. The time an answer
  was made is its `Date` header, which a listing's answer carries as
  `now`.
- **Commits.** A commit is named by 32 bytes (a SHA-1 id zero-padded). A
  repository's object format (`object_format_name`, kept with its path)
  says whether its ids are 40 or 64 hexadecimal digits; anything else is
  a malformed answer. Ids are written in lowercase hexadecimal.
- **Paging is always explicit.** Every listing Forgejo pages is sent
  with `page` and `limit`; a listing without both is never sent:
  Forgejo answers with everything where it ignores the size. The size
  is fixed per forge and per listing: the smallest of the client's
  `rows`, the forge's `max_response_items` (read at the forge's start,
  4.1), and the number of that listing's largest rows the client's
  `answer_bytes` holds (5.3), each row by the measure of 3.7, so the
  size is the same on every target. A listing has
  more after it when it came full, or when Forgejo says so (`X-HasMore`
  on a pull request's files, `total_count` on a combined status).
  Listings Forgejo does not page are read whole and cut as they stream.
- **Users by id.** The vocabulary names people by their forge user id;
  the API names them by login in paths and bodies. The component keeps
  a table of the two (forge-connection.md, section 4).

### 5.2 Each operation

What each call sends, in order; R is `/api/v1/repos/{owner}/{name}`, from
the repository's path, and `<size>` the page size of 5.1. Calls of more
than one request make them in turn on one connection, and end on the
first failure, but where a row says otherwise.

| Call | Requests | Answer from |
|---|---|---|
| `Read::Items { since, page, kind }` | `GET R/issues?state=all&type=issues\|pulls&since=<since>&sort=leastupdate&page=<page>&limit=<size>`, `type` only with a kind | each row a `Summary` (5.3): `number`; a `pull_request` object makes it a pull; `state`; `user.id`; `labels[].name`, for display; `updated_at`; `title`; `body` and its key; `more` when the page came full; `now` from `Date` |
| `Read::Item { number, after }` | `GET R/issues/<number>`; `GET R/issues/<number>/comments` (unpaged) | the issue as a `Summary`; its comments with ids above `after`, oldest first, as many as the answer holds (5.3): `id`, `user.id`, `created_at`, `body` and its key, its provenance and revision; `more` when one is left |
| `Read::Pull { number }` | `GET R/pulls/<number>`; `GET R/commits/<head.sha>/status?page=1&limit=1` | `number`; `state`; `head.ref`, `base.ref`; `head.sha` as `commit`; `base.sha` as `base_commit`, none when empty; `merge_commit_sha` when `merged`; `mergeable`; `requested_reviewers[].id`; CI from the combined `state` and `total_count` |
| `Read::PullFor { head, base }` | `GET R/pulls/<base>/<head>`; when it names a pull request that is not open, `GET R/pulls?state=open&sort=oldest&page=<n>&limit=<size>` from page 1, until a row has the head and base or a page comes short; then the status read, as `Pull`, of the pull request answered | the open pull request for the head and base, else the one the lookup named; as `Pull` |
| `Read::Reviews { number, page }` | `GET R/pulls/<number>/reviews?page=<page>&limit=<size>` | submitted reviews, oldest first: `id`, `user.id`, `state` as the verdict, `commit_id`, `body` and its key, `submitted_at` as `at`, `official`, provenance and revision; `more` when the page came full |
| `Read::Statuses { commit, page }` | `GET R/commits/<commit>/status?page=<page>&limit=<size>` | CI from `state` and `total_count`; each context's latest status: `updated_at`, `context`, `status`, `description`, `target_url` as its link (links are absolute, below), no job; `more` while the pages read fall short of `total_count` |
| `Read::Remarks { number, review, page }` | `GET R/pulls/<number>/reviews/<review>/comments` (unpaged) | the `page`th slice of `<size>`, cut as it streams: `id`, `user.id`, `path`, `position` as `line`, `body`; `more` when a comment follows the slice |
| `Read::Branch { branch }` | `GET R/branches/<branch>` | `commit.id` |
| `Read::Branches { prefix }` | `GET R/git/refs/heads/<prefix>` | branch names from matching `refs/heads/` refs; no matches (`404`) gives an empty list and keeps the repository row; past the matching answer's bound, `TooLarge` |
| `Read::PullFiles { number, head, page }` | `GET R/pulls/<number>`; `GET R/pulls/<number>/files?page=<page>&limit=<size>`; for each file, `GET R/raw/<path>?ref=<merge_base>` (but for `added`; at `previous_filename` for `renamed`) and `GET R/raw/<filename>?ref=<head>` (but for `deleted`) | `head`; each file's `filename` with its contents at the merge base and at the head, a side absent when the file's `status` says so or its read answers `404`; `more` from `X-HasMore`; a `head.sha` other than `head` fails the call `Stale` before any file is read |
| `Read::Compare { before, after }` | `GET R/compare/<before>...<after>?verification=false`; `GET R/compare/<after>...<before>?files=false&verification=false`; for each file, `GET R/raw/<filename>?ref=<before>` and `?ref=<after>` | `files[].filename` with its contents at `before` and at `after`, a side absent when its read answers `404`; `commits[].sha`; `contains_before` when the second comparison's `total_commits` is 0; the whole comparison within the answer's bound, or `TooLarge` |
| `Read::Checks { commit }` | `GET R/commits/<commit>/status?page=<n>&limit=<size>`, every page; for each failing status whose link names a run of this repository's Actions: `GET R/actions/runs?head_sha=<commit>&run_number=<n>&page=1&limit=1`, then `GET R/actions/runs/<id>/jobs` | each context's latest status as for `Statuses`, its `job` the commit, the run's `id`, the job's `id` and its `attempt` when found, none otherwise |
| `Read::Job { attempt, max_bytes }` | `GET R/actions/runs/<run>`; `GET R/actions/jobs/<job>`; `GET R/actions/jobs/<job>/logs?attempt=<attempt>&step=<number>` with `Range: bytes=-<max_bytes>`, `step` the failed step's, absent when no step failed | the failed step, its `number` and `name` (5.3), none when no step failed; the last `max_bytes` bytes of that step's log, or of the job's whole log when no step failed; `truncated` when `Content-Range` names bytes before them; `MissingJob` when a read answers `404`, the run's `commit_sha` is not the attempt's head, or the job's `run_id` or `attempt` is not the attempt's |
| `Read::File { head, path, max_bytes }` | `GET R/raw/<path>?ref=<head>` with `Range: bytes=0-<max_bytes>` | the first `max_bytes` bytes; `truncated` when more came |
| `Read::Protection { branch }` | `GET R/branches/<branch>`; then `GET R/branch_protections/<effective_branch_protection_name>` when it names a rule, or `GET R/branch_protections` when it names none | `rule_name`, `status_check_contexts` when `enable_status_check`, `required_approvals`, `dismiss_stale_approvals`, `require_signed_commits`; none when no rule applies; a `403` (a writer who is not an admin) `Forbidden` |
| `Read::Settings` | `GET /api/v1/repositories/<id>` | `default_branch`; `allow_merge_commits`, `allow_squash_merge`, `allow_rebase`; the repository's row kept afresh |
| `Read::Collaborators { page }` | `GET R/collaborators?page=<page>&limit=<size>`; for each user, `GET R/collaborators/<login>/permission` | each user's `id` and permission; `more` when the page came full |
| `Read::Permission { user }` | `GET R/collaborators/<login>/permission` | `permission`: `none`, `read`, `write`, `admin`, and `owner` as `Admin` |
| `Read::Locate { owner, name }` | `GET /api/v1/repos/<owner>/<name>` | `Located { repository }` from `id`; the repository's row kept |
| `Write::CreateIssue { key, title, body }` | `POST R/issues` `{title, body}`, the body behind the key's marker | `Created` from `number` |
| `Write::OpenPull { title, body, head, base }` | `GET R/branches/<head>`; `GET R/branches/<base>`; `GET R/compare/<base's commit.id>...<head's commit.id>?files=false&verification=false`, of which only `total_commits` is kept; when it is 0, nothing more; else `POST R/pulls` `{head, base, title, body}` | `Created` from `number`; `NothingToMerge` when `total_commits` is 0, the pull request not written |
| `Write::Post { number, key, body }` | `POST R/issues/<number>/comments` `{body}`, behind the key's marker | `Commented` from `id` |
| `Write::Review { number, key, verdict, body }` | `POST R/pulls/<number>/reviews` `{event, body}`, `event` `APPROVED`, `REQUEST_CHANGES` or `COMMENT`, the body behind the key's marker | `Reviewed` from `id` |
| `Write::Edit { number, title, body }` | `PATCH R/issues/<number>` `{title, body}`, each only when given | `Done` |
| `Write::SetReviewers { number, reviewers }` | `GET R/pulls/<number>`; `POST R/pulls/<number>/requested_reviewers` `{reviewers: [logins]}` for those missing; `DELETE R/pulls/<number>/requested_reviewers` with the same body for those extra | `Done` |
| `Write::Close { number }` | `PATCH R/issues/<number>` `{state: closed}` | `Done` |
| `Write::Reopen { number }` | `PATCH R/issues/<number>` `{state: open}` | `Done` |
| `Write::Merge { number, head }` | `POST R/pulls/<number>/merge` `{Do, head_commit_id: <head>}`; `GET R/pulls/<number>` | `Merged` from `merge_commit_sha` (the merge answers with no body) |
| `Write::Update { number }` | `POST R/pulls/<number>/update?style=merge` | `Done` |
| `Write::Status { commit, context, check }` | `POST R/statuses/<commit>` `{state: pending\|success\|failure, context}` | `Done` |
| `Write::CreateBranch { branch, commit }` | `POST R/branches` `{new_branch_name: <branch>, old_ref_name: <commit>}` | `Branch(Created)`; a `409` (the branch exists) `Branch(Exists)` |
| `Write::DeleteBranch { branch }` | `DELETE R/branches/<branch>` | `Done` |

`Branches` requires a nonempty prefix bounded like a branch name. An empty
prefix ends with `Error::Empty` before repository resolution or any GET.
Only matching names count against the row and answer-byte bounds.

Notes on the table:

- **The keep-up listing** is the only listing of items: always
  `state=all`, always `since`, so its rows are what changed since the
  last pass and its cost follows the rate of change, not the number of
  pull requests held (forge-client.md, section 4). A find for an issue
  lists from the entry's start, `type=issues`. Nothing is listed by
  label.
- **`sort=leastupdate`** is Forgejo's least-recently-updated order. Its
  default is `latest`, newest *created* first, and a GitHub-style
  `sort=updated&direction=asc` is not in its vocabulary: a client that
  sends the wrong words gets the wrong order with no error. The fake
  forge answers the same way (fake-forge.md, section 4), and section 9
  names the real one's facts.
- **An item's comments are not paged by Forgejo.** The call reads the
  whole thread, keeps the comments with ids above `after` as the answer
  holds them, and skips the rest as they stream, bodies unread; the next
  call names the last comment kept as its `after`. Section 12 weighs
  long threads.
- **Issues and pull requests share one numbering,** so an `Item` read of
  a pull request is its conversation, and `Edit`, `Close` and `Reopen`
  are the issue routes for both.
- **Reviews** are mapped by their state: `APPROVED` to `Approve`,
  `REQUEST_CHANGES` to `RequestChanges`, `COMMENT` to `Comment`. Pending
  reviews and review requests (`PENDING`, `REQUEST_REVIEW`) are not
  reviews, and are skipped, though they count towards a page coming
  full. A dismissed review is carried as `Comment`: its words and its
  key stay, its verdict does not (section 12).
- **CI.** A combined state of `failure` or `error` is `Failed`, `pending`
  is `Pending`, `success`, `warning` and `skipped` are `Passed`, and a
  `total_count` of 0 is `None`, whatever the state says. A status's own
  state maps the same way to a `Check`. Forgejo's Actions report each
  job as a status on the commit it ran on, so CI is read from statuses
  alone, never from a history of runs.
- **Job attempts.** An Actions status links to its job's page,
  `<the path of the repository's html_url>/actions/runs/<run
  number>/jobs/<job index>`, relative to the forge's origin, as Forgejo
  writes it (section 9); the same link with the whole `html_url` before
  it is read the same way. The run's number is its `index_in_repo`,
  which the runs' listing filters by (`run_number`); the job is the one
  at that index of the run's jobs; its attempt is the job's `attempt`. A
  link of another shape, one that starts `//` or names another scheme or
  host, or a run or a job not found, leaves the status's `job` none, and
  the call answers all the same.
- **A failed job's log is its failed step's end.** Forgejo answers a job
  (`actions/jobs/<job>`) with its steps, those of its latest attempt:
  number 0 is the runner's set-up (`Set up job`), the last its
  completion (`Complete job`), and the workflow's own steps are between.
  The failed step is the first whose `status` is `failure`: the
  completion step reports `failure` whenever the job fails, so it is the
  failed step only when none before it failed. The steps are read as
  they stream, and only the failed one is kept, the others skipped as
  they pass: a job of any number of steps is read, its steps costing
  time, not memory. Its log alone is read (`step`), and of it the end,
  by a suffix range, since a step tells its failure last. A job with no
  step failed (one cancelled, or one whose runner was lost) is read
  whole, its end the same way; one never given to a runner has no log
  (`404`, `MissingJob`). The job's `attempt` is its latest: one other
  than the call's (the job ran again since) is `MissingJob`, as a log
  Forgejo no longer keeps is (`404`). A `200` is read through, its last
  `max_bytes` bytes kept as it streams, within the component's
  `body_bytes_in` like any body (forge-connection.md, section 7); a
  range over an empty log is an empty log (section 9). The log's search
  (`q`) and its NDJSON form are not read: neither answers a range.
- **Links are absolute.** A status's link is carried to the owner
  whole: one relative to the forge's origin (a `target_url` that starts
  with one `/`) gets the origin (scheme, host and port) of the
  repository's `html_url` before it, so a person or an agent reading it
  has a link that opens; any other is carried as Forgejo wrote it.
- **Who posted a status is not carried.** A status written by Actions
  has no `creator`, and nothing in the vocabulary reads a status's
  poster: CI is read by context and state. `Status` has no author.
- **Contents of a pull request's files and of a comparison** are read
  file by file, two reads each, since the vocabulary carries each
  changed file whole on both sides (`File`); the reads count in the
  call's cost. A side that does not fit what is left of the answer's
  bound fails the call as `TooLarge`. A comparison's files are
  Forgejo's (from the two commits' merge base); their sides are read at
  `before` and at `after`.
- **Merges.** `Do` is the repository's `default_merge_style`, kept with
  its path (forge-connection.md, section 4): the merge style is the
  repository's own. `head_commit_id` makes the merge conditional on the
  head the call names, which Forgejo refuses when it moved (`Stale`).
  The merge answers with no body; the read after it gives the merge's
  commit. If that read fails, the call answers `Timeout`: the merge's
  outcome is unknown, and the client finds it by reading the pull
  request (forge-client.md, section 7).
- **Updates** merge the base into the branch (`style=merge`), never a
  rebase.
- **A pull request for a head and base.** Forgejo keeps at most one pull
  request open for a head and a base. Its lookup, `pulls/{base}/{head}`,
  names one of several, the oldest (section 9), and its listing has no
  head or base filter. So `PullFor` answers the open one: the lookup's,
  or, when the lookup names one closed or merged, the row of the open
  pull requests' listing whose `head.ref`, `head.repo_id` and `base.ref`
  are the call's head, its repository and its base, every other row
  skipped as it streams. With none open, it answers the one the lookup
  named. An owner that opens a pull request only when none exists for its
  branches, and reopens it when it was closed, follows the oldest, and
  the listing is read only when that one is closed or merged, or another
  hand opened one for its branches. Each page counts in the call's cost;
  the call's deadline bounds them.
- **Comparisons ask for what they keep.** Every comparison sends
  `verification=false`, since no commit's signature is read; one read
  only for its `total_commits` (`OpenPull`'s, and `Compare`'s second)
  also sends `files=false`, so Forgejo lists no commit's files.
  `files=false` leaves the comparison's own `files` empty too (section
  9), so `Compare`'s first comparison, which keeps them, never sends it.
  The answer is decoded in the same way: what the call does not keep
  is skipped as it streams.
- **Opening compares first.** Forgejo opens a pull request whose head
  adds nothing to its base (section 9), which the vocabulary refuses as
  `NothingToMerge`. So `OpenPull` reads both branches and compares their
  commits before it writes: a head with no commit its base lacks
  (`total_commits` 0, a head equal to its base included) fails the call
  `NothingToMerge`, and nothing is written. The three reads count in the
  call's cost, and fail as 5.4 says for them. A base that moves to
  contain the head between the comparison and the `POST` still gets a
  pull request with nothing to merge; that window is the only way a
  pull request with nothing to merge is opened.
- **Keys.** `CreateIssue`, `Post` and `Review` write the key's marker,
  then the text (section 6). A pull request's key is its branches.
  `Edit` writes the title and body it is given.
- **Several requests, one call.** `Item`, `Pull`, `PullFor`,
  `PullFiles`, `Compare`, `Checks`, `Job`, `Protection`,
  `Collaborators`, `OpenPull`, `SetReviewers` and `Merge` make more than
  one; a path to resolve or a login to look up adds one. A `404` on a
  file's side, or on a failing status's run or job, ends nothing: it is
  an answer, and the repository's path stays. The client asks for what
  it needs, and the answer's cost tells its budget what that took
  (forge-client.md, section 6).
- **Success.** Each request succeeds with one status; any other `2xx`
  is a malformed answer (5.3). A read's is `200`, and a raw file's or a
  log's `200` or `206`. A write's is `201`, as Forgejo answers its
  creations and its edits of an issue (`Edit`, `Close`, `Reopen`), but
  `200` for `Review`, `Merge` and `Update`, and `204` for
  `DeleteBranch`; `SetReviewers` adds with `201` and removes with
  `204`. The reads a write makes succeed as reads do.

### 5.3 Decoding answers

- **Streams, not documents.** Each answer's decoder is a state machine
  over skein-json's tokens with a bounded stack, pulled as the body
  arrives (json.md, section 3.1). It keeps only the fields its answer
  needs, as plain values; every other field, nested object and array is
  skipped as it passes.
- **A string is kept whole, or not at all.** skein-json keeps a string
  only when it fits the cap its reader names, and otherwise reads it
  through and says its length (`Text` and `Long`, json.md, section 3.1).
  So:
  - a name, a title, a context, a description, a link or a path is kept
    within `string_bytes`; a longer one makes the answer malformed;
  - but for a job step's name, which Forgejo makes of the step's whole
    script when the workflow names none: a longer one is carried empty,
    and the step is named by its number alone, so that a long script
    never makes a job unreadable;
  - a body (an issue's, a comment's, a review's, a remark's) is kept
    whole within `body_bytes`. A longer one is carried empty, its key
    none, its provenance `Unknown`, and its revision made from its
    length (below); the component counts it among its diagnostics.
    `body_bytes` is at least the largest body the owner writes, its
    marker included (the client's `op_bytes`), checked at start, so the
    owner's own creations are always read whole and their keys found;
  - every string is read as UTF-8 by skein-json; a body that is not
    valid UTF-8 is a malformed answer.
- **The answer's bound.** The page size (5.1) holds a full page of a
  listing's largest rows within the client's `answer_bytes`, so a paged
  answer never passes it. An item's comments are kept until the next
  would pass `answer_bytes` or `rows`, and `more` is then set. Raw
  contents are read within what is left of `answer_bytes`: a file's
  after its path, a job's log after its step's name, so the answer never
  passes it. `File` and `Job` are cut there as at their `max_bytes`, and
  say so (`truncated`); a side of `PullFiles` or `Compare` is never cut
  (5.2).
- **Revisions.** A comment's or a review's revision is the first eight
  bytes, big-endian, of the SHA-256 of its whole body as the forge
  returns it, computed as the string streams past; for a body not kept,
  of its length and its `updated_at`. So it changes whenever the body
  does, at any resolution of the forge's clock, and names what the forge
  stored, not what was sent.
- **Provenance.** `Original` when `updated_at` equals `created_at` (a
  review's `submitted_at`), `Revised` when it is later, `Unknown` when
  either is missing or the body was not kept. Forgejo's clock has a
  second's resolution: an edit within the second of the creation reads
  as `Original` (section 12).
- **Markers** at the head of a body, a comment or a review are decoded
  into its `key` (section 6).
- **What may follow** is told by the page coming full, `X-HasMore`, or
  `total_count`. For a call made of several requests, `more` is its
  listing's.
- **Absent headers.** An `Items` answer without `Date` is malformed,
  since its `now` becomes the next listing's `since`. A `429` without
  `Retry-After` is `RateLimited { after: 0 }`, and a listing without
  `X-HasMore` has more only when it came full.
- **A malformed answer** (a success status other than its request's,
  5.2; a document that does not decode, a field missing, a number out
  of range, a commit id of the wrong length, a string past its cap, a
  body past the component's `body_bytes_in`, a log read through
  included; a repository read by its id answering another) fails a
  write as `Timeout`: the request reached the forge, so what it asked
  may have been done. It fails a read as `InvalidAnswer`. Either way the
  connection is closed, since what followed on it can no longer be
  trusted. A repository id past the vocabulary's 32 bits fails the call
  as `TooLarge`.

### 5.4 Failures

A call's `Error` is the first failure of its requests. "Nothing written"
means the call failed before any byte of its first request was written;
such a request costs nothing.

| What happened | `Error` |
|---|---|
| nothing written: no connection, a refused connect, a TLS handshake that failed; or the server never saw the request (skein-http's `Closed`); or the call's deadline while it waited | `Unavailable` |
| a `3xx`, as Forgejo answers a moved repository's old path before it acts: the path dropped | `Unavailable` |
| `429`, with `Retry-After` (seconds or a date), before any of the call's writes succeeded | `RateLimited { after }`: the wait, a date taken from the answer's `Date` |
| anything after a byte was written: a reset, the end of the stream, the call's deadline, `5xx`; a `429` after one of the call's writes succeeded (`Merge`'s read after its merge) | `Timeout` |
| a malformed answer (5.3) | `Timeout` on a write; `InvalidAnswer` on a read |
| `401`, `403` | `Forbidden` |
| a `404` on a branch leaf (`reads::Stage::Read`, `Read::Branch`) or contents leaf (`contents::Stage::File`), classified as `Asked::Leaf` | `Missing`, `Drop::Nothing`: the repository row stays |
| every other `404` that ends the call (one 5.2 says ends nothing is an answer, and the path stays), including repository resolution, its own GET and listings | `Missing`, the repository's path dropped; `MissingJob` on `Job`'s reads |
| `413` (Forgejo's quota) | `Full` |
| `423` (the repository is archived) | `Forbidden` |
| `409` on `OpenPull` (a pull request for the head and base is open) | `Exists` |
| `OpenPull`'s comparison counting no commit (`total_commits` 0): the pull request not written | `NothingToMerge` |
| on `OpenPull`'s reads, before its `POST`: what this table makes a `Timeout` or an `InvalidAnswer` (the pull request was not written) | `Unavailable` |
| `422` on a write whose title or body is empty | `Empty` |
| `405` on `Merge` for a pull request merged or closed | `Closed` |
| `405` on `Merge` for a conflict | `Conflict` |
| `405` on `Merge`, `403` on `DeleteBranch`, for a branch's protection | `Protected` |
| `405` on `Merge` for a style the repository does not allow | `Refused`, the repository's row dropped |
| `409` on `Merge`, `head out of date` | `Stale` |
| `409` on `Update`, `merge failed because of conflict` | `Conflict` |
| a pull request's head other than `PullFiles` names | `Stale` |
| an answer past the component's bounds | `TooLarge` on a read; `Timeout` on a write, which may have been done |
| a write whose text is not UTF-8, which JSON cannot carry; nothing written | `Refused` |
| any other `4xx` | `Refused` on a write; `InvalidAnswer` on a read |

A repository gone or inaccessible loses its row at its next repository-level
call. An absent branch or contents leaf alone keeps the row that answered.

- **The plan gives the `Error`,** from what it hears (4.2). The
  component tells it how an exchange failed, `Unseen` or `Seen`, from
  skein-http's errors and its own deadlines (forge-connection.md,
  section 8).
- **Unavailable is provable only before the first byte.** skein-http's
  client tells a request the server never saw (`Closed`) from one it
  may have read (`Truncated`, `Stream`; http.md, section 3.4). A
  connection kept alive that the forge closed just as a request went out
  may have been read: it is a `Timeout`. To make that rare, an idle
  connection is closed before the forge's keep-alive timeout would
  close it (`idle`, forge-connection.md, section 7).

- **One status, several meanings.** Where Forgejo answers two failures
  with one status (`405` on a merge), the decoder reads its `message`
  into the failure. The dialect names a refusal only when both the
  status and the whole message match: a merge on a closed pull request,
  a conflict, a protection, a style, or a head out of date; an update
  with a conflict; an empty title or body. The messages are those the
  pinned release answers, in the conformance's transcripts (section 9).
  A `4xx` means Forgejo did not act, so a message the decoder does not
  know is `Refused` on a write, never a guess at which failure it was;
  on a read, `InvalidAnswer`.
- **Not errors:** a `409` on `CreateBranch` is `Branch(Exists)`, which
  the client follows with a read of the branch; a `404` on
  `DeleteBranch` is `Missing`, which the client counts as the branch
  gone (forge-client.md, section 7). Forgejo answers the deletion of a
  branch that is not there `500` with an empty body (section 9): a
  `Timeout`, after which the client's find reads the branch, and that
  read's `404` is `Missing`, the branch gone.
- **Reads before a write.** `OpenPull`'s reads come before its `POST`,
  so a failure among them leaves the pull request provably not written:
  what would be a `Timeout` or an `InvalidAnswer` is `Unavailable`, and
  the client may send the call again. Their other failures are as the
  table says (a `404`, a branch not there, `Missing`; a `429`
  `RateLimited`; a `401` or a `403` `Forbidden`).
- **The deadline.** Each call has the component's `deadline`, which the
  component checks at start is at most the client's `lifetime`, so no
  byte of a write is sent past the time the client counts on. A call
  still waiting at its deadline is `Unavailable`; one in flight is a
  `Timeout`, and its connection is aborted.
- **Never `Busy`.** The component holds as many calls as the client may
  have in flight (its `calls`), so it never refuses one: `Busy` is the
  client's own admission.

## 6. Keys' markers

What the kit writes inside what it creates: keys' markers, and nothing
else. No record, outcome or page of the owner's is written into the
forge, and no call of 5.2 writes a label: labels are for display only,
and no decision reads one.

- **A creation's key,** at the head of an issue's body, a comment or a
  review: `<!-- `, the word, `:key `, the key in lowercase hexadecimal,
  then ` -->` and a line feed, then the text. It is an HTML comment,
  which Forgejo's rendering hides. A pull request has no marker: its key
  is its branches.
- **The word is the owner's.** It is 1 to 16 bytes of lowercase ASCII
  letters, digits and `-`, checked at start. The same word marks the
  setup's bot, `<word>:bot` (forge-setup.md, section 4).
- **Services that share a forge** each have their own word, their own
  bot and their own keys' namespace. A body whose marker carries
  another word has no key.
- **The key is the owner's bytes.** The client builds it with
  `effect_key(namespace, identity, max)` (forge-client.md, section 7):
  - version 1, one byte;
  - the namespace's length, as two bytes big-endian;
  - the namespace, which is the owner's;
  - the identity, which is the owner's opaque bytes for the effect.

  The key's first byte is its version; the marker has none of its own.
  Owners that share a repository tell their creations apart by
  namespace.
- **Decoded** from a body whose first bytes are `<!-- `, the word and
  `:key `, then an even count of lowercase hexadecimal digits, then
  ` -->` and a line feed; any other body has no key.
- **The marker code only encodes and decodes,** and never reads the key.
  Whether to believe a key is the client's call: its version, its
  namespace, and whether its author is a configured writer
  (forge-client.md, section 7). A person who copies a marker into their
  own comment gives it no provenance.
- **Bounded.** The key is within the client's `op_bytes`. The marker is
  twice the key's length plus the word's length plus 15 bytes: with a
  6-byte word, twice the key plus 21. The body sent is the marker and
  the text, which `body_bytes` holds (5.3).
- **One code serves every use:** the writes' encoders, and the decoders
  of items, comments, reviews and deliveries.

## 7. Deliveries

Forgejo's deliveries, made hints: they cut polling's latency, and
decide nothing. Polling stands with no webhook (forge-client.md,
section 4), and finds whatever a delivery lost, refused or never sent
told of. The listener, its connections and its caps are
forge-connection.md, section 6. This section is what the dialect holds.

- **The listener's path.** A forge's deliveries arrive at
  `/hooks/<number>`, the forge's number in decimal with no sign and no
  leading zero, within 16 bits. A path of another shape names no forge.
- **Signed, and verified as it streams.** Forgejo signs each delivery's
  body with its hook's secret: HMAC-SHA256, in lowercase hexadecimal, in
  `X-Forgejo-Signature`. The component computes it as the body arrives,
  with skein lib's `sha256::Hmac` keyed by the forge's secret, and
  decodes the body at the same time into the few fields its hint reads;
  nothing of it is believed until the whole body is read and the
  signature holds, compared with `sha256::equal`, in constant time
  (lib.md, section 6.2).
- **Its verdicts:**

  | A delivery | Answer | Hint |
  |---|---|---|
  | with no signature | `403` from its head, its body unread | none |
  | with a wrong signature | `403` once its body is read | none |
  | whose body length passes its cap | `413` | none |
  | verified, and its body malformed | `400` | none |
  | verified, and an event of no row below (a tag's included) | `204` | none |
  | verified, and mapped | `204`, queued first | one |

  An unknown path, a forge with no secret and a method other than `POST`
  are the listener's (forge-connection.md, section 6).
- **Answered first.** Once the body is read and verified, the response
  (`204`) is queued, and only then does the hint go up. The component
  never makes Forgejo wait on its owner's work. skein's HTTP server lets
  the component answer when it chooses (http.md, section 5.4).
- **The events a hook subscribes to** are every event whose delivery the
  table maps to a hint, and no other: 22 in all. The component reads
  each delivery by `X-Forgejo-Event`, the header it arrives under, which
  several events share; each hint's repository is the forge's number and
  the payload's `repository.id` (`run.repository.id` for a job's), so no
  path is needed (5.1). A repository id past 32 bits makes no hint.

  | Events subscribed | `X-Forgejo-Event` | `Hint` |
  |---|---|---|
  | `issues`, `issue_assign`, `issue_label`, `issue_milestone` | `issues` | `Change::Item(number)`, with the key of the issue's body |
  | `pull_request`, `pull_request_assign`, `pull_request_label`, `pull_request_milestone`, `pull_request_sync`, `pull_request_review_request` | `pull_request` | `Change::Item(number)`, with no key: a pull request's key is its branches (section 6) |
  | `issue_comment`, `pull_request_comment` | `issue_comment` | `Change::Item(number)`, with the key of the comment's body |
  | `pull_request_review_approved`, `pull_request_review_rejected`, `pull_request_review_comment` | `pull_request_approved`, `pull_request_rejected`, `pull_request_comment` | `Change::Item(number)`, with the key of the review's body |
  | `push` | `push` | of a branch, `Change::BranchMoved { branch, head, actor }`, the commit it moved to and the pusher's id; of a branch deleted (its `after` all zeros), `Change::Branch(branch)` |
  | `create`, `delete` | `create`, `delete` | of a branch, `Change::Branch(branch)` |
  | a job's ends: `workflow_job_success`, `workflow_job_failure`, `workflow_job_cancelled`, `workflow_job_skipped` | the same | `Change::Commit(commit)`, the run's `commit_sha` |
  | none | anything else; a tag's `push`, `create` or `delete` | nothing |

  A body not kept whole (5.3) gives no key. The decoder yields a
  `Delivery`: `Issue`, `Pull`, `Comment`, `Review`, `Push`, `Ref` or
  `Job`, a job's end being `Success`, `Failure`, `Cancelled` or
  `Skipped`. Each row's events, its header and the fields its hint reads
  are recorded on the pinned release (section 9). A row that went wrong
  would cost a poll's wait, never a decision: a hint only brings a read
  sooner (forge-client.md, section 4).

  **Who caused it** is what lets the owner drop its own writes' echoes,
  by key (a creation's, for an item) and by pusher (for a push); the
  client reads on every hint it holds a resource for, and uses the
  pusher for drift alone (forge-client.md, section 4). A job's delivery
  has no top-level repository or sender: its `run.repository.id` and
  `run.commit_sha` stand for them, and its hint names the commit and no
  actor. A hint for a repository the owner has not adopted goes up all
  the same; the owner drops it.
- **CI is hinted as each job ends.** A job's delivery is sent for each
  of its attempts, so a job run again is hinted again when it ends. The
  run's deliveries (`workflow_run_*`), sent once its last job ends, and
  the deprecated `action_run_*` are not subscribed.
- **What has no delivery** is found by polling alone (forge-client.md,
  section 4): a commit status, which Forgejo never delivers, so CI that
  a system other than Actions reports and the verdicts the owner posts
  as statuses.
- **What Forgejo settles after it delivers** (a pull request's
  mergeability, a job's commit status: section 9) is read again by the
  owner's fresh reads, within its `settle` of the move; the client runs
  no window for it (forge-client.md, sections 4 and 5). The component
  hints at once.
- **Bounded.** Push payloads list commits, which the decoder skips. The
  listener's connections, head, body and deadline are capped
  (forge-connection.md, sections 6 and 7).
- **Not sent again.** Forgejo does not resend a failed delivery: one
  lost while the owner is down, refused for its size or its signature,
  or never sent is found by polling. A hint that arrives before the
  owner's domain is ready is dropped, and the keep's first listing finds
  what it told of.
- **Hooks are set up by reconcile, or by the person,** at the listener's
  URL for the forge, with the forge's secret, the events above, JSON and
  active (forge-setup.md, section 4.5). A repository with no hook polls.

## 8. Credentials

- **A credential has a kind, and each dialect names the kinds it
  takes:**
  - **a token,** which lasts until revoked: Forgejo's, or GitHub's
    fine-grained token;
  - **an App** (GitHub's: `Credential::App { id, key }`): the App's id,
    and its private key, an RSA key of 2048 bits as PKCS#1 DER. The
    component signs a JSON Web Token with it (RS256, through `ring` as
    `skein_tls::sign_rs256` has it), mints from that each installation's
    token, which expires within the hour, and chooses the installation
    by a repository's owner (forge-connection.md, section 5). The
    dialect gives the token's bytes and the mint (forge-github.md,
    section 8); GitHub's tokens render as `Authorization: Bearer <t>`.
- **The owner gives the value** at start, for each forge (section 2.2).
  The component reads no file.
- **The dialect says how a credential renders on a request.** Forgejo's
  is `Authorization: token <t>` (5.1). The request's head, that field
  among them, fits skein-http's `request` limit: the component bounds
  the credential's size at start (forge-connection.md, section 7).
- **For a renewed kind,** the dialect also gives the plan that renews
  the credential, and how long before expiry it is due. The component
  runs it (forge-connection.md, section 5).
- **A credential's value reaches no record, request line, fact or
  hint.** It is not logged, granted or stored.
- **A credential that is expired or revoked** reads as `Forbidden` on
  every call (section 12). An App's key that GitHub refuses (a `401` on
  the JSON Web Token) is the same.

## 9. What skein-forge relies on of Forgejo

What the component and the client rely on of Forgejo, on the pinned
release (4.3). Each is named by its fact in the conformance's record
(fake-forge.md, section 7), where it `holds`; a fact marked *fails* is
phrased the other way there, and its failing is what the dialect relies
on. What no run records yet has its own bullet, near the end.

- **Every call of 5.2, made by the component.** Each read and write
  answers as its row says, its cost the requests it sent, and the keys
  the owner writes are found on reading back; a forge's start is ready,
  and another user or a wrong token stops it (every `component-` fact:
  one per call, from `component-read-items` to
  `component-write-delete-branch`, with
  `component-read-pull-for-open-after-closed` and
  `component-write-open-pull-no-changes`; one per start's outcome, from
  `component-start-ready`; `component-costs-equal-requests-sent`;
  `component-keys-found-on-read-back`).
- **Paging.** With Forgejo's default settings, `max_response_items` is
  50 and `default_paging_num` 30 (`api-default-page-cap-50-and-30`). A
  pull request's files are paged, with `X-HasMore`
  (`pull-files-page-with-x-hasmore`); a comparison ignores `page` and
  `limit` and answers everything (`compare-ignores-page-and-limit`).
- **The keep-up listing.** `sort=leastupdate` lists the least recently
  updated first, ties by number
  (`leastupdate-orders-least-recently-updated-first`,
  `leastupdate-ties-ordered-by-number`); `since` is inclusive at the
  second (`since-is-inclusive-at-the-second`), and in 2554 answers an
  empty page with its `Date`
  (`since-in-2554-answers-an-empty-page-with-date`). An item's updated
  time moves on a comment, a review, a push to its head, closing and
  reopening (`updated-moves-on-comment`, `updated-moves-on-review`,
  `updated-moves-on-head-push`, `updated-moves-on-close`,
  `updated-moves-on-reopen`), and stays on a comment's edit or
  deletion, a status and a push to its base
  (`updated-stays-on-comment-edit`, `updated-stays-on-comment-delete`,
  `updated-stays-on-head-status`, `updated-stays-on-base-push`): so a
  pull request's CI and base are read on a backoff (forge-client.md,
  section 4).
- **Bodies and their versions.** A marker's line feed is kept
  (`marker-line-feed-kept`); bodies of 64 KiB and 1 MiB are kept whole
  (`body-of-65536-bytes-kept-whole`, `body-of-1048576-bytes-kept-whole`).
  An unedited comment's or review's updated time is its creation's
  (`unedited-comment-updated-equals-created`,
  `unedited-review-updated-equals-submitted`), and an edit moves it
  (`edit-moves-comment-updated`). An issue's comments and a pull
  request's reviews come oldest first (`comments-come-oldest-first`,
  `reviews-come-oldest-first`); a dismissed review keeps its body
  (`dismissed-review-keeps-its-body`); a remark's `position` is its line
  (`remark-position-is-its-line`).
- **Pull requests.** `PATCH issues/{index}` edits a pull request's title
  and body (`patch-issue-edits-a-pull-request`); its `base.sha` is its
  base's tip (`base-sha-is-the-base-tip`). The lookup
  `pulls/{base}/{head}` takes a percent-encoded slash
  (`pull-for-accepts-encoded-slash`) and names the oldest of several
  pull requests, a closed one before a newer open one
  (`pull-for-names-the-newest-of-several`, *fails*). Opening one whose
  head adds nothing to its base answers `201` and opens it
  (`open-pull-no-changes-is-422`, *fails*); opening one for a head and
  base that have one open answers `409` (`open-pull-exists-is-409`).
- **Merges and updates.** A merge answers `200` with no body
  (`merge-answers-200-with-no-body`); one naming an old head `409`,
  `head out of date` (`merge-old-head-is-409-head-out-of-date`); one
  refused for a closed pull request, a conflict, a protection or a style
  `405` (`merge-closed-is-405`, `merge-conflict-is-405`,
  `merge-protected-is-405`, `merge-style-is-405`). An update refused for
  a conflict answers `409`, `merge failed because of conflict`
  (`update-conflict-is-409-merge-failed`).
- **Branches.** `branches/{branch}` takes a percent-encoded slash
  (`branch-path-takes-encoded-slash`). A branch is created at a full
  commit id (`create-branch-at-full-commit-id`), and creating one that
  exists answers `409` (`create-branch-exists-is-409`). Deleting a
  protected branch answers `403` (`delete-protected-branch-is-403`), and
  deleting one that is not there `500`, with an empty body
  (`delete-missing-branch-is-404`, *fails*).
- **Protection.** A write collaborator who is not an admin is refused
  protection reads, `403` (`writer-protection-reads-are-403`), and reads
  a branch's `effective_branch_protection_name` empty
  (`effective-protection-name-for-a-writer`, *fails*), which names the
  rule for an admin (`effective-protection-name-for-an-admin`): so an
  owner that is not an admin records protection as unknown, never
  absent.
- **CI.** A combined status with no status counts 0
  (`combined-status-without-status-counts-0`), and `skipped` with
  `success` combines as passed (`skipped-and-success-combine-as-passed`).
  Actions report each job as a status on its head
  (`actions-report-each-job-as-a-status-on-its-head`), linking
  `<…>/actions/runs/<run number>/jobs/<job index>` and naming no attempt
  (`actions-status-link-is-runs-n-jobs-i`,
  `actions-status-link-names-no-attempt`); a run's number is its
  `index_in_repo` (`run-number-is-index-in-repo`), and its jobs come in
  index order (`run-jobs-come-in-index-order`).
- **Jobs and their logs.** A job's steps are numbered from 0, `Set up
  job` first and `Complete job` last
  (`job-steps-number-from-0-set-up-first-complete-last`); a failed
  workflow step is `failure`, and the completion step with it
  (`failed-workflow-step-and-completion-step-are-failure`). A step's log
  holds that step's lines alone (`step-log-holds-that-steps-lines-alone`),
  and a step past the last answers `404` (`step-past-the-last-is-404`).
  A range answers `206`, over a job's log and over a raw file
  (`job-log-range-answers-206`, `raw-range-answers-206`); a suffix range
  over a step's log, its last bytes, with a `Content-Range` naming where
  they start (`step-log-suffix-range-answers-206-with-its-last-bytes`,
  `step-log-suffix-content-range-names-its-first-byte`); over an empty
  log, nothing (`empty-log-suffix-range-answers-206-empty`).
- **Comparisons.** `files=false` lists no commit's files and leaves the
  comparison's own `files` empty, and keeps `total_commits` and the
  commits' ids (`compare-files-false-lists-no-commits-files`,
  `compare-files-false-keeps-its-own-files-empty`,
  `compare-files-false-keeps-total-commits-and-shas`);
  `verification=false` changes nothing the calls keep
  (`compare-verification-false-changes-nothing-kept`).
- **Access and errors.** A repository the token cannot read answers
  `404`, by path and by id (`unreadable-repository-by-path-is-404`,
  `unreadable-repository-by-id-is-404`); a moved repository's old path
  answers a redirect (`moved-repository-is-a-redirect`); a write to an
  archived repository `423` (`archived-write-is-423`); an empty title
  `422` (`empty-title-is-422`). No rate-limit headers are sent
  (`no-rate-limit-headers`).
- **Deliveries.** Each row of section 7's table arrives under its
  `X-Forgejo-Event` with the fields its hint reads
  (`delivery-issues-row`, `delivery-pull-requests-row`,
  `delivery-comments-row`, `delivery-reviews-row`, `delivery-push-row`,
  `delivery-create-delete-row`, `delivery-job-ends-row`,
  `delivery-cancelled-job-end-is-delivered`), with its repository's id
  (`delivery-carries-the-repository-id`); a hook's events read back as
  made (`delivery-subscribed-events-read-back`); the signature is
  HMAC-SHA256 in lowercase hexadecimal
  (`delivery-signature-is-hmac-sha256-hex`);
  refs are full, and a tag's told from a branch's
  (`delivery-refs-are-full-and-tags-are-told-apart`); a branch's
  deletion is delivered as a push too
  (`delivery-branch-deletion-sends-no-push`, *fails*). Forgejo delivers
  the actor's own writes (`delivery-echoes-the-actors-own-writes`), so
  the owner drops their echoes, by key and by pusher (section 7); it
  refuses to deliver to loopback by default, and delivers there when its
  settings name the address
  (`delivery-loopback-is-refused-by-default`,
  `delivery-loopback-is-allowed-when-named`); a failed delivery is not
  sent again (`delivery-failed-delivery-is-not-sent-again`).
- **What it settles after it delivers.** A pull request reads
  `mergeable` false while Forgejo checks it
  (`delivery-pull-reads-not-mergeable-while-checked`). A pull request is
  still being checked when the push that moved its head or base is
  delivered (`delivery-pull-is-checking-when-its-push-is-delivered`;
  `delivery-pull-is-settled-when-its-push-is-delivered`, *fails*), and a
  job's commit status is written before its end is delivered
  (`delivery-job-end-follows-its-commit-status`;
  `delivery-job-end-precedes-its-commit-status`, *fails*). The client
  relies on neither order and runs no window for them: it checks only
  that `settle` is above 0 (forge-client.md, section 4), and the rereads
  that wait on a settling are the owner's fresh reads (forge-client.md,
  section 5).
- **What no run records yet,** read from Forgejo's API description and
  sources alone, so no fact names it:
  - that an issue's comments and a review's comments take no page and
    come whole however many there are (5.2, `Item` and `Remarks`);
  - that a job no runner took has no steps, and that its log, and a log
    Forgejo no longer keeps, answer `404` (5.2, "A failed job's log").
- **The setup's, and the owner's git.** What reconcile relies on
  (Forgejo's command line, its admin routes, orgs, repositories'
  collaborators and hooks, the runner) is forge-setup.md, section 8's.
  What an owner's git relies on of Forgejo (a renamed repository's
  redirect, a protected branch's refusal of a push) is the owner's own,
  recorded in its own transcripts.

## 10. GitHub

GitHub's dialect is `skein_forge::github`, a module of `skein-forge`
beside `forgejo`, at the pin `X-GitHub-Api-Version: 2026-03-10`.
forge-github.md holds its documents, plans, failures, deliveries, the
App's documents and the facts it relies on; fake-forge.md, sections 10
and 11, its fake layer and its conformance. Here is what it adds to the
kit.

- **A provider is documents plus call plans.** Nothing above the
  Forgejo dialect and its plans assumes Forgejo. GitHub's calls are REST
  at the pin and three GraphQL documents (the viewer, the queue's
  enqueue, a pull's merge commit), each call's cost counted by the
  component as Forgejo's is (4.2).
- **The vocabulary gains:**
  - validators on reads and their not-modified answer, free of the rate
    (3.3);
  - the rate's window, `Rate`, which the budget takes in place of its
    configured one (3.3);
  - pauses, as `RateLimited { after }` (3.4);
  - `Queued`, a merge put in a merge queue (3.2, 3.3), and `Pull`'s
    `queued`;
  - the protection's queue, signatures, linear history, code owners and
    `Bypass` (3.6);
  - `NotConnected` and `NotInstalled`, the component's (3.4);
  - the App's credential, `Credential::App` (section 8).

  Each addition is optional for a dialect: Forgejo's carries no rate and
  no validator, answers no queue and `Bypass::Never`, and never
  `Queued`. Reads batched into one query are not added.
- **Credentials:** the App (section 8); git over HTTPS with an
  installation's token narrowed to contents.
- **Deliveries:** `X-Hub-Signature-256`, `X-GitHub-Event` and
  `X-GitHub-Delivery`, one URL for the App, each naming its
  installation. A pushed delivery and one read from the App's delivery
  log go through one decoder (forge-github.md, section 9).
- **CI:** check runs and Actions' jobs, whose logs come as a redirect to
  a download host the component reaches (forge-github.md, section 10).
- **Setup:** the App manifest flow (forge-setup.md). There is no owned
  mode.
- **The fake:** a GitHub protocol layer over the same domain
  (fake-forge.md, section 10).

## 11. Verification

The dialect's step tests, in `skein-forge`, with no io:

- **Documents.** Each decoder and encoder is tested:
  - against the conformance's transcripts;
  - against answers generated from the seed;
  - against mutations of both.
- **Encoders.** Golden bytes for each encoder; bounds at the limit and
  one past.
- **Decoders.** A fuzz target per decoder; the bounds at the limit and
  one past (`string_bytes`, `body_bytes`, `inner`, the stack's depth); a
  string cut at every byte; the server's encoders met by the client's
  decoders, and the client's encoders by the server's decoders.
- **Plans.** Each plan against canned answers, for every call of 5.2:
  - the page size, the smallest of three (5.1);
  - names percent-encoded, a slash in a branch's name `%2F`;
  - times read and written, 2554 included;
  - each failure of 5.4, in the status and the message that tell it;
  - the first failure ending a call;
  - a request that was never seen and one that may have been, for a
    write and for `OpenPull`'s reads;
  - the tables filled from the answers heard, and a row dropped.
- **The marker.** Encoded and decoded with several words, a word of 1
  and of 16 bytes, a body whose marker carries another word, an odd
  count of digits, an upper-case digit, a missing line feed, and a key
  at the limit of `op_bytes`.
- **Deliveries.** Each event of section 7 decoded to its hint; a
  signature right, wrong and missing; a branch's deletion; a tag; a job
  that ends; a repository id past 32 bits.
- **Transcripts** are format-faithful and content-irrelevant: volatile
  fields (times, ids, hashes) are masked, and no credential or secret
  reaches a file (fake-forge.md, section 7).

The component's machine world, the protocol world, the client's world,
the setup's world and the conformance are in forge-connection.md,
forge-client.md, forge-setup.md and fake-forge.md. The worlds are under
`tests/forge/`, one crate each (`machine`, `protocol`, `client`,
`setup`); the conformance, `tests/forge/conformance`, is a tool outside
the suites (testing.md, section 6).

## 12. Open questions

- **Long threads, read whole.** Each `Item` read of a participating
  object streams its whole thread, since Forgejo does not page comments.
  Its timeline is paged (`issues/{n}/timeline?since&page&limit`) but
  mixes every event with comments and pages by offset, which a deletion
  shifts; or the component could send `since`, the creation time of
  comment `after`, if the client named it. Default: the whole thread,
  its cost in time, not memory.
- **Dismissed reviews:** whether one counts as no verdict, or as the
  comment it was. Default: the comment it was.
- **Contents on demand.** A pull request's files and a comparison cost
  two reads per file, since `File` carries contents; an overlap needs
  paths only. Default: contents always read, as the client asks; a read
  of paths alone would be the client's to name.
- **A file too large for the answer** fails its call as `TooLarge`.
  Default: so; a side cut with a mark would need `File` to carry one.
- **Bodies past `body_bytes`** are carried empty. Default: so, the
  component counting them; a skein-json read that keeps a string's first
  bytes and digests the rest would let it keep their heads and keys.
- **An edit within a creation's second** reads as `Original`, so a
  person who edits the owner's keyed comment within that second is not
  heard. Default: `Original` when the times are equal.
- **The merge style:** the repository's `default_merge_style`. A style
  an owner names per repository would need `Write::Merge` to carry it.
  Default: the repository's.
- **An expired or revoked credential** reads as `Forbidden` on every
  call until the next start, which it stops (forge-connection.md,
  sections 3 and 5). Default: so; the owner could tell it apart, so a
  person is told once.
- **GitHub's further gaps.** Settled: its dialect is a module of
  `skein-forge`, and section 10 lists what it adds. Open:
  forge-github.md, section 15.
