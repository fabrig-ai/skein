# The forge component

Provisional, 2026-10-20. `skein-forge-connection` runs a service's calls
to a forge end to end for the protocol layer that owns it. For each call
it:

- connects through io, over TLS or in plaintext to loopback;
- runs the dialect's plan (forge.md, section 4.2) and decodes each
  answer as it streams;
- counts the requests the call cost, and reuses idle connections.

It also takes each forge's access when the owner hands it, checks it,
holds and renews the credential, resolves repositories and users, and
turns verified deliveries into hints: a webhook's, or a GitHub App's
delivery log's. When its owner closes it, it closes everything it keeps
and says so. A service that works on a forge writes no connection code.

## 1. In one page

- **A component of a protocol layer:** a step machine with its own
  state, limits, worst case, deadlines and most outputs per entry point,
  which a service's protocol layer owns and routes to
  (programming-model.md, sections 4 and 4.2).
- **Calls in, answers out.** A call goes out once; its one terminal is
  an `Answer` or an `Error`, with the requests it cost.
- **A forge starts with no access,** and its calls are answered at once,
  not connected. The owner hands access at runtime, and the same request
  replaces it (3.5).
- **Each forge's start checks its access:** that the credential stands
  for the user the access names. A wrong user, or another `4xx`, stops
  that forge alone, never the component (3.3, 3.4).
- **Credentials have a kind** (forge.md, section 8): a token is never
  renewed. A GitHub App's key signs the JWTs that mint each
  installation's tokens, and the component renews those tokens before
  they are due, so a call waits for one within its deadline (section 5).
  None leaves the component but git's, which goes to the owner alone
  (5.3).
- **An App's installations** are chosen by account: a call takes the
  token of the installation for its repository's owner (4.4).
- **Hints for latency, polling for liveness:** a listener of its own
  verifies each webhook delivery as it streams, answers it first, then
  offers its hint (section 6). For a GitHub App, a puller reads the
  App's delivery log instead (6.5).
- **It reads no file,** and its memory is bounded: the owner hands it
  every value (3.1), and nothing grows with a forge's history (7.5).

## 2. In its owner's protocol layer

- **`skein-forge-connection`** depends on lib, io's vocabulary,
  skein-http (client, and server for the listener), skein-tls's client,
  skein-json and `skein-forge`. It knows no service's configuration,
  files or domain. One component serves every configured forge.
- **The owner's protocol layer:**
  - gives io its own tokens for the component's entities, keeping the
    component's token in each, so io's events reach the component that
    asked. The component's tokens are below 2^60, so 60 bits keep one:
    its API connections from 1, its listener 2^59, and its webhook
    connections above it;
  - reports the component's earliest deadline, and fires its deadlines
    where it fires its own;
  - routes what passes between the component, io and the owner's
    domain, and decides nothing;
  - closes the component as part of its own end, and counts it settled
    only on the component's `closed` (programming-model.md, section
    5.2).

### 2.1 What crosses

Down, from the owner: a **call** (token, forge, repository, `Op`);
**refused** and **refused hint**, an answer or a hint its domain did not
take; **returned**, an answer handed back in the domain's drain; an
**offer**, when the domain has room; a forge's **connect**, with its
access (3.5); a GitHub forge's **installations**, the ones the owner
accepted (4.4); and the **close**, its last act.

Up, to the owner: **answered**, a call's one terminal with its token,
cost and result; a **hint**, after its delivery's response is queued
(6.2) or as its log's row is read (6.5); **listening**, the webhooks
listener's bound address (6.4); **listener lost** (6.4); **connected**,
a forge's start done (3.3); **stopped**, a forge's access refused, at
most once per connect; an **installation**'s change (4.4); a
**git token**, an installation's for git (5.3); a **fact**, a
diagnostic for the owner's log; and **closed**, the last event (8.5).

| Down | Up |
|---|---|
| `Request::Connect { forge, access }` | `Event::Connected { forge }` |
| `Request::Installations { forge, accepted }` | `Event::Stopped { forge, why }` |
| | `Event::Installation { forge, installation, account, change }` |
| | `Event::GitToken { forge, installation, account, token, expires }` |
| | `Event::Fact { forge, fact }` |

An installation event's `account` is `Some` with its current id and
login, or its last known value on `Deleted`. `None` occurs only on
`Deleted` of an id never seen; a missing accepted id keeps its known
account across later reads. `change` is `Created`, `Deleted`,
`Repositories`, `Suspended`, `Unsuspended` or `Renamed`, and `expires` is
the token's expiry in wall seconds, as the forge said it. A `fact` is
`Origin { origin }`, a job log's origin not configured (4.5);
`Unkept { installation }`, an installation the table has no room for
(4.4); or `Gap`, a delivery log read short of its cursor (6.5). A fact
is a diagnostic: the owner logs it, and no decision, the component's or
the owner's, rests on it.

An entry point emits at most 16 events up and 16 requests to io, and the
owner leaves room for them before it.

### 2.2 Room, and the drain

(programming-model.md, sections 5.2 and 7.) An answer or a hint the
owner cannot take is held and offered at its next offer: at most one per
call, and one per webhook connection, which reads nothing more while its
hint is held. One the domain returns during its drain is never offered
again: the next process reads afresh.

### 2.3 The rules it keeps

API only, with `page` and `limit` always and within the forge's cap
(7.3). Paths and users are cached by their change token (section 4).
Webhooks are hints, answered first (section 6). The rate is honoured: a
`429` is `RateLimited { after }`, and nothing retries. GitHub's pauses
are the same error, with `after` as forge-github.md gives it (section
7), and each answer carries the rate's window the forge reported (4.1).
A request is a call's step, a start read, a renewal (5.1), a read of the
installations (4.4), a redirect followed (4.5) or the puller's (6.5),
with no heartbeat. The rest are the client's and the plans'
(forge-client.md, sections 4 and 6; forge.md, section 5.2).

## 3. Start

### 3.1 What the owner hands it

The values of forge.md, section 2.2, from no file:

- **for each forge:** its number, unique among the forges; its dialect,
  `Kind::Forgejo` or `Kind::GitHub`; its API's socket address; its
  transport, TLS with a server name and what to trust, or plaintext to
  loopback with the name the forge was configured by, when it was one;
  its access, when the owner has one at start (3.5); and, for GitHub,
  its job logs' origins (4.5);
- **the listener's address,** when webhooks are used;
- **the word** (forge.md, section 6) and **the user agent** of every
  request: temper's are `temper` and `temper/0.0.0`;
- **the limits** (7.2) and **the shared values** (7.3).

