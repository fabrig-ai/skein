# The fake forge

Provisional, 2026-10-20. The kit's fakes of a forge: a domain that plays a
forge's state and git, a protocol layer that serves Forgejo's API over it,
GitHub's layer that serves github.com's (section 10),
the forge peer that hosts it on loopback io, the throwaway Forgejo that runs
the real one, and the conformance that holds the dialect to what it does.
The dialect is forge.md's; the parts under test are the kit's others.

## 1. In one page

- **The domain** (`crates/skein-fake-forge-domain`, on skein-lib alone) is
  a step crate with a vocabulary of its own. It is Forgejo-shaped, and no
  better (section 2).
- **Its git** lives in the same store, so a branch pushed is the head the API
  reads. It is reached through a typed transport: a shared handle within a
  process, and the protocol layer's private git route from another one.
  Git's own protocol is never served (section 3).
- **The protocol layer** (`crates/skein-fake-forge-protocol`) serves
  Forgejo's API over the domain on skein's HTTP server, with its quirks, the
  setup's routes, signed deliveries and byte faults (section 4).
- **The forge peer** (in `crates/skein-fake-peers`, beside the LLM and OAuth
  peers) hosts the protocol layer on loopback, in plaintext, over a forge its
  owner hands it and may share with another fake in the process (section 5).
- **The throwaway Forgejo** (`testing/skein-throwaway-forgejo`) runs the
  pinned binary on loopback, on SQLite in a scratch directory (section 6).
- **The conformance** (`tests/forge/conformance`, outside the suites) drives
  the throwaway and records each fact the dialect relies on as `holds`,
  `fails` or `unchecked`, with masked transcripts (section 7).
- **GitHub's layer** (`testing/skein-fake-forge-github`) serves github.com's
  API at the dialect's pin over the same domain: the App, its installations
  and tokens, the browser's pages as plain redirects, the merge queue,
  rulesets, the delivery log and the job log's blob host. Where it differs
  from github.com is written down as contracts (section 10).
- **GitHub's conformance** (`tests/forge/github`) runs the same cases on
  that layer, in the focused suite, and, opt-in and from the owner alone,
  on github.com with the probe App's fixtures (section 11).
- **No serde.** The throwaway and the conformance use skein's HTTP client,
  driven by `skein-shell`'s `drive` as a one-shot host, and skein-json.
- **The service's own:** its fake of its git component, its worlds, stories,
  cost ceilings and git transcripts, and an owned Forgejo's preparation
  (forge-setup.md, section 9).

## 2. The domain

It knows no HTTP, no JSON and no dialect. Its `Read`, `Write` and `Git` are
its own types, close to forge.md, section 3; the layers above translate.

### 2.1 The boundary

- **In:** `Event::Call { reply_to, user, repository, op }`: a user number,
  never a credential, and an `Op` of `Read`, `Write` or `Git`. **Out:**
  `Request::Reply`, exactly one per call; `Request::Hook`, best effort, with
  no terminal event.
- **Entry points:** `step` takes an event, `fire` the clock, the only way
  time moves. Each emits at most `MAX_OUT` = 1 request; `next_deadline` says
  when to fire.
- **A call is decided as it arrives:** refused for its rate, failed before
  it is made, made and then timed out, or made and answered after a drawn
  latency. One beyond `calls` is answered at once as unavailable; a change
  beyond the `hooks` in flight sends no hook.

### 2.2 What it holds and refuses

- **It holds** repositories with their users' permissions, labels, issues
  and pull requests on one numbering, comments, reviews, CI statuses,
  Actions' runs and jobs, merges, a wiki and webhooks. Twenty-one limits
  bound it: counts of each kind, bytes of a name, title, body and file, the
  page size, and the calls, hooks and observations held. Over one, a call is
  refused as too large (bytes) or full (counts).
- **It refuses what Forgejo refuses:** too little permission, an undefined
  label, an empty title, a stale head on merge, a merge its base's
  protection does not allow. It keeps what Forgejo keeps and no more.
- **Listings are Forgejo's:** items updated at or after a time, at the
  forge's resolution, least recently updated first, paged by number. A page
  is read from the order as it stands when its call arrives, so an item
  that changes between two pages can shift another onto a page already read.
- **An item's updated time** moves for a comment, a review, a label, a
  title or body, a push to its head, closing, reopening and merging, not for
  a status on its head or a comment edited or deleted (`status_updates` and
  `edit_updates` make either move it). A pending review is hidden until
  submitted. Stale approvals are dismissed only where the protection says so
  (`dismiss_stale`). Anyone with write permission may edit or delete
  another's comment.
- **Merges** are squashes at an exact head: one commit on the base's tip,
  three-way from the newest commit head and base share, conflicting where
  both changed a path differently. A repository has one protected branch,
  `Protection { branch, contexts, approvals, dismiss_stale }`: a merge needs
  the statuses and approvals asked; pushes, creation and deletion are
  refused. Deleting a branch closes the pull requests it headed or based.

### 2.3 Timing, faults and CI

Chances are per mille, in `Config`, and drawn from the domain's seed.

- **Latency:** `latency_min..=latency_max`, or with the `late` chance
  `late_min..=late_max`. `unavailable` fails a call having done nothing;
  `timeouts` makes it, then fails it; `landing` fails a write or a git call
  as timed out and makes it `land_min..=land_max` after the answer.
- **Rate:** a user may make `rate_limit` calls in a window of `rate_window`,
  which starts with their first call after the last one ended; zero is no
  limit. A call beyond is `limited`; one that finds every window kept
  running is `crowded`.
- **Clock:** the forge keeps its own, at `resolution` seconds, skewed from
  the world's by `Skew::{None, Ahead, Behind}`; every time it shows is on it.
  Hooks come after `hook_min..=hook_max`, late with the `hooks_late` chance
  (`late_min..=late_max`), never with `hooks_lost`.
- **A commit that becomes a head** gets a pending status per context of its
  repository (`Checks { contexts, latency_min, latency_max, silent, passes,
  reruns, cue }`), then a verdict after a drawn latency: passed with the
  `passes` chance, failed otherwise, never reported with the `silent`
  chance. CI runs once per commit and repository. `Cue::File { path, green }`
  replaces the chance: a commit whose file at `path` holds `green` passes,
  any other fails. `Cue::Message { fail }` instead fails a commit whose
  message contains `fail`, and passes any other commit. With no cue the
  configured chance decides; neither cue changes latency, silence or reruns.
- **A rerun** happens once per context, with the `reruns` chance: pending
  again, then a new verdict, so a head's CI can move after it settled.
  People with write permission report statuses of their own.
- **Actions' identities** follow the statuses kept: one `Run` per commit and
  repository, numbered from 1, and one `Job` per context, at attempt 1 or,
  after a rerun, 2. A commit whose statuses are forgotten loses its run.

### 2.4 Seen from outside

- **Every change is observed,** content and all, and so is every write the
  forge refused, in a bounded queue a referee drains (`pop_observation`). A
  full queue drops and counts (`observations_lost`).