The component makes each request's `Host` from the address and the
transport's name (forge.md, section 5.1); the owner hands no `Host`. It
reads a TLS name back (`Name::server`, tls.md, 3.4): a DNS name goes to
`host` as the name, and an IP name in place of the address's IP, with
the address's port, so that it is written as an address is. An IP name
that differs from the address's IP is the `Host`, as a DNS name is: the
name names the forge, and the address only reaches it. Start's check and
each request make it by the same function, from the same values, so they
agree.

### 3.2 What start checks

A refusal names the value, or both limits where two disagree (7.4).
Nothing starts and no io is requested. It refuses: more forges than
`forges`, or two with one number; an in-process name for an address, and
plaintext off loopback; a plaintext name that is empty or holds a byte
at or below a space, at or above DEL, or one of `/:[]`; an access that
3.5 refuses; a user agent that is empty or holds a byte below a space
other than tab, or DEL, which skein-http would refuse in every request's
head; more than `log_origins` origins, an origin on a Forgejo forge, and
an origin whose host is not a DNS name or whose address 3.1's rules
refuse; an `https` origin on a plaintext forge, which has no trust to
check it with; a listener that is not a loopback IP socket; a word that is not
1 to 16 bytes of lowercase ASCII letters, digits and `-`; and the limits
7.4 checks. A TLS name is refused by none of them: `Name` holds only a
DNS name or an IP address, and either makes a `Host`. The check makes
each forge's `Host` as its requests will (3.1), for 7.4's head.

JSON response bodies pass through a connection-local intake. It reads
what has arrived from HTTP, at most the remaining intake room, and meets
the tokenizer's finer demands locally. There is at most one body read
outstanding below. HTTP retains responsibility for framing and the body
bound, so read-ahead cannot cross a response. Header lines and raw bodies
keep their own demands.

A pump routes at most 64 child-machine movements. At that boundary,
local work remains ready or its answer remains queued. Held bytes meet
the tokenizer before its end is delivered; queued HTTP terminal events
are consumed before another refill is requested. Closing, discarding or
reusing an exchange clears its local bytes and demand state.

### 3.3 Each forge's start

A forge's start begins at each connect (3.5). Calls for a forge wait
until it is ready, within their deadlines. Its start runs two of the
dialect's plans (forge.md, section 4.1): the identity read, which says
who the credential stands for, and the page cap read, which gives the
forge's page cap. Forgejo's are `GET /api/v1/user`, for the `id`, and
`GET /api/v1/settings/api`, for `max_response_items`. GitHub's are 3.4's.
Each plan runs on a connection of the forge's own within `deadline`, and
is counted in no call's cost. The component decides from what a plan
heard (8.3), and says `connected` when the forge is ready.

- **Stopped:** a user other than the access's, or any `4xx` but `429`.
  A wrong credential would make the owner act as someone else, and
  another `4xx` says the address names no such API; asking again would
  mend neither. The component says `stopped`, naming the forge and the
  id it heard, or the read and the status. The forge then holds no
  access, and its calls are answered as 3.5 says, until the next
  connect. The other forges run on.
- **Asked again** for anything else (8.3), after `start_backoff`, then
  at double the wait up to `start_backoff_max`, or a `429`'s
  `Retry-After` when longer, however long: the rate is honoured (2.3),
  the sum saturates, and the forge's calls meanwhile end at their
  deadlines (`Unavailable`, forge.md, section 5.4).

### 3.4 A GitHub App's start

A GitHub forge's credential is an App's (5.2), and its start runs these
of forge-github.md's plans (section 8), each on the App's JWT unless it
says otherwise:

1. **The installations,** `GET /app/installations`, paged, which fill
   the installations' table (4.4). A `401` says the key is refused:
   stopped, `KeyRefused`.
2. **Each accepted installation's tokens,** minted (5.1), the API's
   token and git's narrower one (5.3).
3. **The identity read,** GraphQL's `viewer { databaseId login }` on one
   installation's token: the bot's user id, which must be the access's
   user. With no installation accepted, there is no token to read it on.
   The forge is ready without it, and the read runs at the first
   accepted installation's mint, its user checked as here.
4. **No page cap read:** GitHub's cap is the dialect's constant, 100.

Each read is asked again, or stops the forge, as 3.3 says. A mint's
refusals are 5.1's.

### 3.5 Access, at runtime

- **What it is:** `Access { credential, user, webhook_secret }`. The
  credential is `Credential::Token(Secret)` for Forgejo and
  `Credential::App { id, key }` for GitHub (forge.md, section 8). `user`
  is the forge user id the credential must stand for, never 0, which
  setup's connect read and checked before the owner kept it
  (forge-setup.md, sections 4 and 5). `webhook_secret` is the hook's,
  when the forge has one.
- **None at start is allowed.** A forge with no access runs no start,
  and each of its calls is answered `NotConnected` at once, cost 0. So is
  each call to a stopped forge, and to a forge whose replacement is
  refused, until its next connect.
- **`connect` hands it,** as the first access, a replacement or a
  reconnect after a stop alike. The component checks it at once, then
  starts the forge again (3.3). A connect it refuses says `stopped`,
  `Invalid`, naming the value: a token past `token_bytes`, empty, or with
  a byte skein-http would refuse in a head; a user of 0; a credential of
  the other dialect's kind; an App key that is not a 2048-bit RSA key's
  PKCS#1 DER, or that is past `key_bytes`; a secret on a GitHub forge
  (6.1) or on a forge with no listener, or one empty or past
  `hook_secret_bytes`.
- **A replacement ends what the old access began, as a revocation
  would.** An exchange already written is heard to its end, so a write
  that reached the forge is answered. A call's next step is not sent on
  the old access: the call ends `Forbidden`, as the forge would answer a
  revoked credential. Calls waiting for the forge's start wait for the
  new one, within their deadlines. The old access is dropped, not kept.
- **At most one of each answer per connect generation:** `connected`
  when the start is done, and `stopped` if access is refused, including
  a later refusal after Ready (deferred Identity or a key refused at a
  mint). Configured access starts a generation too. A replacement ends
  the old generation unheard: only the newest generation may answer.
- **Another user is refused.** A connect whose user differs from the
  access the forge holds says `stopped`, `Invalid`, naming the user, and
  the forge holds no access after it. A credential that stands for
  someone other than its access's user stops at the start's check
  (3.3). So the owner's domain never acts as a second user on one forge,
  and its client refuses a second author too (forge-client.md, 7.1). The
  component never holds an admin's token (forge-setup.md, section 3).

## 4. Calls, repositories and users

### 4.1 Calls

- **Admission is the owner's.** At most `calls` calls are outstanding;
  the component never refuses one, and `Busy` is the owner's word
  (forge.md, section 3.4). A call for a forge no one configured ends
  `Unavailable` at once, one for a forge with no access `NotConnected`
  (3.5), and one for an account with no accepted installation
  `NotInstalled` (4.4), each at once.