- **Written out, each is a JSON line,** for a referee in another process,
  such as temper's fake forge's `--observations` file (temper's
  `protocol/setup.md`, 8.4). The protocol layer's `observations` holds the
  encoding, since the domain has no JSON: `encode` writes a `Recorded`, an
  observation or a checkpoint, as one object and a newline, and
  `decode` reads one back exactly. For example:

      {"observation":"moved","repository":"temper/widget","branch":"landing","from":3,"to":4,"by":2,"forward":true}
      {"observations_lost":0}

  - **Lines come in drain order,** the order `pop_observation` gave them,
    which is the order the forge made them.
  - **The object's first member** is `observation`, the variant's name in
    lower case, as `moved` or `wiki`, and the rest are its fields by their
    names, in their order.
  - **A moved line also says `forward`,** which the writer computes as it
    drains, from `Domain::is_ancestor`: whether `to` descends from `from`,
    or, for a branch made again (`from` null), from the `at` of that
    branch's last deleted line; `null` if there is neither. The writer
    keeps each deleted branch's `at` until the branch is made again, as
    many as the domain's `Limits::branches` per repository, forgetting the
    oldest past it (a branch made again whose deletion it forgot has
    `null`). It is the only member not in the observation itself, since a
    reader in another process has no domain to ask.
  - **A checkpoint,** `{"observations_lost":N}`, is the count lost so
    far, 0 included: cumulative, one at each flush. A writer ends each
    flush with one, and flushes in each pass that observed or lost
    anything and once at its stop: a file is whole up to its last
    checkpoint, and what follows it is a flush still being written. A
    stopped writer's last line is a checkpoint, so a stream that does not
    end with one is truncated, and a judge reading it rules Undecided.
    temper's writer sends nothing of a pass before its flush is written
    (temper's `protocol/setup.md`, 8.4), so a reader that has seen a
    reply, or what a reply caused, finds what it observed.
  - **A number,** a commit, a user, an item, an id, a revision or a
    position, is a JSON integer, exact to `u64`. A flag, a group's
    `ended` or `landed`, is `true` or `false`. An absent value is
    `null`, a list is an array, and an opened pull request's `branches`
    an object of its fields.
  - **A name,** a repository's, a branch's, a label's, a context's or a
    page's, is a string. Content, a title, a body or a page's content, is
    base64, so its bytes cross whole, as on the git route (section 4). A
    name that is not UTF-8 has no line: `encode` answers none, and the
    writer counts it with the lost.
  - **An enum** is its variant's name in snake case: a `kind`, a
    `verdict`, a `state`, and a refusal's `what`, its operation
    (`create_issue` to `create`). A refusal's `error` is an object: its
    `name`, the error's in snake case, then its fields: `reset`, in
    nanoseconds as the domain's `Time` holds it, for `rate_limited`; `what` for
    `missing`; and `number`, `head` and `position` for `already_queued`.
  - **A reader** takes the members in any order and refuses an unknown,
    a repeated or a missing one, naming it. So a reader older than its
    writer fails by name rather than misreading.
- **A world acts from outside the API** with free functions (grants,
  commits, branch moves, CI outcomes, reachability) and, at settle, reads the
  store (`inspect`), the `tally` of what the forge did and refused, and the
  `room` left, so a forge that filled up is told from one that works. Needing
  room, it forgets the statuses of the oldest commit no head shows, never a
  commit.

### 2.5 The promises' judge

The domain's `promises` module judges a client's writes over the decoded
stream of section 2.4. Its `Profile` supplies the client's user id, key
markers, owned branch prefixes, allowances on foreign items and table
bounds. `Recorded` supplies observations with the encoder's `forward`
answer and cumulative loss checkpoints. `Decision` supplies the exact
repository, pull number and head the client considered; `decided()` says
all decisions have arrived, even when there are none.

- **Once:** two client creations with the same marked key, per repository
  and kind (issue, pull or comment). Only the first opening marker and its
  next closing marker count.
- **Forced:** a client move that does not descend, including a remake
  after deletion, or a rejected push. An existing branch moved without a
  `forward` answer leaves the judgement undecided.
- **Foreign:** writes to branches, items or comments the client did not
  make in the stream, except owned branch prefixes and the profile's
  explicit item allowances. A body edit over another user's last edit
  breaches this promise even on an owned object. Label allowances apply
  to added and removed labels against the last set seen; an unseen
  item's first `Labelled` is a baseline and judges nothing. The forge's
  merge base moves and queue group branch creations and deletions are
  recognized by their observation order and commit, and exempted.
- **Order:** a client merge or enqueue must have a decision at that exact
  head. A queue merge at the enqueued head shares its obligation.
  Decisions can arrive before or after the observation.

`Defined`, `Reported`, `Wiki` and `Refused` are judged by no promise.
Groups identify the queue's writes; dequeues introduce no obligation.

The judge allocates repository slots and each repository's item, comment,
branch, key, label and evidence tables at `bounds.objects`, and the
shared decision table at `bounds.decisions`. Last editors live with
items and comments; labels, including removed labels, are remembered per
item. Any table overflow is sticky and leaves the judgement undecided.

`verdict()` yields `Pass`, `Fail` with breaches, or `Undecided` with its
reason and the breaches still visible. Reasons take precedence in this
order: the latest lost count when nonzero, no final checkpoint, a full table,
a merge or enqueue before `decided()`, and a move lacking `forward`.
Breaches are deduplicated by promise and subject, in first-seen order.
Their `line()` is `promise/<promise>/<subject>`: names and keys are
percent-encoded byte by byte, numbers decimal. The judge does no parsing,
printing or network work.

## 3. Its git, and the remote boundary

What a fake of a service's git component may rely on. That fake is the
service's own (temper's is the first); the domain is the remote it acts
against.

- **One store.** Commits are named by a count the store keeps, from 1, so a
  seed replays the names. A commit is `Object { parent, merge_parent, tree,
  message }`: a first parent, an optional second, a tree of paths to
  contents, and a message of opaque bytes, at most `title_bytes + body_bytes
  + 2`. Squash merges and `advance` carry an empty message.
- **The calls** are `Op::Git`. `Clone` (every branch, and the default) and
  `Fetch { Branch | Commit | Default }` (the commit and those before it, by
  both parents) need read permission. `Push { branch, commit, expected }`
  and `Create { branch, commit }` need write; a protected branch takes
  neither.
- **A push** moves a branch by a fast-forward, or creates it; any other move
  is `Rejected`. A given `expected` must be the branch's head, or the push
  is rejected. A creation answers `Created` or `Exists`.
- **Every move of a branch** is observed, starts CI on the new head, and
  moves the head of each open pull request that follows it. `advance` is
  the world's other party moving a branch under a run.
- **The scenario's commits:** `commit` returns `None` for an unchanged tree;
  `merge_commit` and `commit_tree` store one anyway. A parent's id is less
  than its child's, as `git::Remote` asks (fake-checkout.md, section 4).
- **Faults, time and identity.** A git call meets section 2.3's latency, rate
  and faults; a fake that needs an instant answer sets the latencies to
  zero. The caller is a user number, mapped from its credential by the
  protocol layer's tables (section 4), so a token for the API and one for git
  stand for the same user.
- **Within a process,** a working tree reaches the store through
  `git::Remote`, answered on the shared forge (section 5) without the
  network.
- **From another process,** the same calls travel the protocol layer's
  private git route (section 4) on the forge peer's listener, with the
  store's reads and commits beside them. So a fake of the service's git
  can run as a program of its own, as temper's `temper fake git` does
  beside a demo's forge. The domain stays the only store: such a program
  holds nothing the store has not answered, and keeps no object past its
  command.
- **No git over HTTP.** Git's own protocol, its smart HTTP and its packs,
  is never served; commands, arguments and their outputs are the
  service's. A bridge for real git is designed separately, and lands
  here.

## 4. The protocol layer

It depends on skein-lib, skein-http, skein-json, `skein-forge` and the
domain, and holds no clock of its own.

- **A forge it is handed.** It serves a fake forge its owner hands it and
  builds none (section 5). The forge peer hosts it.
- **Types.** `Server` is one HTTP connection (`MAX_UP` = 3, `MAX_DOWN` =
  18). `Service::new(config, &limits, seed)` owns routing, the tally and the
  setup store; `Service::target` hands a domain reply to its connection, or
  returns it whole when that connection has gone. Its `Limits` sit beside
  the domain's, and `new` refuses limits whose `worst_case` is none.
- **The scenario's tables** (`Config`): the hooks' `Endpoint { forge, host,
  secret }`, if any, `host` being the loopback `ip:port` the forge peer
  connects to and sends as `Host` (section 5), and with none the setup's
  hooks route them (below); `base`, an `http://` origin; `tokens`,
  secrets to user numbers; `users`, numbers to logins; each repository's
  `id`, full name and protected branch; `max_response_items`,
  `default_paging_num` and `faults`.
- **Routes.** Outside the setup's, each route is one domain call: decoded by
  the server side of `skein-forge` (forge.md, section 5.2), translated to a
  `Read` or `Write`, and its answer encoded as Forgejo encodes it. A write
  Forgejo answers with what it made or changed (an issue, a pull request,
  a comment, a review, an edited issue, a status, a branch) makes one call
  more, the domain's read of that, on the same request. A commit is a
  40-digit SHA-1 id: SplitMix64's finalizer of the domain's count, then the
  count, each eight bytes big-endian, then four zero bytes; any other id
  names none.
- **Credentials.** The header is `Authorization: token <value>`; an unknown
  token is `401`. Tokens stay in this layer: no domain call, observation or
  tally holds one. The tally counts requests at the entrance: total, by route
  (up to 74), by user, unrouted and unauthorized.
- **Forgejo's quirks are kept,** so a client that leans on Forgejo being
  better than it is fails here first: the default order is newest created
  first, and `sort=leastupdate` gives least recently updated first; a page is
  capped at `max_response_items`; a size without a page is ignored; an
  issue's comments, a review's comments and a comparison come unpaged;
  collaborators come without their permissions; a merge answers with no body;
  `pulls/{base}/{head}` names the oldest of several; a pull request whose
  head adds nothing is opened (`201`); an issue's edit, close or reopen
  answers `201`, as a reviewer's request does, and a reviewer's removal and
  a branch's deletion answer `204` with no body; deleting a branch that is
  not there answers `500`; a failed job's completion step reports
  `failure`; a push to a pull request's head or base starts a mergeability
  check. Forge.md, section 9 names the fact that records each, or lists
  those not yet. A reviewer's request answers with no body, where Forgejo
  sends the pull's reviews, which no client reads.
- **Faults move down to bytes.** A call refused for its rate is `429` with
  `Retry-After` in whole seconds, rounded up; one failed unavailable is `503`;
  one made and then timed out is `500`. The layer adds per mille byte
  faults: `reset` closes the connection before the next request is read;
  `drop` closes it, and `server_error` answers `500`, after the domain
  answered, so the call is made and its answer lost.
- **Deliveries.** `Service::delivery(&hook, &limits)` renders a domain hook as
  a `Post { head, body }`: a `POST` to `/hooks/<number>` with the event header
  and an HMAC-SHA256 signature of the body in lowercase hexadecimal, by the
  endpoint's secret (forge.md, section 7). A status the fake's CI made is
  delivered as its job's end; a status anyone else reports sends nothing, as
  on Forgejo. Of forge.md's table (section 7) it renders `issues`,
  `pull_request`, `issue_comment`, `pull_request_approved` for every review
  whatever its verdict (the hint is the same), `push` of a branch, `delete` of
  a branch, and `workflow_job_success` and `workflow_job_failure`; never
  `pull_request_rejected`, `pull_request_comment`, `create`, a tag's events,
  or a job cancelled or skipped.
- **Deliveries follow the setup's hooks** when the scenario names no
  endpoint, as Forgejo's follow the hooks its API made, such as the one
  an application's reconcile makes on each repository it adopts
  (forge-setup.md, section 4.5). `Service::deliveries(&hook, &limits)`
  renders a domain hook once for each hook the setup store keeps on its
  repository that takes it, in id order, each with the loopback address
  to post it to: its target is the hook's URL's path, its `Host` the
  URL's authority, its signature by the hook's own secret, and its
  address that authority's.
  - A hook takes a delivery when its events include any event subscribed
    of the delivery's row in forge.md's table (section 7). The fake does
    not tell an issue's assignment from its edit, so a hook subscribed to
    `issue_assign` alone takes every `issues` delivery: a stated
    over-approximation, which a client that subscribes as forge.md does
    never meets.
  - A hook takes none that is inactive, whose secret is empty, or whose
    URL is not `http://` with a loopback IP literal, a port and a path.
    So a fake never posts beyond its machine, nor unsigned. An
    organization's hook takes none (section 9).
  - A configured endpoint wins: with one, `deliveries` yields the
    endpoint's alone, and `Service::delivery` stays, the endpoint's
    rendering, for the worlds that pin an address.
- **The git route** is private, for a fake of a service's git that runs
  in another process (section 3); Forgejo has none like it. A request is
  `POST /.fake/git/<call>` with a JSON body, and its answer is JSON, one
  call each. A first segment that begins with a dot names no Forgejo
  account, so the route shadows none of Forgejo's. A commit is its count,
  a JSON number; a branch or repository name is a string; paths, contents
  and messages are base64, so bytes cross whole. The tally counts it
  route by route, and the byte faults reach it as they reach every route.
- **The repository's calls** are `clone`, `fetch`, `create` and `push`,
  section 3's `Op::Git` one for one, and `branch`, the domain's read of a
  branch's head, which a fake asks to word its output as git does. Each
  names a repository by its full name and is one domain call, with
  section 2.3's latency, rate and faults. Its credential is
  `Authorization: Basic` of a login and a token, as git sends what its
  remote URL or `http.extraHeader` holds: `tokens` maps the token to a
  user number, and `users` must give that number the login. A missing or
  unknown credential is `401`. A domain error is a status and a JSON body
  naming it: `403` forbidden, protected or refused, so the caller knows
  it was known; `404` missing, naming the repository, branch or commit;
  `502` unreachable; and `429`, `503` and `500` as above. A push answers
  pushed or rejected, a creation created or exists, both `200`.
- **The store's calls** reach git's object database, which is the
  domain's: `objects` reads commits, `commit`, `merge` and `commit-tree`
  store them as section 3's scenario functions do, and `ancestor` asks
  `is_ancestor`. They are no domain call, so they meet no latency, rate
  or domain fault, and they take no credential, as a local git command
  has none. That is the stated limit of a fake: anyone on loopback can
  read every object by its count and store new ones, though never move a
  branch. A store with no room is `507`, a tree past the domain's limits
  `413`. `new` refuses layer limits whose request or answer cannot hold
  the largest tree the domain allows.
- **`objects`** answers each commit it is asked whole (parents, tree and
  message), and the graph alone (the parents) of the commits before them
  by both parents, newest first, bounded by `max_response_items` and the
  answer's room. An object never changes, so a caller keeps what it has
  been given for the rest of its command.
- **The wire** uses standard, padded base64 for paths, contents and
  messages. Repository names are full names; commits are JSON integers.
  Members may arrive in any order.
  - `clone`: `{"repository"}` answers `{"default", "branches":
    [{"branch", "commit"}]}`.
  - `fetch`: `{"repository"}` with at most one of `"branch"` or `"commit"`
    answers `{"commit"}`; neither asks for the default branch.
  - `create`: `{"repository", "branch", "commit"}` answers
    `{"result":"created"|"exists"}`.
  - `push`: `{"repository", "branch", "commit"}` with optional `"expected"`
    answers `{"result":"pushed"|"rejected"}`.
  - `branch`: `{"repository", "branch"}` answers `{"commit"}`.
  - `objects`: `{"commits":[..]}` answers `{"objects":[{"commit", "parent",
    "merge_parent", "tree":[{"path", "content"}], "message"}],
    "graph":[{"commit", "parent", "merge_parent"}]}`. An absent parent is
    `null`.
  - `commit`: `{"parent", "tree", "message"}`; `merge`: `{"parent",
    "merge_parent", "tree", "message"}`; `commit-tree`: `{"parent", "tree",
    "message"}` with optional `"merging"`. Each answers `{"commit":N|null}`;
    trees carry `[{"path", "content"}]` as above.
  - `ancestor`: `{"ancestor", "commit"}` answers `{"ancestor":true|false}`.
  - An error answers `{"error": NAME}`, NAME the domain error in snake
    case (`missing`, `rate_limited`, `already_queued`, ...), with the
    members that error carries: `"what"` for `missing` (`"repository"`,
    `"branch"`, `"commit"`, ...), `"reset"` in nanoseconds for
    `rate_limited`, and `"number"`, `"head"` and `"position"` for
    `already_queued`, so every domain error crosses whole. A refusal the
    layer makes before the call is decoded (an unknown call `404`, a wrong
    method `405`, a missing or unknown credential `401`, one that may not
    write `403`) has no body, and the client's decoder says whether an
    error was named, so a caller tells those, and a host with no route,
    from a domain error.
  - An `objects` call asking for more commits than `max_response_items`
    is `422`, before the store is read.
  - A body other than one object, an unknown, repeated or missing member,
    a wrong type, invalid base64, or both `"branch"` and `"commit"` on a
    fetch is malformed, `422`.
- **The setup's routes** (accounts, tokens, organizations, a repository's
  read by id, collaborators and hooks, and the admin's repository search,
  in id order and paged by number as Forgejo pages it) are served from this
  layer's own bounded store, not the domain's (forge-setup.md, section 4).
  A repository read by id answers the domain's settings when the host
  lends the domain.
- **A first repository is made in the domain** (forge-setup.md, 4.9).
  The setup's `CreateRepository` in an org the setup store keeps makes
  a domain repository owned by that org, private, whose `main` is one
  commit holding `README.md`, `# NAME`; a name the domain has already is
  `409`. `CreateFile` on a domain repository commits the file on its
  branch, its author and committer the request's, and answers its
  commit; a path already there is `422`. Its git serves both commits,
  and its branch, raw and repository reads find them, as they find a
  seeded repository's.