- **A call runs its plan:** each step sends one request and hears its
  answer. It waits, holding no connection, for its forge's start; one
  whose repository has no path kept resolves it (4.2), and one that
  needs a login not held looks it up (4.3). A locate, which finds a
  repository by owner and name, names repository 0 (forge.md, 3).
- GitHub File keeps the requested head within the answer bound; Compare and
  PullFiles keep whole file bodies to their bound, with PullFiles reading
  each before side at the merge base.
- **Cost** is the component's count, never a plan's (forge.md, 4.2): one
  for each request whose first byte was written, the requests that
  resolve a path or a login included. It goes up with the one terminal
  (forge.md, section 5.4). Beside it go `free`, how many of those
  requests the forge answered `304`, which its rate does not count
  (always 0 until a call's read takes a validator, forge-github.md,
  section 8.5), and `rate`, the window the forge reported on the call's
  last answer, when it reported one:
  `Rate { limit, remaining, reset_at }`, wall seconds, per installation
  on GitHub.
- **The deadline** is `deadline` from admission, at most the owner's
  `lifetime`, so no byte of a write goes out past the time the owner
  counts on. A call still waiting at it ends `Unavailable`. An exchange
  that has written no byte fails `Unseen`, and one that has, `Seen`; the
  plan gives the `Error` (forge.md, section 5.4). The connection is
  aborted.

### 4.2 Repositories

The component keeps a row for each repository, by forge and id: its path
(owner and name), object format, default merge style and `html_url`,
and, on GitHub, its owner's account id, which chooses its installation
(4.4). The plan reads a row and fills it from the answers it hears
(forge.md, 4.2).

- **Filled** by the read of the repository by id, counted in the call
  that needed it; by a locate; and by a read of its settings. On
  GitHub, each delivery read whole (6.5) renames a kept row within its
  owner, and drops it for a new owner, whose account may differ. Its
  repository names neither the account nor the settings, so it makes
  no row.
- **Dropped** when a call by its path ends on a `404` or is answered a
  redirect, or a merge is refused its style (forge.md, 5.4); the next
  call resolves the id again. A `404` that is an answer (forge.md, 5.2)
  keeps it.
- **At most `repositories` rows;** the one used longest ago goes first.

### 4.3 Users

A table of logins by user id, filled from every user object an answer
carries (a user, a status's creator when not `null`, the requested
reviewers, a collaborators' listing). An id it lacks is looked up by the
dialect's search, counted in the call (Forgejo's:
`GET /api/v1/users/search?uid=<id>&limit=1`); a user not found fails the
call `Missing`. At most `users` rows; the one used longest ago goes
first. A lost row costs one request, and a restarted component starts
with none.

### 4.4 A GitHub App's installations