- **A setup grant reaches the domain.** A collaborator grant made through
  the setup routes is kept in the setup store, and the domain grants the
  account's user number (section 5) the same permission on the
  repository, as Forgejo's collaborator has it. So the bot that adoption
  grants reads and writes through the domain's routes, as an
  application's demo needs (temper's `protocol/setup.md`, 8.4). Only the
  grant crosses: the store's other rows stay its own.

## 5. The forge peer

`skein_fake_peers::forge::Peer` hosts the protocol layer as the LLM and OAuth
peers host theirs. The real loop hosts the same one.

- **Its shape is the other peers'.** `new(address, limits, ..)` listens on a
  loopback address, port 0 for the kernel's, and refuses a non-loopback
  address or unusable limits (`Error::Limits`) before any effect.
  `address()` is the bound address, known after io's listening event.
  `observations()` holds frozen records under the world's caps. `shutdown()`
  closes admission, then each connection once its client has settled. It
  implements `Host` (shell.md, section 12), and a checked `worst_case` sums
  the face, the protocol layer, the shared forge, the connections and the
  observations. By default it records connections, request heads, calls
  with their terminals, and deliveries with their outcomes: two records
  for each connection (its opening and its close), two for each call (its
  head and its terminal) and one for each delivery. Nothing drains them, and none
  is dropped: a record past the world's caps fails the peer's assertion,
  "frozen record count fits" or "frozen payload admitted before copying",
  and with it the story. So a world's caps are a stated room for a whole
  story, each named for what it holds, with its reason, and a story that
  outgrows it fails by name rather than losing its history.
- **A peer that serves a person keeps no records.** An application that
  runs the fake as a product, such as a demo's forge (temper's
  `protocol/setup.md`, 8.4), lives longer than any story's caps.
  `without_records()`, called on a fresh peer before its first pass,
  makes it keep none: `observations()` stays empty, `worst_case` leaves
  the observations out, and the assertion never runs. A cap of zero
  still means a story that records nothing, and fails at its first
  record. The LLM and OAuth peers have the same mode (fake-llm.md,
  section 3).
- **A story keeps what it reads.** `keeping(kept)`, on a fresh peer before
  its first pass, names the kinds it keeps (connections, heads, calls,
  deliveries); the others are never copied. The caps still bound what is
  kept and fail by name past it. A world whose referee reads only
  deliveries keeps those and the connections, so its records grow with
  the story's events, not with its clients' polls over wall time.
- **A Forgejo peer may serve TLS.** `tls()`, called on a fresh peer before
  its first pass, serves the fixed test chain (`ROOT`, for `skein.test` and
  `127.0.0.1`) with `https://` origins. Hook deliveries stay plaintext to loopback.
- **The owner may seed the setup's accounts.** `setup_mut()` lends the
  protocol layer's setup store (section 4), as `Service::setup_mut`
  does, so that an owner makes, before the peer serves, the site
  administrator and its token that an application's setup takes over as
  an owned forge's (temper's `protocol/setup.md`, 6.5). A setup
  account's number is the user number its tokens act as in the domain,
  so an owner that seeds both keeps the scenario's `users` apart from
  them.
- **Plaintext by default.** Its `base` is `http://`; its face is the shared one
  with `Transport::Plaintext` and a `queue` of at least 64. The peer supplies
  its bound origin to either dialect's service before it accepts requests.
- **It hosts the layer over a forge it is handed.** The owner builds the
  forge once: its domain, its configuration, the time of the last pass and
  the replies not yet routed. It hands the peer a single-threaded shared
  handle, and may hand the same forge to any fake in its process that acts
  on it, such as a fake of a service's git component (section 3). A fake
  in another process reaches it on the peer's listener by the git route
  (section 4), each request borrowing the forge in the peer's pass, by
  rule 1.
- **Rules of sharing:** (1) each entry point borrows the forge for one pass,
  so borrows never overlap; (2) the peer writes the world's time into the
  forge at the start of each pass, and another fake reads it; (3) whoever
  steps the domain leaves a reply or hook it did not ask for on the forge's
  pending list, and the peer routes the list with `Service::target` at the
  end of its pass; (4) each pass the peer fires the domain once, then
  reclaims the domain and the service; `next_deadline` is the earlier of
  io's and the domain's.
- **It writes the deliveries.** A hook the domain releases is rendered
  (section 4) and delivered over loopback, once for each address its
  rendering names: the endpoint's `host`, or each setup hook's URL's
  authority, the address it connects to and the request's `Host` alike,
  as a Forgejo posts to its hook's URL. Each is a delivery of its own,
  with its own outcome and record. An owner whose listener binds port 0
  sets the endpoint in the forge it shares, or makes its hook through the
  setup routes, as an application in another process does, once that
  listener listens, its address known after io's listening event as the
  peer's own is, not at its bind, before a hook it means to have written
  is due. Until then, and for a `host` that is no loopback `ip:port`,
  there is no address; a setup hook that takes none (section 4) is no
  delivery. The deliveries in flight are bounded by the connections, and
  the peer's `worst_case` sizes each by the longest it can be: the
  endpoint's, or a setup hook's whose URL and secret are the longest the
  setup routes decode. Delivery is best effort:
  one that cannot be written (no address, no room, the connection refused
  or reset) is dropped and observed, never resent. A hook due before the
  owner's listener listens is such a drop, and one due once the owner has
  begun to stop may be; neither the peer nor the owner holds a hook back
  for the listener. A world asserts a delivery written only for a hook due inside
  its listener's life: a story set up before any host runs loses the
  setup's hooks, and a story that asserts the hook its own end causes
  ends only once the peer holds a delivery's outcome for each hook the
  story's own changes cause, a count the story states. A hook is scheduled
  at its change and released later, so no count of the hooks the forge
  has released so far (its domain's tally) says what a story is still
  owed: the story, which made the changes, does. Its domain's tally is
  checked against that count at the end.

## 6. The throwaway Forgejo

The throwaway runs the pinned Forgejo for the tools and process tiers that
need a real one. It never meets an existing Forgejo.

- **The binary.** `Forgejo::from_path` takes the path from its caller and
  reads no environment. It runs `--version` and refuses, naming it
  (`Refusal`), a binary that is unreadable or whose version line does not
  name the pin's `VERSION` (forge.md, section 4.3). It computes the SHA-256.
  The pin's `RUNNER_VERSION` and `names_runner` are re-exported. The kit's
  tests that need the binaries are ignored by default; their test files
  read the paths from `SKEIN_FORGEJO` and `SKEIN_FORGEJO_RUNNER`, and fail
  naming the variable when it is unset. No library code reads a variable,
  and a service's tests read their own.
- **The instance.** `start(scratch, &Settings)` writes an `app.ini` for
  loopback, SQLite and the page cap, makes the admin user and its token, and
  waits for the API. The scratch must be empty or absent and not under
  `/tmp`. The environment is cleared, with a home in the scratch, a fixed
  `PATH` and `TZ`, and no system or user git configuration.
  `Settings::small()` pages at 2, enables Actions and bounds an answer at
  16 MiB.
- **Attaching.** `Instance::attach(child, port, scratch, admin_token,
  &Settings)` takes a Forgejo its caller started and keeps, such as a
  service's owned forge. It waits for the API and reads the admin's login
  and id with `GET /api/v1/user`, naming no login of its own. Its `stop`
  ends the child and leaves the scratch, which stays its caller's, and
  `cli` refuses, having no work path.
- **Provisioning:** users, tokens, repositories (`Init::{Readme, Empty,
  Sha256}`), collaborators and `fresh_secret`. `Forgejo::work_path` and
  `Instance::cli` run Forgejo's command line without and with a web
  process, keeping the exit code and both streams.
- **Exchanges.** `exchange` (a token, or none) and `exchange_basic` send one
  request and return what was sent and received, through skein's HTTP client
  driven by `drive` as a one-shot host (shell.md, section 12). A recording
  holds no credential: `token MASKED` or `Basic MASKED`. `secrets()` lists
  every secret the instance made, for a leak scan.
- **`stop`** removes the instance and its scratch; a start that fails cleans
  up. The conformance starts the runner (section 7), not the throwaway.

## 7. The conformance

`tests/forge/conformance` is a tool outside the suites. It drives a throwaway
Forgejo with a page cap of 2, so paging shows, and records what the dialect
relies on (forge.md, section 9). It makes its own admin user and tokens.

- **Inputs:** `--forgejo <path>`, `--scratch` (empty or absent, not under
  `/tmp`), `--out`, and optionally `--runner <path>`, checked with the pin's
  `names_runner`. It reads no variable.
- **Transcripts:** `transcripts/<case>/{request,response}` are raw HTTP/1.1
  bytes. A response is unframed, with chunking removed and `Content-Length`
  recomputed; JSON key order, whitespace and types stay as received. A case
  replaces only its own directory. Keys, nesting, types and status lines
  must match; ids, times, counts and text may vary. `transcripts/binary.txt`
  holds the binary's version line and SHA-256.
- **Deliveries:** `deliveries/<event>/{request,response}`, one per event of
  forge.md, section 7, with a mask map of their own.
- **Facts:** `facts.txt` has provenance lines, then a line per fact (forge.md,
  section 9): name, `holds`, `fails` or `unchecked`, reason.
- **Masking:** one map masks object ids and RFC 3339 times by first
  appearance, keeping width and offset; ports, `Date` and `Last-Modified`
  take fixed values; numeric resource ids stay (provisioning has a fixed
  order).
- **Secrets never reach a file.** Before masking, the tool registers every
  token's value and its eight-digit tail, and every password. It scans each
  response body before masking and every output before writing. A leak stops
  the run naming the case and the kind of secret, never its value.
- **The groups:** the dialect's cases; the setup (accounts, tokens,
  organizations, repositories' collaborators and hooks, the admin's
  repository search, under the least admin scopes); `cli-` facts
  (Forgejo's command line, outcomes only); `runner-` facts
  (registration, the runner's connection, a workflow that clones a
  private repository with the job's token); and the Actions facts, which
  run two inline host-shell jobs, one succeeding and one failing, with
  nothing downloaded. The runner groups are `unchecked` without the
  runner.
- **`--component`** runs the kit's component (forge-connection.md) through
  real io sockets on the shell's ring against a fresh fixture, under the
  scopes `read:user`, `write:issue` and `write:repository`. It replaces only
  the `component-` facts and records no transcripts. Each call's cost is
  compared with the request lines counted from socket sends.

## 8. Coverage and budgets

- **Step tests** sit in each crate. The domain's cover each call's decision,
  listings, merges, protection, CI and hooks. The protocol layer's cover
  routes, errors, faults, the tally, the setup routes and deliveries, each
  decoded by `skein-forge`, with a signature that holds for its body only;
  deliveries that follow the setup's hooks: one for each hook that takes
  it, in id order, each with its own URL's target and `Host` and its own
  secret's signature, the events' filter by row, the hooks that take none
  (inactive, an empty secret, an `https` URL, a name, no port, an address
  off loopback, an organization's), and an endpoint that wins; and the git
  route: each call's statuses, a credential's login, the
  store's calls beside the domain's faults, and `objects` at its bound.
  The observations' lines: every variant, error and checkpoint
  round-trips through `encode` and `decode`, a name that is not UTF-8
  has none, and a line with a member unknown, repeated or missing is
  refused by its name; `forward` for a fast-forward, a move that is not
  one, a branch made again after its deletion, one made for the first
  time, and one whose deletion the writer forgot.
- **Memory:** a forge filled to its limits, and a long merge history walked
  through both parents, stay within `worst_case`, in binaries of their own
  under the counting allocator (testing.md, section 6).
- **Replay and the peer:** a seed replays the names, replies, observations
  and deliveries. The peer's tests run a client over loopback against the
  protocol layer, with deliveries written, late, lost and refused, a
  repository's two setup hooks each written to its own listener, and a
  forge shared with a second fake.
- **The throwaway and the conformance:** tests of masking, facts, `app.ini`
  and refused versions and scratches need no binary and run in the focused
  suite; those that need one are ignored unless the caller names it. The
  tool itself runs outside both suites.
- **Budgets:** the focused suite's 30 seconds (15 seconds before the user's
  doubling of 10-22, provisional) and the fuzzy suite's two minutes (suite's
  minute before the user's doubling of 10-22, provisional) (testing.md,
  section 7) bind these tests.

## 9. Open questions

- **A bridge for real git** needs a protocol it speaks, over a listener of
  the peer's. It is designed separately and lands in section 3. Default:
  none; a fake of a service's git reaches the store through
  `git::Remote`, in its process or by the git route (section 4).
- **The GitHub layer's speculative merges.** A queue's group is built
  from the entry ahead, one at a time (10.2); github.com batches groups.
  Default: one at a time, until a story needs batches.
- **An organization's hooks** are stored and route nothing: reconcile
  makes a repository's (forge-setup.md, section 4.5), which deliveries
  follow (section 4). Default: they stay so until a story needs an
  organization's.
- **The conformance against the fake.** Forgejo's conformance does not run
  against its protocol layer; that layer's step tests hold its quirks.
  GitHub's runs against its layer (section 11). Default: Forgejo's stays
  as it is.
- **Recorded live transcripts.** A live run's masked transcripts would serve
  the dialect's decoder tests. Default: the owner reviews them for leaks
  before committing them; the fake's own transcripts serve until then.
- **The runner's start.** The conformance starts the runner (section 7).
  Whether a second user moves that start into the throwaway is open.
  Default: it stays in the conformance until a second user needs it.

## 10. GitHub's layer

`testing/skein-fake-forge-github` serves github.com's API, at the
dialect's pin (forge-github.md), over the same domain, beside the Forgejo
layer and in its shape (section 4): a `Server` per connection, a
`Service` that owns routing, the tally and the layer's own stores, a
`Config` of the scenario's tables, and `Limits` with a `worst_case`.
Each route is decoded by the server side of `skein_forge::github` and
answered as GitHub answers. GitHub's quirks live here. The domain gains
only what GitHub has and Forgejo lacks (10.2).

Built here are the checks and job reads, signed job-log redirects, the
peer's blob listener and its redirect scenarios, drawn mergeability settle
deadlines, person credentials and installation scopes/revocation, byte faults,
and the service and hosted-peer allocator binaries. The remaining contracts
below are tracked in STATUS.md.

### 10.1 What it serves

- **REST at the pin.** Every route of forge-github.md, sections 4, 8, 9
  and 11, at github.com's paths, with no prefix. A request without
  `X-GitHub-Api-Version: 2026-03-10`, or without `User-Agent`, is
  refused as GitHub refuses it (`400`, `403`). Pages are GitHub's: up to
  `per_page` rows, at most 100, with `Link` next while more remain.
  Issues and pull requests share one numbering, and the issues' listing
  holds pull requests, each with its `pull_request` member. An absent `since`
  lists all rows; otherwise the bound is inclusive (`updated_at >= since`).
  A `since` before `1971-01-01T00:00:00Z` answers `200 []`. The named
  `ISSUES_SINCE_MIN` bound in the GitHub reads layer is the earliest observed
  nonempty bound; the intervening dates were not probed further.
- **GraphQL at `POST /graphql`.** The layer answers the dialect's three
  documents, matched byte for byte against its constants (`viewer`,
  `pull`, `enqueue`), each with its variables. Any other document is
  answered `200` with an `errors` entry. This is a difference from
  github.com, which answers any query.
- **Credentials.** `Authorization: Bearer <t>` names one of three kinds:
  - a person's token (the scenario's `tokens`, as in section 4);
  - an installation's token, which the layer mints;
  - the App's JSON Web Token, which the layer checks by signing the
    token's signing input again with the App's key: RS256 with PKCS#1
    v1.5 is deterministic, so it needs no verifying function. The layer
    keeps the last token it verified, its exact bytes and its claims, so
    the same token again under the same key skips the signing; a new key
    or a revoked one forgets it, and the claims are checked on every
    request. A token past its `exp`, or more than 600 seconds after its `iat`, or not
    yet valid by more than 60 seconds on the forge's clock, is `401`.

  The App's routes on another kind are `401`. `GET /user` on an
  installation's token is `403`. Tokens stay in the layer, as in
  section 4. Person tokens, `GET /user` and the installation-token refusal
  are built. Each person secret names a configured account's domain user;
  repository permissions and write authorship follow that user. An optional
  expiry on each configured person token emits
  `github-authentication-token-expiration` on that token's answers, in
  `YYYY-MM-DD HH:MM:SS UTC` form. No expiry emits no header.
- **The App** is held in the layer's store: id, slug, owner, key,
  webhook secret, permissions, events and the hook's URL. Its bot is a
  domain user, `<slug>[bot]`, who acts on the domain. `GET /app`, `GET`
  and `PATCH /app/hook/config`, and `GET /users/<slug>%5Bbot%5D` read and
  change the store. The bot read needs no credential and answers its
  `login`, `id` and `type: "Bot"`; another slug is `404`. The scenario
  owns the domain bot's stable id, distinct from the App's id; conversion
  skips it when allocating an App id.