An App is installed on accounts, users or organizations, one
installation per account, and each installation has its own tokens and
its own rate (forge-github.md, section 8.5). The owner accepts
installations (its setup's, forge-setup.md, section 5.4) and hands their
ids down in `installations`, which replaces the accepted set, whenever
it changes.

- **The table:** at most `installations` rows for each GitHub forge, by
  installation id. Each holds the account's id and login, whether it is
  suspended, whether the owner accepted it, and, for an accepted one
  that is not suspended, its API token and git's, with their expiries.
  A full table keeps the accepted rows first. It is read whole from
  `GET /app/installations` at each start (3.4), when `installations`
  names an id it has no row for, and when the log says the
  installations changed (6.5).
- **A call takes its installation by account:** its repository's row
  gives the owner's account id, and the accepted row for that account
  gives the token. A locate takes the row whose login is its owner's. A
  call that names no repository, a user's lookup or the identity read,
  takes the accepted installation used most recently.
- **Refused at once, cost 0:** an account with no accepted row,
  `NotInstalled`; a suspended one, `Forbidden`.
- **Not refused at once:** a repository that its owner's installation
  does not select. Its call is sent, answered `404`, and fails `Missing`
  (forge.md, section 5.4). The component keeps no table of the
  repositories an installation selects.
- **A repository with no row:** the component reads it by id with each
  accepted installation's token in turn, most recently used first, until
  one answers. That is at most `installations` reads, each counted in
  the call. The first that answers fills the row, and the call goes on
  with the owner's installation. If every one answers `404`, the call
  fails `Missing`. Rows come from the calls' own reads and the
  deliveries (4.2), so this path runs only for an id first named after
  a start, or one whose row the table dropped.
- **The first successful whole read** compares with the owner's accepted
  set: an accepted id absent is `Deleted`; an accepted suspended row is
  `Suspended`; an unaccepted retained row is `Created`; an accepted
  unsuspended row is silent. Failure preserves this baseline, consumed
  once per construction; reconnects compare tables.
- **Missing accepted ids** in a changed set share one whole reread; a
  read already running for that set suffices. On success each id still
  missing is `Deleted` once, deduplicated against the baseline and the
  old table's deletion. Failure retries with backoff without marking
  absence checked. Equal sets, even reordered, and later timer fires
  cause no reread or repeated deletion; a changed set rearms the rule.
  A set whose ids all have rows needs no read. A changed accepted set
  invalidates an in-flight read's staging; its written exchange finishes
  before page 1 is read for the latest set.
- **Changes go up** as `installation` events, later reads comparing
  the table read with the one it replaces: `Created` for a new id,
  `Deleted` for one gone, `Suspended` and `Unsuspended`, and `Renamed`
  for an account whose login changed. New and deleted rows report only
  membership; a suspension change precedes a rename on the same row.
  Acceptance alone emits no installation change. A rename leaves calls by
  repository untouched, since they choose by account id. A log row that
  says an installation's repositories changed sends `Repositories` up
  for that installation, without names: the owner lists them afresh
  with its setup host's `Reachable` (forge-setup.md, 4.8 and 5.9),
  which the component takes no part in.
- **Retention is bounded across pages.** Accepted rows win, displacing
  strays when needed; among strays the earliest encountered stay. A row
  seen at the forge but lost to local capacity is not `Deleted`.
  `Fact::Unkept { installation }` is emitted at most once per whole
  page-1-through-end read attempt, its id naming the first discarded or
  displaced row. Further overflow in that attempt emits no fact. A
  failed attempt ends it, and its retry may emit one again; a fact
  already emitted remains a capacity diagnostic if a later page fails.
  An unretained row emits no `Created` and goes no further. A pending
  fact holds the next page until output room is available. Only whole
  successful reads commit; a bounded cursor resumes their changes
  within each entry point's remaining reservation, before startup's
  mints and `connected`.

### 4.5 A job log's redirect

GitHub answers a job log's read with a `302` to a signed address on
another host, valid for minutes (forge-github.md, section 10). The plan
answers `Redirect { origin, target }`. Its origin is the scheme, host
and port of `Location`, and its target is the path and query.

- **Followed once,** and only to an origin the owner configured for the
  forge, `log_origins`: tls alike, host alike in ASCII case, port alike.
  Each origin comes with its address, which the shell resolved at start,
  since io connects to addresses alone and resolves no name at runtime
  (io.md, section 4). The configured origins are at most `log_origins`.
  An empty list, the default, reads no job log:
  a job's read answers `Refused`.
- **To another host,** the call fails `Refused` at once, and
  `Fact::Origin { origin }` names the origin, as `Redirect`'s, so the
  operator can add it. It is said once for each origin from each
  connect: the component remembers at most `log_origins` origins said,
  the oldest forgotten first when full, and forgets them all at the
  forge's next connect. A fact is a diagnostic: no decision rests on it,
  and a call fails alike whether its origin was said or not. The origins
  are settings, unchanged until a restart, so the client does not retry.
- **The request** carries no credential and no `Range`, which that host
  ignores. It runs on a connection of the pool to the origin's address,
  as a call's do, with TLS to the origin's host and the forge's trust,
  within the call's deadline, and is counted in the call. The API's
  connection goes back idle. Its answer streams through the plan as
  the log's body. A redirect answered by a redirect fails `Unavailable`.

## 5. Credentials and their renewal

- **A kind** says how a credential is made and how long it lasts
  (forge.md, section 8): a token lasts until revoked (Forgejo's), and a
  GitHub App's key lasts until the App's owner replaces it, and signs
  the JWTs that mint each installation's tokens, which expire within
  the hour (forge-github.md, sections 8.1 and 8.2).
- **One per forge,** in the component's own table: a token, a `Secret`
  of at most `token_bytes`, or an App's id and key, at most `key_bytes`,
  with the installations' tokens (4.4). The dialect says how each
  renders on a request: Forgejo's is `Authorization: token <t>`, and
  GitHub's `Authorization: Bearer <t>`, a JWT's or an installation
  token's as the plan's step says (forge-github.md, section 3).
- **It reaches no one:** no record, request line, fact, hint or event
  carries a credential, nothing logs it, and no entry point returns it.
  The one exception is git's token, handed to the owner alone (5.3).

### 5.1 Renewal

A token is never renewed: Forgejo's is held until it is replaced (3.5).
An installation's tokens are, by the component itself:

- **Minted** by forge-github.md's plan (section 8.2), `POST
  /app/installations/<id>/access_tokens` on the JWT, for each accepted
  installation that is not suspended: at the start (3.4), when the owner
  accepts it, and before each falls due. A mint answers the token and
  its `expires_at`, wall seconds.
- **Due `renew_before` its expiry.** The expiry is turned into the
  component's own time once, when the mint is heard: the life left is
  `expires_at` less `env.wall`, and the due time is `env.now` plus it,
  less `renew_before` (programming-model.md, section 9). A wall that
  jumps later moves no due time.
  - A mint already due when it is heard is used, and renewed after the
    private backoff, as a failed renewal is: a short `expires_at` or a
    skewed wall makes no loop.
- **Run on the forge's own connections,** within `deadline`, each
  request counted in no call's cost, as a start's reads are.
- **A call that needs a token while its mint runs waits,** within its
  own deadline, and is sent with the new one. Until the old one
  expires, calls go on with it. The new value replaces the old, which is
  not kept.
- **A refused mint:** `401` says the JWT is refused, so the forge stops,
  `KeyRefused` (3.3). `404` says the installation is gone, and `403`
  that it is suspended: the installations are read again (4.4) and the
  change goes up. Another `4xx` but `429` stops the forge,
  `TokenRefused`, naming the installation and the status: the App's
  permissions do not cover the mint (forge-github.md, section 8.2), and
  asking again would not mend that. Anything else is asked again as a
  start's read is (3.3).
- **A token refused in a call,** `401` on an installation's token, ends
  that call `Forbidden` (forge.md, section 5.4), and the component mints
  that installation's tokens anew, once until a mint is heard.

### 5.2 The App's key, and its JWT

- **The key** is the App's RSA private key, 2048 bits, as PKCS#1
  `RSAPrivateKey` DER, at most `key_bytes`. Setup turns the PEM GitHub
  gives into it, at the conversion and at a paste (forge-setup.md,
  section 5.2). The component holds it as a `Secret` and checks it at
  connect (3.5).
- **The JWT** is forge-github.md's (section 8.1): the App's id as its
  issuer, issued at `env.wall` less 60 seconds, and expiring 600 seconds
  after that, so 540 seconds from its signing. It is signed RS256 by
  `skein_tls::sign_rs256` (tls.md, section 4). Signing is local: no
  request.
- **Reused** until less than `deadline` of its life is left, then signed
  anew, so no request goes out on a JWT that ends before its deadline.
  7.4 keeps `deadline` within half its life, so each JWT serves for at
  least half of it.
- **A key that will not sign,** which 3.5's check makes impossible,
  stops the forge, `Invalid`, naming the key.

### 5.3 Git's tokens

Git pushes with a token narrower than the API's: `contents` alone, for
the whole installation (forge-github.md, section 8.2). The component
mints it beside the API's token, renews it the same way (5.1), and hands
each one up as a `git token`, with its installation, account and expiry,
at each mint.

- **It keeps only the expiry,** to renew it: the value goes up once, and
  the owner holds it for git, in one git identity per forge whose login
  is `x-access-token`.
- **The owner drops it** at the installation's `Deleted` or `Suspended`,
  and at an `installations` that no longer accepts it. The component
  mints none for those.
- **GitHub doesn't fully honour the narrowing:** a contents-only git
  token can still open issues as the bot for its hour, though it is
  refused an edit (observed 2026-10-20, `token-narrowed-creates-not-edits`,
  forge-github.md, section 8.2). The narrowing stays; a second App would
  close the gap, and that is the user's call.
- **Forgejo has none:** its git token is the owner's, set up beside the
  API's (forge-setup.md, section 4.3).

## 6. The webhooks' listener

Deliveries cut polling's latency and decide nothing (forge.md, section
7). Polling stands with no webhook (forge-client.md, section 4) and
finds whatever a delivery lost, refused or never sent told of.

### 6.1 The listener, and each forge's path

- **The listener** is a socket of the component's own, on loopback:
  plaintext HTTP on skein-http's server, at the owner's address. A forge
  on another host polls alone until skein-tls has a server side (11).
- **One path per forge,** which the dialect gives (Forgejo's is
  `POST /hooks/<number>`, forge.md, section 7), and which setup's hook
  names (forge-setup.md, section 4.5). A connection takes one request at
  a time and is kept alive.
- **A forge's secret** comes only with the listener. A forge with none
  has no path: it is answered as a path that names no forge.
- **A GitHub forge has no path:** its deliveries are read from the App's
  log (6.5), whose reads TLS and the JWT vouch for, so its access holds
  no secret.

### 6.2 Verified as it streams, answered first

The component computes the HMAC-SHA256 as the body arrives, with
`sha256::Hmac` keyed by the forge's secret, and decodes the body at the
same time into the few fields the hint reads. Nothing of it is believed
until the whole body is read and the signature holds, compared with
`sha256::equal`, in constant time (lib.md, 6.2). The header, the events
and the fields are the dialect's (forge.md, section 7).

Once the body is read and verified, the `204` is queued, and only then
does the hint go up, so the owner never makes the forge wait on its work
(http.md, 5.4). A verified delivery that hints nothing, one without the
dialect's event header (`X-Forgejo-Event`) among them, is answered `204`
too. A hint names the forge's number and the payload's repository id;
one for a repository the owner has not adopted goes up all the same.

### 6.3 What it answers

The head is checked in this order. A refusal is answered, its body
discarded, and the connection closed.

| The request | Answer |
|---|---|
| a head the server itself refuses | skein-http's own answer: a `4xx`, or `501` or `505` |
| a path that names no forge, or a forge with no secret | `404` |
| a method other than `POST` on a forge's path | `405` |
| no signature, or one that is not 64 lowercase hexadecimal digits | `403`, body unread |

A body is read within `hook_body_bytes`. A body too long (`413`), a
wrong signature (`403`) and a verified body the decoder finds malformed
(`400`) are answered as forge.md, section 7, says; each closes the
connection and hints nothing. A `204` alone keeps it alive. A request
not read within `hook_deadline` closes with no answer.

### 6.4 Bounds, and a listener that fails

Connections (`hooks`), the head (`hook_head_bytes`), the body
(`hook_body_bytes`) and a request's time (`hook_deadline`) are capped; a
full listener accepts no more until one closes. A push payload's commit
list is skipped by the decoder, not held. The listener's bound address
goes up once as `listening`, so an owner that gave port 0 learns its
port. A listener that fails while the component runs is said once, as
`listener lost`, and it and its connections close. What the owner does
is its own.

### 6.5 A GitHub App's delivery log

A GitHub App's hook points at no listener (forge-setup.md, section 5.5):
GitHub keeps each delivery it could not make in the App's log, and the
component reads the log instead. The reads are forge-github.md's plans
(section 9.3), on the App's JWT and the forge's own connection, counted
in no call's cost, one at a time.

- **Each `log_poll`,** while the forge is ready, the puller reads the
  log's first page, newest first, conditionally: with the validator the
  last first page answered, kept for the forge (at most
  `VALIDATOR_BYTES`, forge.md). A `304` says nothing is new, and the
  poll ends there.
- **It reads back to the newest it kept.** The puller keeps the guid of
  the newest delivery it read. A poll reads rows, and pages by the log's
  cursor, until it meets that guid, or a row delivered more than
  `log_catch_up` ago, or `log_pages` pages, and then keeps the first
  row's guid as the newest. A redelivery repeats its delivery's guid:
  the puller skips it, and never takes it as the newest.
- **With none kept,** at the component's first start of the forge, the
  first poll keeps the newest guid and hints nothing older: the client's
  reads afresh at its restart cover what came before (forge-client.md,
  section 8). A later start of the forge, after a connect or a stop,
  reads back to the guid kept, within `log_catch_up` and `log_pages`.
- **A gap,** a poll that ends on `log_catch_up` or `log_pages` before it
  meets the guid kept, is said once as `Fact::Gap`, and the
  installations are read again (4.4). The client's polling finds the
  rest (section 6).
- **A row's event decides what it costs:**
  - one of an installation's (forge-github.md, section 8.3) reads the
    installations again, once for the poll (4.4), and one saying an
    installation's repositories changed sends that installation's
    `Repositories` up;
  - one the dialect hints is read whole, `GET
    /app/hook/deliveries/<id>`, within `delivery_bytes`, and decoded as
    it streams into its hint, as a webhook's is (6.2). Its repository's
    path renames or drops that repository's row (4.2). One past `delivery_bytes` hints
    nothing, and the client's polling finds it;
  - any other is passed over, unread.
- **A hint goes up** as 6.2's do: one held for the forge while the owner
  has no room, and the puller reads nothing more until it is taken.
- **What a poll is refused:** a `401` stops the forge, `KeyRefused`
  (3.3). Anything else ends the poll, which the next `log_poll` asks
  again; the guid kept stays.

## 7. Entities, limits and the worst case

### 7.1 Entities

Each is in a slab of the component: a **call**, a **forge's start**, a
**forge connection** (bound to io's socket and one forge, or one of its
log origins, with its stack: TLS, HTTP client, JSON tokenizer, the
answer's decoder), the **tables** (repositories, users, page caps,
credentials, secrets, and a GitHub forge's installations and log
origins), each forge's **own connection**, which its start, its mints,
its installations' reads and its puller share, one request at a time, a
GitHub forge's **puller** (the guid and validator kept, where its poll
is, and the hint it holds), the **listener**, and a **webhook
connection** (its stack: HTTP server,
tokenizer, the delivery's decoder; the HMAC state, the fields decoded
and the hint it holds until the owner has room).

### 7.2 Limits

The component's own; their values are the owner's.

- **Forges and tables:** `forges`; `token_bytes`, the largest token, an
  installation's too; `key_bytes`, the largest App key; `repositories`
  and `users`, the two tables' rows; `installations`, a GitHub forge's
  installations' rows (4.4); `log_origins`, at least 1, a GitHub forge's
  job log origins, and the most origins not configured it remembers
  having said (4.5).
- **Connections:** `connections`, equal to the owner's `calls`, so no
  call waits for one and the component never says `Busy`; an idle
  connection of one forge closes for a call to another when none is
  free. Each forge has one more, its own (7.1). `idle` is how long one
  is kept: below the keep-alive time of
  whatever serves the forge's API by more than a request's round trip.
- **What is read:** `head_bytes`, the largest response head, and
  `headers`, the most fields of a head, a delivery's too; `body_bytes_in`,
  the body read, streamed, so it caps time and bytes, not memory;
  `depth`, the JSON nesting followed (Forgejo's documents need about 8);
  `chunk`, the most text of a string demanded at once, and the most one
  read of a delivery takes. A JSON response intake holds `max(chunk, 5)`
  bytes, enough for a tokenizer chunk or its largest literal; each refill
  is bounded by its remaining room.
- **What is kept,** the dialect's (forge.md, section 4.1):
  `string_bytes`, `body_bytes`, `inner` and `target_bytes`.
- **Time:** `deadline`, a call's and each request's of a forge's own,
  from when it is asked; `start_backoff` and `start_backoff_max`;
  `renew_before`, how long before an installation token's expiry it is
  minted anew (5.1).
- **The delivery log** (6.5): `log_poll`, the time between a puller's
  polls; `log_catch_up`, how far back a poll reads; `log_pages`, the
  most pages it reads; `delivery_bytes`, the largest delivery read
  whole.
- **Webhooks:** `hooks`; `hook_head_bytes` and `hook_body_bytes`, the
  largest delivery's head and body; `hook_deadline`, a request's read
  and answer, from its connection's accept or its last answer;
  `hook_secret_bytes`, the largest secret, at least 32.
- `tls`, and what HTTP and JSON take as their own (tls.md, http.md,
  json.md).

### 7.3 Shared values

The owner hands the component, as plain numbers, what it shares with its
domain: `calls`, `rows` (the most rows of a listing it keeps),
`answer_bytes`, `op_bytes` (an operation's text and key together),
`lifetime` (the longest a sent write may still arrive), `name_bytes`
(the longest name its domain keeps), `repositories` (the most it
watches), and io's caps. A plan is handed only what it reads: a
listing's page size, `body_bytes`, `rows` and `answer_bytes`. A page
size is the least of `rows`, the forge's page cap, and `answer_bytes`
over the listing's largest row, measured from `string_bytes`,
`body_bytes` and `inner`. Compare pages commit ids, each 32 bytes,
by the same rule. Its commits and files each fit `rows`; together their
bytes fit `answer_bytes` (forge-github.md, 4.1).

### 7.4 Checked at start

Each refusal names both limits:

- every limit is above 0, and the worst case fits a `u64`;
- `connections` equals `calls`, and `deadline` is at most `lifetime`;
- `body_bytes` is at least 2 * `op_bytes` + the word's length + 15: a
  marker writes the key twice over, in hexadecimal (forge.md, 6);
- `string_bytes` is at least `name_bytes`, and `repositories` at most
  the owner's;
- each listing holds at least one of its largest rows in `answer_bytes`,
  by the measure of forge.md, section 3.7, so no page size is 0;
- the dialect's own `check` of its limits (forge.md, section 4.1);
- for each forge, the request head fits skein-http's `request`, the send
  cap of the stream below: `target_bytes` of target, its `Host`, its
  credential (a GitHub forge's the longer of `token_bytes` and
  `github::jwt::JWT_BYTES`, forge-github.md, section 8.1), the user
  agent (the owner's, of any length) and 192 bytes of framing; and for
  each log origin, its target and host likewise, with no credential;
- with a GitHub forge configured: `deadline` at most 270 seconds, half a
  JWT's life (5.2); `renew_before` at least twice `deadline`, so a mint
  that fails is asked again before its token expires, and at most 30
  minutes, half an installation token's hour; and `key_bytes` at least
  `skein_tls::RS256_KEY_BYTES`, the largest 2048-bit key's DER (tls.md,
  section 4);
- `hook_secret_bytes` is at least 32;
- each stacked machine's largest demand fits the cap of the stream
  below it: HTTP over TLS and io, TLS over io, JSON over HTTP, and the
  listener's server over io and JSON over it.

### 7.5 The worst case

It is the sum of:

- for each of `connections`: its stack, its intake and output caps, its
  connection-local JSON intake and one delivery overlap, each at
  `max(chunk, 5)`, its decoder and the answer as it is built (twice `answer_bytes`, for a
  body kept raw while it grows), TLS priced on every connection, since
  the bound knows each kind's count, not their transports;
- with a GitHub forge configured, for each call or forge-owned connection:
  one boxed GitHub exchange, retaining its request, raw retention and
  comparison selection together;
- for each of `calls`: its record, plan and copy of the operation, and
  its held answer, at most `answer_bytes`;
- each configured dialect's repository/user tables, full and shared across
  its forges by forge key. The bound takes each kind's count and prices a
  dialect's tables, decoder share and call terms only when a forge of that
  kind is configured, each call slot at the larger dialect's terms. Discovery's
  staged row, retained operation, route revisions and exchange/header
  handoffs coexist with the plans, row copies and held answers. Each
  active call's discovery also holds at most `installations` ids with
  eligibility revisions and a cursor; the successful probe plan and
  original plan may overlap at handoff; GitHub files state holds its bounded
  descriptor queue, File and Commit lists, current file, previous path and
  their owned paths and raw bodies;
  each forge's credential, secret and name, a
  TLS name's DNS text or a plaintext name (an IP name holds none); and
  the word and the user agent;
- for each of `hooks`: its stack, HMAC state, decoded fields and held
  hint; and the listener;
- each GitHub forge's own connection, priced in addition to `connections`;
- for each GitHub forge: its key at `key_bytes` and its JWT at
  `JWT_BYTES`; its installations' table full, each row's login at
  `name_bytes`, its API token at `token_bytes` and two expiries; both
  bounded metadata tables, old-row seen flags and accepted/missing-id
  bookkeeping with known accounts; a 100-row page and its handoff
  overlap, one overflow fact and the diff cursor's held account; private
  row states, eligibility and Full-token revisions, each scope's monotonic
  expiry and due time, pending work, selected work and retry state,
  a deferred Identity candidate and one held git token at `token_bytes`
  with its account and the old/new API token handoff, all at simultaneous
  peak; its log origins, each a host of 253 bytes and an address, and its
  said origins, each a host bounded by `target_bytes` (the dialect
  bounds Location, including the host), both at `log_origins`; and its
  puller, its validator and cursor at the dialect's bounds, a guid, the
  page of log rows and its handoff, the delivery's streaming decoder
  and its row's bounded event word, the payload's repository path at
  39 owner bytes and 100 name bytes, and the hint it holds.

A long thread or a large payload costs time, not memory. The owner
counts its io's worst case once.

## 8. State machines

### 8.1 A forge connection

| State | Holds, and demands | On |
|---|---|---|
| Closed | nothing | a call, or a request of its forge's own: Connecting |
| Connecting | the holder | connected: Handshaking, or Sending without TLS; failed: `Unseen`, Closed |
| Handshaking | the holder; TLS's demand | done: Sending; failed: `Unseen`, Closed |
| Sending | the holder, its request; room | sent: Reading; a fault: `Unseen` if skein-http says `Closed`, else `Seen`; Closed |
| Reading | the holder, the decoder; the head, then the body as demanded | answered: the next step's Sending, or Idle with the answer up; a fault: `Seen`, Closed; a malformed answer is heard as such (forge.md, 4.2), Closed |
| Idle | nothing; none | a call to its forge: Sending; `idle`, the forge closing, or a call to another forge with none free: Closed |

A connection the forge marked `Connection: close`, or that read a raw
body past its bound, goes to Closed after its answer, not to Idle.

The holder is a call, or a request of its forge's own (7.1): a start
read (3.3, 3.4), a mint (5.1), an installations' read (4.4) or the
puller's (6.5), whose failures go to what asked it. Deadlines are the holder's: every phase but
Closed and Idle waits until it, so no exchange outlives what it serves.
At it the exchange fails `Unseen` if no byte of it was written, else
`Seen`, and the connection is aborted. Which deadline runs is a function
of the phase, set in one place after each transition
(programming-model.md, section 5.4). A closing connection is bounded by
io's close deadline.

### 8.2 A call

Waiting for its forge's start; Resolving a path or a login (4.2, 4.3);
running its plan's steps on its connection; then answered, once.

### 8.3 A forge's start

| State | On |
|---|---|
| Unconnected | a connect: Checking, or Listing for GitHub; one 3.5 refuses: Stopped |
| Checking | the identity read: the configured user: Settings; another user, `401`, `403`, any other `4xx` but `429`: Stopped; unreachable, `429`, `5xx`, malformed: Waiting |
| Settings | the page-cap read: Ready; a `4xx` but `429`: Stopped; unreachable, `429`, `5xx`, malformed, a cap of 0: Waiting |
| Listing | GitHub's installations read: Minting; `401`: Stopped, `KeyRefused`; another `4xx` but `429`: Stopped; unreachable, `429`, `5xx`, malformed: Waiting |
| Minting | each accepted installation's tokens (8.6): Identity, or Ready with none accepted; refused: as 5.1 says |
| Identity | the viewer read: the access's user: Ready; another user, a `4xx` but `429`: Stopped; unreachable, `429`, `5xx`, malformed: Waiting |
| Waiting | the backoff: Checking, or Listing for GitHub |
| Ready | serves calls; a connect: Checking, or Listing for GitHub |
| Stopped | says `stopped` once, naming the forge, and holds no access; a connect: as Unconnected |

A token has only one state, held. An installation's tokens are 8.6's.

### 8.4 A webhook connection

The answers are 6.3's.

| State | Holds | Deadline | On |
|---|---|---|---|
| Head | the head so far | `hook_deadline` | a signed `POST` to a forge's path within `hook_body_bytes`: Body; any other head: its answer, Closing; past the deadline: Closing, no answer |
| Body | the HMAC state, the fields decoded | `hook_deadline` | read and verified: Answering; a wrong signature or a body that does not decode: its answer, Closing |
| Answering | the hint, or none | `hook_deadline`, from its `204` | `204` queued: the hint offered up, held while the owner has no room (2.2); then Head (kept alive) or Closing |
| Closing | nothing | io's close deadline | closed |

A fault or the deadline in any state closes the connection with no
answer and no hint.

Answering has no moment before its `204` is queued that its owner can
hold. With the room the owner leaves each entry point (2.1), the step
that verifies the body queues the `204`, sent from the room skein-http
set aside for its head when the request came, and then offers the hint,
in that entry point (6.2). Answering lasts only while its hint is held, so
its deadline and the owner's close meet it with its `204` queued and its
hint held: then the hint is dropped, never offered. A world proves the
order inside the step, the `204` queued before the hint and one of each,
and crosses Answering's deadline and the close with a held hint.

### 8.5 Ending

- **The close is the owner's last act,** after its domain's drain
  (programming-model.md, section 5.2). By then the domain takes no
  answers.
- **It ends what is open.** Each call still open ends with its
  connection and has no answer: no terminal is sent for it. It drops
  every answer and hint it holds, and every answer the owner returned,
  and offers none again. The next process reads afresh (forge-client.md,
  section 8).
- **It closes what it keeps:** its connections, the listener and each
  webhook connection. **`closed`** goes up once, when none is left and
  io has settled every socket. It is the last event.

### 8.6 An installation's tokens

| State | On |
|---|---|
| Unminted | accepted, not suspended, its forge past Listing: Minting |
| Minting | minted: Current, with a `git token` up; `401`: the forge Stopped, `KeyRefused`; `403` or `404`: the installations read again; another `4xx` but `429`: the forge Stopped, `TokenRefused`; unreachable, `429`, `5xx`, malformed: Waiting |
| Current | due, `renew_before` its expiry: Minting, the old token still sent until it expires; a `401` on it in a call: Minting |
| Waiting | the backoff: Minting; calls wait once the old token expires |
| Gone | deleted, suspended, or no longer accepted: its tokens dropped; accepted again, or unsuspended: Unminted |

Unminted and Gone also run while the forge is Ready: a newly accepted
or unsuspended retained row becomes Unminted. A committed deletion or
suspension, or an accepted-set removal at once, drops its Full token
and both expiries and invalidates its mint, Identity and held git
output. Identical sets, metadata-only reads and renames keep Current
tokens. Eligibility revisions reject an older answer even after loss
and reacceptance; an already-written exchange settles before the own
connection is reused. Events use the newest account.

Ready-time mints, reads and Identity share the reserved own connection,
one exchange at a time. Public Ready and other Current rows survive
private backoff. A mint's transient failure retries its selected scope;
`403`/`404` drop its tokens and wait before a whole installations read,
with no mint from stale metadata. Backoff doubles up to
`start_backoff_max`, and a longer Retry-After wins beyond that maximum.
The wait wakes at its future deadline. Changes and a held git token
share the remaining output reservation; eligibility is checked again
when the token is released.

When Ready was reached without an eligible installation, the first
later Full/Contents sequence runs 3.4's Identity on Full. Until it
succeeds the candidate is ineligible for calls. A matching bot sets
identity_checked without another `connected`; a refusal may emit the
generation's one `stopped` after `connected`. A transient Viewer retry
retains its valid candidate and repeats neither Contents nor its git
event. An expired candidate starts fresh mints, and loss invalidates
the check so another eligible row may supply it. Replacement resets
the check; Close discards all private work and token values.

### 8.7 The delivery log's puller

| State | On |
|---|---|
| Resting | `log_poll`, its forge ready: Reading |
| Reading | a page's rows, each as 6.5 says; the guid kept, `log_catch_up`, `log_pages` or the log's end: Resting, the newest guid kept; a row to read whole: Delivery; `304`: Resting; `401`: the forge Stopped, `KeyRefused`; anything else: Resting, the guid kept as it was |
| Delivery | read and decoded to a hint: Holding; to none, past `delivery_bytes`, or failed: Reading |
| Holding | the hint taken: Reading |

## 9. What its owner owes it

The owner's protocol layer owes the routing of section 2, **the values**
of 3.1 and 7.3 with limits that pass 7.4, the log origins' addresses
resolved, **a decision on `stopped`**, **a decision on `listener lost`**
and **the close after its domain's drain** (8.5). It owes too:

- **a connect** for each forge it holds access for, at start or when
  setup hands it, with access that setup checked and the owner keeps
  (forge-setup.md, sections 4 and 5); a stopped forge waits, not
  connected, for the next one;
- **the accepted installations** of a GitHub forge, after its connect
  and at each change of the owner's set (4.4);
- **git's tokens** kept for git alone, in memory, never logged, and
  dropped as 5.3 says.

The owner's domain owes:

- **at most `calls` calls outstanding,** one live call per token;
- **a locate,** `Read::Locate { owner, name }`, answered
  `Answer::Located { repository }`, addressed with repository 0, its
  answer checked as every other's, a located id never 0;
- **costs,** as the boundary carries them: an answer's `cost` is the
  requests the call wrote, cache fills included, 0 for none, which the
  owner's budget charges (forge-client.md, section 6).

## 10. Verification

- **Step tests:** the start's decision for every state and everything
  heard; the tables' fills, drops and eviction; each limit check, one
  below and at its bound; each listing's page size; the signature check
  against known HMACs, with a header of 63, 64 and 65 digits and one of 64
  uppercase; `Host` for a TLS forge and a named plaintext one, and for an
  IPv4 and an IPv6 address, at the scheme's default port and off it. The
  plans' own are `skein-forge`'s (forge.md, section 11).
- **The machine world:** the component against a scripted HTTP peer over
  in-memory streams, for a plaintext and a TLS forge: every state of
  section 8, with faults at every byte and each deadline passing; a
  start stopped, and asked again with its backoff doubling; a path
  dropped and resolved again; a body past `body_bytes`; an idle
  connection closed for a call to another forge; a held answer offered
  again, and a returned one never; the owner's close in every state,
  crossed with a terminal racing it, `closed` once and last, with no
  spin; access handed at runtime, replaced while calls run and wait,
  and one answering as someone else; for a GitHub forge, on the fake's
  GitHub layer and clock (fake-forge.md): a renewal due while a call
  waits on it, a mint refused each way of 5.1, a call's token chosen by
  its repository's account, a repository with no row found by trying
  the installations in turn, the puller's catch-up, cursor, gap and
  `304`s, and a job log's redirect followed to its configured origin
  alone, another origin said once per connect as a fact.
- **Over TLS, every byte outside the gate.** Each TLS fault is a world of
  its own, with a real handshake that draws the kernel's entropy
  (testing-strategy.md, 4.4). So its flight is its own, bound to its own
  hello, and no two faults can share one. In a debug build, a fault past
  the client's hello costs 3 to 10 ms, about 2.4 ms of it the handshake's
  cryptography. Faults at
  every byte of both handshake flights and of the HTTP start's and first
  call's requests and responses, each with EOF and reset, take 20 to 30
  seconds. The gate's fuzzy suite cuts at the boundaries a reader
  changes state at, each one byte either side, with EOF and reset: each
  TLS record's start and its header's end, found in that fault's own
  flight, and the flight's end; each HTTP line's end, and the end of the
  head and of the body. The every-byte sweep runs unchanged, with the same
  assertions, in the exhaustive suite. The post-landing lane runs it
  after each push to main and reports a failure against that push's
  commit. This departs from testing-strategy.md, section 8, whose gate
  runs both suites in full: the exhaustive suite is a third, run after
  the gate rather than in it.
- **The listener,** against scripted deliveries: a good delivery hinted,
  its `204` queued before its hint goes up; each answer of 6.3; an event
  the dialect does not hint; a hint held while the owner has no room,
  and dropped at its deadline or the owner's close; `hooks` connections
  full; faults at every byte.
- **The protocol world,** in skein's `tests/forge/protocol`: the client and
  the component against the fake forge's protocol layer and domain
  (fake-forge.md, section 4), joined by in-memory streams cut at random and
  driven by a scripted durable owner that restarts (forge-client.md, section
  10). Its stories: a creation whose answer was lost is found after a
  restart and not made twice; listings keep up with another party's changes;
  deliveries, signed or read from the log, reach the owner. Its invariants:
  each call is answered once, however its streams are cut; the requests of a
  forge's own aside, the costs the answers reported equal the requests whose
  first byte the component wrote, which the streams count apart from it
  (4.1), and the fake served those but the ones whose stream was cut before
  their head was whole, which are charged and not served; no credential
  reaches a record, a request line or a fact, the handed, replaced and
  renewed ones too. The fuzzy runs take seeds and stay within 10 seconds (5
  seconds before the user's doubling of 10-22, provisional) (testing.md,
  section 7).
- **The worst case,** against the counting allocator (skein-heap): every
  limit at its maximum, all `calls` outstanding, the tables full, every
  webhook connection holding a hint, and every holder filled: the
  access handed at runtime, an App's key and JWT, each installation's
  tokens and each puller's held hint, metered against `worst_case`.
  The machine world's `full_forgejo_holders_fit_the_declared_worst_case`,
  `full_github_holders_fit_the_declared_worst_case` and
  `full_mixed_holders_fit_the_declared_worst_case` tests measure this per
  kind and with both kinds together.
- **The conformance** drives the component against a real Forgejo
  (fake-forge.md, section 7).

## 11. Open questions

- **A clock off by more than a minute.** A JWT is issued a minute in
  the past, so a wall more than 60 seconds ahead or behind GitHub's
  makes it refused, which reads as `KeyRefused`, and a skewed wall
  shortens or lengthens an installation token's life as 5.1 measures
  it. Default: the host keeps its clock, and the diagnostic says to
  check it. Measuring the life from the mint's `Date` answer instead
  would take skew out of the tokens, not the JWT.
- **Runtime DNS** for a job log's origin. Its address is resolved once,
  at start (4.5), so a host whose address moves is refused until the
  owner restarts. Default: until io resolves names (io.md, section 4).
- **`free` stays 0** until one of a call's reads takes a validator.
  Default: only the log's first page takes one (6.5), and the client's
  reads take none; a read that adds one says so in forge-github.md,
  section 8.5.
- **A listener over TLS,** for a forge on another host, waits for
  skein-tls's server side (tls.md). Default: that forge polls alone
  until then.