- **Installations and their tokens.** An installation is an account and
  either all its repositories or a list of them, and may be suspended.
  A repository's `owner.type` is its account's kind, so one an
  organization owns reads `Organization`, and its pulls take the queue
  (forge-github.md, 4.1 and 5.1).
  The mint answers a token that expires an hour later on the forge's
  clock, full, or contents-only when the body asks. The domain sees the
  bot's user number, and the layer refuses beforehand what the token
  does not reach:
  - a repository outside the installation is `404`;
  - issue creation succeeds even for a contents-only token, with `201`
    and `x-accepted-github-permissions: issues=write`; editing its title
    is refused with `403` "Resource not accessible by integration";
    the mint's narrowed permissions are `contents: write`, `metadata: read`;
    the remaining contents-only route refusals are inferred `403` responses;
  - an expired token is `401`.

  `GET /app/installations` lists them and `GET /app/installations/<id>`
  reads one, each row with its account's type and its creation time on
  the forge's clock. `DELETE /installation/token` revokes a token. The
  App's `DELETE /app/installations/<id>` removes an installation.
  Revocation and the scope matrix are built: every routed REST request and
  GraphQL document is checked against full, contents-only, expired, revoked,
  person and JWT credentials. Contents-only GraphQL refusals use `errors`.
  The conformance case records the mint response permissions object and
  the statuses of issue creation and its title PATCH with the same token.
  An installation's all-repositories selection stays within its account.
  A person's `PUT` or `DELETE /user/installations/<id>/repositories/<repo>`
  checks that user's domain admin permission and answers `204` when it changes
  the selection. The repository must belong to the installation's account.
  Removing from an all-repositories installation is `422` "Validation Failed",
  a docs-derived text unchecked live. Removing the last listed repository is
  `422` "Cannot remove the last repository from this installation.", checked
  live on both fixture installations. Neither changes reach. `PUT` on
  an all-repositories installation is `204` without changing reach.
- **Git, on the private route.** The layer serves section 3's private
  git route at the same path as the Forgejo face, over the same domain
  and its store, so the fake git program reaches a GitHub forge as it
  reaches a Forgejo one. A git call is authenticated by HTTP basic
  authentication with the login `x-access-token` and an installation's
  token that the layer minted, full or contents-only: the token
  unexpired on the forge's clock and not revoked, its installation
  present and not suspended, and the repository its account's and
  within its selection. Clone, fetch and a branch's read need the token
  alone; a push and a creation need contents write, which both kinds
  have, and the domain's protection still applies. No credential is
  `401`; a token that fails one of these, another login, or a person's
  token is `403`, and a repository outside the installation `404`, as
  its REST routes answer. The domain sees the bot's user number. The
  mint's life is the scenario's (`Config`'s token life), an hour unless
  it says otherwise, so that a story on real time sees a renewal. The
  layer keeps a bounded record of its git calls, each with its
  installation and the mint number of its token, which a scenario
  reads to tell which token a push carried. The private git route, scenario's
  token life and bounded git call record are built.
- **Ruleset mutations.** A person's `POST /repos/<o>/<n>/rulesets`
  replaces the domain's repository protection, requiring admin permission;
  `DELETE /repos/<o>/<n>/rulesets/<id>` removes it. The subset is strict:
  `name`, branch `target`, active `enforcement`, a single
  `conditions.ref_name.include` with empty `exclude`, `bypass_actors`,
  and approvals, required status checks and merge queue rules. Unknown and duplicate fields are
  `422` "Validation Failed". The pull request rule requires the approval
  count and all four booleans (false in this subset). Required status checks
  require both the contexts array and `strict_required_status_checks_policy`
  (false in this subset). Merge queues require all seven documented
  parameters: `check_response_timeout_minutes`, `grouping_strategy`,
  `max_entries_to_build`, `max_entries_to_merge`, `merge_method`,
  `min_entries_to_merge`, and `min_entries_to_merge_wait_minutes`.
  A missing required parameter answers the observed 422 Invalid request
  message naming `/rules/N` for that rule's index and data matching no
  possible input. The accepted shapes follow the documented schema.
  A merge-queue ruleset on a user-owned repository is an inferred 422
  "Validation Failed"; live's refusal form is unknown.
  Creation answers `201` with the configured ruleset id and the existing
  server encoder's bypass field; only the id is recorded, other fields are
  inferred. Deletion's empty `204` is recorded. Installation tokens receive
  an inferred `403` "Resource not accessible by integration".
- **Comment edits.** `PATCH /repos/<o>/<n>/issues/comments/<id>` takes
  exactly `{"body": "..."}` and calls the domain's `EditComment` write.
  A full installation token may edit as the domain's authorship and write
  permission allow; installation reach still applies. Fixture person tokens
  receive `403` "Resource not accessible by personal access token".
  It answers `200` with one comment from the server's comment encoder,
  including its updated timestamp. Unknown or duplicate fields are `422`.
  The person-token refusal is checked live. The installation-token edit and
  its advancing updated timestamp remain unconfirmed live.
- **The browser's pages, as plain HTTP** (seams S4), at the `web`
  routes, which the same listener serves:
  - `POST /settings/apps/new?state=<s>`, and
    `/organizations/<org>/settings/apps/new?state=<s>`, take the form's
    `manifest` field. The answer is `200` with a small page holding one
    link, `Create GitHub App` (id `create`), to the manifest's
    `redirect_url?code=<c>&state=<s>`; a test follows it as a person
    clicks GitHub's own form, and no form's `form-action` governs it.
    The code is single-use and expires an hour later. The layer keeps
    each state's posted manifest, which a test reads through the
    layer's handle by state (its `redirect_url`, `setup_url`,
    `hook_attributes` and the rest), since the page's post is not
    otherwise in sight.
  - `POST /app-manifests/<code>/conversions` makes the App and answers
    as forge-github.md, section 11, says. The converted App retains every
    posted `default_permissions` and `default_events` entry. The scenario's
    `Config::app_facts` is `Complete`, `Omitted` (GET /app omits both fields
    while retaining the grants), or `Narrow` (conversion retains only the
    first permission at read access and the first event). The posted
    manifest remains readable in each case.
  - `GET /apps/<slug>/installations/new?state=<s>` takes `account=<login>`
    (by default the App's owner), and `repositories=all` (the default)
    or `repository_ids=<id>,...`. It installs and answers `302` to
    `setup_url?installation_id=<i>&setup_action=install&state=<s>`; for
    an account already installed, `setup_action=update`.
  - A malformed manifest, or an unknown account, is `422` with a
    message.

  The keys an App can get are the layer's fixed test keys (`KEYS`,
  2048-bit, made for the fake and public). The conversion hands them out
  in turn.
- **Status posts.** `POST /repos/<o>/<n>/statuses/<sha>` refuses installation
  tokens with `403` "Resource not accessible by integration", including an
  App advertising statuses write. Fixture person tokens receive `403`
  "Resource not accessible by personal access token". Both are checked live.
- **Pull creation refusals.** No commits and an existing pull answer `422`
  with top-level message "Validation Failed" and one errors object with
  resource "PullRequest", code "custom", and message "No commits between
  <base> and <head>" or "A pull request already exists for <owner>:<head>.".
  These forms are checked live; the dialect reads errors[].message.
- **Checks.** The domain's CI statuses for a commit are rendered as the
  `github-actions` App's check runs, one per context. The run is the
  domain's `Run`, the job its `Job`, and the attempt the job's. Statuses
  that people report stay statuses. The combined status is `pending`
  with `total_count` 0 when there are none. The conformance fixture configures
  `ci.rs`, `actions.rs`, and `Cue::Message` to stand for its repositories'
  `.github/workflows/ci.yml`: push, pull opening/head movement, and merge
  groups run `ci` on their head commit. It is in_progress for 45 fake seconds,
  then completed with failure when that commit's message contains FAIL,
  success otherwise. The run and job use the same ids and logs as other
  Actions cases; the GitHub layer renders them in `reads/checks.rs`. CI runs
  once per commit and repository, coalescing overlapping fixture triggers.
- **A job's log** is a `302` to the blob origin: a second listener of
  the peer, the origin `http://<blob>`. The target is a path signed for
  600 seconds on the forge's clock. The blob listener serves the job's
  log and wants no credential. The HTTP listener honours the one-byte
  `Range: bytes=0-0` probe with `206` and `Content-Range`; other ranges
  are ignored. The in-process blob call records header presence only.
  The scenario may name a third origin, one the component does not know, for the diagnostic's
  stories. The HMAC-SHA-256 key is drawn from the seed; the signed input
  names the repository, job and attempt and includes the expiry. At the
  expiry instant the target is refused. A tampered or expired target is
  `403` with an empty body. That shape is inferred: github.com's blob
  store is not modelled. Other paths are `404`.

  `Config::third_origin` replaces the API redirect's blob origin;
  `Config::blob_redirect` makes the blob listener answer a `302` to that
  origin with the same signed target. Both default to `None` in fixtures.
  Each origin is admitted only when its origin plus the largest signed
  target fits `Limits::github.target_bytes` and `Limits::response`.
  The listener records the path and whether `Authorization` and `Range`
  were present, never their values. `Limits::blob_records` bounds this
  FIFO, with the oldest request evicted when full; each path is bounded
  by `Limits::http.head`.
- **Own approval.** A review of one's own pull answers 422 with message
  `Unprocessable Entity` and string errors containing
  `Review Can not approve your own pull request`.
- **Mergeability, merges and the queue.** `mergeable` is null for a
  drawn `settle_min..=settle_max` after a push to the head or the base.
  The domain draws from its seed, carries `checking` separately from
  `mergeable`, and defaults both bounds to two seconds in fixtures.
  Forgejo continues to render false while checking.
  A merge then decides as the domain does:
  - a merged pull answers any repeat merge `200`, `merged: true`, with the
    same merge sha, including a different supplied sha;
  - an open pull's stale head is `409`;
  - a merge refused for its checks or its approvals is `405` with
    `Repository rule violations found\n\n`, then the unmet approval
    sentence, then the unmet context lines, each followed by `\n\n`;
  - the approval sentence is `At least 1 approving review is required
    by reviewers with write access.` for 1, and `At least 2 approving
    reviews are required by reviewers with write access.` for 2.
    The singular is recorded in o/github live 2026-10-20, and the plural
    for 2 in live session 1917; counts of 3 and up use the plural by
    inference;
  - a branch that queues refuses a direct merge with `405`, the same
    header, then `Changes must be made through the merge queue\n\n`,
    then the unmet approval and context lines in the same order.
    Live session 1917 records the queue line followed by the approval
    line. Context lines after the queue line are inferred: neither
    shape was recorded with checks, and the session lists every
    violated rule. With no unmet rules, only the queue line follows
    the header;
  - an enqueue refused for approvals answers HTTP `200`, with
    `data.enqueuePullRequest` null and one `UNPROCESSABLE` error.
    Its message is `Pull request ` followed by the approval sentence,
    without trailing blank lines, and nothing is queued. Live session
    1917 records this for 1; other counts use the shared sentence;
  - after approvals, enqueue requires every queue ruleset check to have
    passed on the pull's head. An unmet check answers HTTP `200`, null
    `data.enqueuePullRequest` and one `UNPROCESSABLE` error with message
    `Pull request Required status check "<name>" is in progress.`; no
    entry is admitted. Missing, pending and failed head checks use this
    refusal. Group CI is separate: after admission, a status posted on
    the `gh-readonly-queue/` commit is accepted, and ci success lets the
    queue merge. The GitHub layer checks admission before the shared
    domain's enqueue; the domain's own queue admission remains its
    approval check.

  The layer's `pull` document answers `mergeQueueEntry` and
  `mergeCommit.oid` from the domain for queued, direct and repeated merges.
  Every REST pull object omits `merge_commit_sha`, as at the pinned
  2026-03-10 version, whether open or merged. Every merged pull read
  therefore costs the additional GQ read.
- **Rulesets and classic protection.** The scenario says how the
  domain's protection shows (`Shown::{Ruleset, Classic, Both}`):
  - as a ruleset, with its id and the App's `current_user_can_bypass`;
  - as classic protection, otherwise `404` "Branch not protected";
  - or both.
- **Rates (built).** Every answer on a token carries `x-ratelimit-*` from the
  domain's window: `rate_limit`, what remains of it, and its end. The
  App JWT calls carry none. A call refused for the rate (`limited`)
  is `403` with `x-ratelimit-remaining: 0`. A crowded one is `403` "You
  have exceeded a secondary rate limit" with `Retry-After`. A request
  with an `If-None-Match` that matches the answer's `ETag` is `304`, and
  not counted. A repository ETag hashes the rendered metadata and settings.
  Conditional repository reads check the domain's current settings before
  reusing the identity's rate snapshot for a 304; a changed response goes
  through normal admission. The snapshot map is bounded by `github.inner`.
  GitHub's `Config` sets the quota and window. Admission
  charges the domain once per authenticated request, including local table
  reads; continuations keep the snapshot and skip further rate charges.
  Installation and domain-user windows have distinct keys in the same bounded
  table. Person tokens charge their user's window; installation tokens charge
  their installation's. Admission resolves a token once to its bounded store
  slot or person-table index and carries no credential bytes. Revocation removes
  the admitted slot only after a successful charge; rate refusal leaves it in
  place. Tokens on App routes are charged before their credential-class refusal.
  The primary message "API rate limit exceeded" is inferred and
  unrecorded; `texts.rs` marks it. Both rate refusals supply `Retry-After`.
- **Faults** are section 4's, at the bytes, with GitHub's statuses. Built:
  `Config::faults` uses the Forgejo face's `Faults` type. Reset closes before
  reading the next request, including on reuse. Drop and server-error draws
  follow the final domain continuation, so an applied write survives the
  lost answer or `500`. A seed replays the draws.

### 10.2 What the domain gains

These are small, and they land after forge-kit's steps on the fakes:

- **Protection** gains `queue`, `linear`, `code_owners` and a list of
  bypassing users. A bypassing user's merge skips the checks and the
  approvals. The domain does not sign, so it keeps no signatures rule:
  the layer shows `required_signatures` from its `Config` and refuses a
  push as `GH013` when the scenario asks for it.
  The push refusal is not built: GitHub-world pushes reach the domain directly,
  with no GitHub layer between them; the text waits for P10, and the domain's
  `Rejected` stays the only push refusal.
- **The merge queue.** `Write::Enqueue { number, head }` refuses a moved
  head, as a merge does. An entry's group commit is the squash of its
  head onto the base's tip, or onto the entry ahead of it. That commit
  becomes the head of `gh-readonly-queue/<base>/pr-<n>-<head>`, with `<head>`
  in section 4's SHA-1 spelling, so the domain's CI runs on it as on any head.
  Every queue group is modelled as a squash, regardless of merge_method.
  Its message follows
  COMMIT_OR_PR_TITLE: one commit relative to the pull's base uses that
  commit's message, more than one uses the pull's title. The GitHub
  repository response advertises squash_merge_commit_title as
  COMMIT_OR_PR_TITLE, and branch reads expose the stored commit message:
  - when it passes, the base moves to it and the pull request is merged;
  - when it fails, the entry leaves with the reason `CI_FAILURE`;
  - entries behind a failed one are built again.

  Each leaving is a hook (`dequeued`, and the group's `destroyed`).
- **Installations' events.** Hooks for an installation made, removed,
  suspended, given repositories or renamed, released by the layer's
  store.

### 10.3 Deliveries and their log

Built in the GitHub layer: installation lifecycle and rename events, domain
queue action snapshots, the loopback HTTP sender that retains the recipient
status, the bounded signed-delivery store, seeded UUIDs
and increasing 19-digit ids, visibility delay, retention and capacity
removal, cursor pages honoring `per_page`, whole-delivery reads, redelivery,
first-page ETag/304, and key revocation. The server-side route decoder and
log encoders live in `skein_forge::github::server`. The scenario's
`Config::deliveries` and `Limits::deliveries` admit these bounds before
allocation. `payload_size` enlarges text-bearing issue, comment and review
payloads to exercise consumers' oversize refusal.


- **Every delivery is logged.** A domain hook is rendered as GitHub's
  payload, with `X-GitHub-Event`, `X-GitHub-Delivery` (a UUID drawn from
  the seed) and `X-Hub-Signature-256`, and is entered in the layer's log.
  Each entry has an id of 19 digits, starting at 10^18 plus a seeded
  offset, and increasing.
- **Pushing is the scenario's choice.** By default the hook's URL does
  not resolve, as temper's App's does not: no delivery is written, and
  each is logged as failed, `status_code` 502. Pushing writes it over
  loopback to the configured endpoint, as section 5 says, and logs the
  answer's status.
- **The log** is `GET /app/hook/deliveries`, newest first, paged by an
  opaque `cursor` in `Link`. The first page's `ETag` changes when an
  entry is added, so a first page asked with it unchanged is `304`.
  `GET /app/hook/deliveries/<id>` answers the delivery whole: the
  request's headers and the payload, re-encoded, and the response. An
  entry is listed `log_min..=log_max` after its change (default 1.5 to
  2.2 seconds, as on github.com), and kept for `log_keep` (default three
  days) or until `log_entries` is full, oldest first out.
- **No installation, no repository** in the list for `ping`,
  `installation` and `installation_repositories`, as on github.com.

### 10.4 Where it differs from github.com

These are contracts: a story that leans past one of them is wrong, not
the fake.

- **Only the dialect's routes and documents** are served. Others are
  `404`, and other GraphQL documents are `errors`. Built, including bodyless
  `HEAD` refusals and `OPTIONS`; representative unknown routes and documents
  have a table test.
- **Plaintext, on loopback,** at three origins: `api`, which also
  serves `web`'s routes, `blob`, and an optional third one.
- **Messages** follow the conformance's recorded texts for the refusals of
  forge-github.md, section 7, and live session 1917. Section 10.1 marks
  the inferred approval counts and queue context lines.
- **Merges** follow the domain: squashes, whatever the method asked,
  and a queue's group is built from the entry ahead, not batched.
  While a head or base move is still checking mergeability, the GitHub
  route answers `405` "Pull Request is not mergeable"
  (`merge-after-move-not-merged`, forge-github.md, section 7); GitHub
  may also answer the recorded 409 just after a move, with no merge.
  An open pull's stale supplied SHA keeps its `409` precedence. A merged
  pull's repeat merge answers `200` for any supplied SHA.
- **Ids** are the domain's numbers, small and sequential. Deliveries'
  ids are 19 digits, so `u64` is held exactly.
- **What is not modelled:** GitHub Apps other than the one App and
  `github-actions`; forks; organizations' members and teams; signed
  commits; workflows; any secondary limit beyond `crowded`; a review's
  inline comments, which the domain does not store: a pull's review
  comments read as an empty list.

### 10.5 The peer and its tests

- **The forge peer hosts it** (section 5) with
  `Peer::github(address, blob, limits, ..)`. It works as the Forgejo
  face does, over a forge it is handed, with the same rules of sharing.
- **Step tests** cover each route, each refusal and its message, the
  mint and the expiry, the JWT's window, the browser's routes, the git
  route's acceptance and each of its refusals, the queue
  through to landing and to ejection, the log's paging and its `304`,
  and a delivery decoded by `skein_forge::github` alike pushed and
  logged.
- **Memory** is checked as section 8 says, with the stores full. Built in
  dedicated `github_memory` binaries under `tests/fake-forge` and
  `tests/fake-peers`: installation rows and listed reach, minted and person
  tokens, App grants and events, manifest states and blob records are full,
  with a queued pull in every domain repository. The hosted test fills both
  connection slots and checks each owner iteration against `Host::worst_case`;
  its measured heap also includes the simulator and client.
  The API/browser and blob listeners share the peer's connection limit
  and io buffers; admission reserves two listener sockets. Each accepted
  connection uses the same bounded HTTP envelope. `worst_case` also
  counts the blob record slots and paths, seed key, configured origins
  and signed-target scratch.
- **The budget** is the focused suite's, as section 8 says. The conformance
  against the fake (section 11) runs there as one test, within 6 seconds (3
  seconds before the user's doubling of 10-22, provisional). The fake's
  clock carries the CI's and the queue's waits, so nothing sleeps.

## 11. GitHub's conformance

`tests/forge/github` is built as a tool beside section 7's. It runs the same
cases against two targets and records each fact of forge-github.md,
section 13, as `holds`, `fails` or `unchecked`. Its transcripts, facts,
masking and refusal of leaked secrets are section 7's.

- **The fake** (`--fake`, the default) is the GitHub layer in-process,
  hosted by the forge peer. It also runs as one test in the focused
  suite, which refuses failed facts and unexpected unchecked facts. Required
  signatures remain unchecked because their enforcement is unbuilt in the fake.
- **github.com** (`--live <directory>`) is opt-in. It runs from the
  owner's session alone, and only with the director's word. It is never
  run by a suite, a gate or an implementer, and never from CI.
- **A fact that holds on github.com and fails on the fake** is a finding
  for the fake. One that fails on github.com is a finding for the
  dialect, and its pin moves only after it is understood (forge-github.md,
  section 12).
- **The fixtures' directory** is the person's
  (`~/src/rust/tmp/github-conformance/`, by its README), named by
  `--live`. The tool reads no variable, and refuses a directory that is
  not `0700` or a file that is not `0600`. It holds:
  - `app.json`: the probe App's id, slug, permissions, events and
    installations;
  - `app.pem`: the App's key;
  - `webhook_secret`: checks logged deliveries' signatures;
  - `person-<owner>.token`: one fine-grained token per owner, for any
    number of owners, found by name. Each is limited to that owner's
    probe repositories.
- **The fixtures:** an App installed on a user-owned and an organization-owned
  probe repository, plus an initially ungranted spare. A run adds the spare
  to the installation and removes it again. Each probe repository carries
  the ci workflow specified in forge-github.md, section 16: push, pull_request
  and merge_group triggers, job ci, 45 seconds, and a FAIL head-message cue.
  The dequeue case requires COMMIT_OR_PR_TITLE and two passing head commits
  with FAIL only in the title; it reads and records the repository setting
  and changes no repository setting.
- **Queue installation discovery.** Before minting or repository changes,
  the startup repository reads select the first installation whose probe
  has `owner.type: Organization`. Every queue case uses its probe repository,
  installation token and matching person token. A run with no such account
  stops before any write (forge-github.md, section 16).
- **Who does what:**
  - The App's key and minted tokens drive the dialect's cases.
  - The person's tokens drive the person's part: rulesets, approvals,
    and an installation's repositories added or removed. They cannot
    list an installation's repositories, so the App's token reads them.
- **Expiry.** Each run reads each person's token's
  `github-authentication-token-expiration` and names, in `facts.txt`'s
  provenance, any token that expires within 30 days.
- **Secrets** never leave the directory. The tool registers the key,
  every token, every JWT, every minted token and the webhook secret
  before the first request. It copies none, and it masks
  `Authorization`, signed blob addresses, guids, node ids and ETags as
  well as section 7's. Numeric ids are masked by first appearance, since
  github.com's do not come in a fixed order. Request-path numbers use the
  same map as body numbers. Every response keeps its status and headers;
  transcript records are separated by CRLF so later status lines start a line.
- **The App is read only, live.** The fixture installations are
  accepted by no setup state, so setup's reconcile would take them for
  strays (forge-setup.md, section 5). A live run therefore never sends
  `DELETE /app/installations/<id>` or `PATCH /app/hook/config`, makes
  or converts no manifest, and changes nothing of the App: it reads
  the App's documents alone. The tool refuses those routes on the live
  target before any byte is written. Strays, the uninstall and the
  hook's address being set are checked on the fake alone, and their
  facts are `unchecked` live (forge-github.md, section 13).
- **Access is put back.** A repository the run adds to an installation
  or removes from it, with the person's token's `204`, is restored at
  the run's end. Each such change is written to `made.txt` before its
  request, so `--clean` restores it too.
- **A run cleans up what it made:** rulesets, pull requests (closed),
  issues (closed), branches, the spare's grant, and minted tokens
  (revoked). Each thing made is first written to `made.txt` in `--out`,
  and `--clean <out>` finishes a run that stopped partway. Each successful
  issue or pull creation also records its returned number durably; cleanup
  addresses that number. A refused creation cancels its intention without
  marking an object done. When a killed run retained only an intention,
  cleanup follows every issues-list page, and reads pull details when
  matching head and base. An unfound object remains pending. An item is
  marked done only after a read shows it closed. Cleanup mints a journaled
  installation token for the repository's lookups, close, and read-back;
  branch deletion uses the person's token. The cleanup tokens are revoked
  after restoration, including when restoration fails. Minted tokens
  stay in memory and are revoked on every controlled exit; after an
  externally killed process, cleanup reports each unacknowledged mint's
  installation and one-hour expiry in `facts.txt`, without recovering
  or writing its value.
- **Calls are few.** A full live run stays under 400 requests and about
  ten minutes, most of it waiting for the probe workflow's 45 seconds.
