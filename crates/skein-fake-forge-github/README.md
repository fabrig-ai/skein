# skein-fake-forge-github

GitHub's protocol layer over `skein-fake-forge-domain`, at the dialect's
pin. Other REST routes answer `404`; other GraphQL documents answer with
`errors`. The contract is [fake-forge.md, section 10](../../docs/design/fake-forge.md#10-githubs-layer).

A shared `Service` owns App authentication, installations, browser pages,
repository routes and the blob request record. Each bounded `Server`
handles one HTTP connection. `skein-fake-peers::forge::Peer::github(address,
blob, limits, ..)` hosts API/browser routes and signed job logs on two
loopback listeners, sharing the connection limit and io buffers. The peer
learns both bound origins before clients request a log.

CI contexts render as `github-actions` check runs with the domain's run,
job and attempt identities. Reported statuses stay classic statuses; an
empty combined status is pending with zero contexts. Pull documents carry
`mergeable: null` during the domain's seeded head/base settle interval.
The domain fixtures default both settle bounds to two seconds.

A job-log request returns a 302 whose target names the repository, job and
attempt, signed with a seed-derived HMAC-SHA-256 key for 600 seconds. The
blob listener serves the whole plaintext domain log without authentication
and ignores Range. Tampered or expired targets return empty 403 responses;
this shape is inferred because GitHub's blob store is not modelled. Other
paths return 404.

`Config::blob` supplies the standalone blob origin; a peer replaces it with
its bound origin. `third_origin: Some(..)` points the API redirect at an
unfamiliar origin. `blob_redirect: Some(..)` makes the blob listener return
a second 302 to that origin. Origins are plaintext HTTP origins, validated
at admission with enough target and response room for the largest signed
path. The fixture defaults leave both choices unset.

`Limits::blob_records` retains recent requests, evicting the oldest when
full. Each record contains a path bounded by `Limits::http.head` and only
the presence of Authorization and Range. `Service::blob_records` and the
peer's corresponding accessor expose them. Blob record and response Debug output hides signed paths
and credentials. `worst_case` counts records, paths, origins, the signing
key and target scratch; per-connection buffers use the API's envelope.

`Config::tokens` maps invented person secrets to account user numbers. Their
repository reads, reviews and writes use that person's domain permissions and
identity. App routes require the App's JWT. Installation tokens act as the
configured bot, reach only their installation's account and repository
selection, and expire after an hour on the forge's clock. Contents-only tokens
are refused before domain dispatch beyond contents, including GraphQL.
`GET /user` answers a person and refuses installation tokens with `403`;
`DELETE /installation/token` revokes the presented token with `204`.
Person-authenticated installation repository edits require domain admin access.

`Config::faults` reuses `skein_fake_forge_protocol::Faults`. Reset closes before
the next request is read. Drop loses the final domain answer; server-error
replaces it with `500`. Writes remain applied in both cases. The scenario seed
and clock make faults, expiry and generated identities replayable.

`Limits` bounds retained stores, HTTP buffers, document decoding and responses.
`worst_case(&limits, &config)` includes the service and one HTTP connection;
the hosted peer prices its additional connections and shared domain. Dedicated
counting-allocator tests live in `tests/fake-forge/tests/github_memory.rs` and
`tests/fake-peers/tests/github_memory.rs`. They fill the layer's stores while
every domain repository holds a queued pull; the peer test also holds both
connections and measures each iteration. Route and credential matrices,
revocation, byte faults, browser flows and signed logs have focused tests.

The layer displays `Config::required_signatures`. Git pushes go directly to
the domain, whose `Rejected` is the only push refusal. A GitHub `GH013` adapter
and its unrecorded P10 text are deferred. Rates are described separately in the design.
`Config::deliveries` controls the optional numeric loopback endpoint, log
visibility delay (1.5–2.2 seconds), retention (three days), capacity and a
text-payload size for oversize stories. `Limits::deliveries` bounds entries
and encoded payloads. Invalid choices are refused before stores are created.
Without an endpoint, a release records status 502. Worlds feed the forge's
clock; no clock read or socket operation occurs inside the layer.

The App JWT reads `/app/hook/deliveries` and an individual delivery. Pages
honor `per_page`, use opaque cursors, and list visible entries newest first.
The first page has an ETag and supports empty 304 responses. Retention and
capacity remove the oldest entries; redelivery adds a newer entry carrying
the same guid. `revoke_key` refuses subsequent App requests with 401.

GitHub wire documents are encoded through `skein_forge::github::server`.
Payloads carry seeded UUIDs and HMAC-SHA256 signatures. The layer's
`worst_case` includes the bounded log and encoding scratch.

Installation mutations queue bounded lifecycle events, including repository changes
and renames. `Service::poll` releases them on the forge clock; hosts drain
`next_post` for configured endpoints. Domain queue hooks retain their action
and removal reason when created.

`Peer::github` sends configured deliveries as bounded HTTP client exchanges,
records the recipient's status and freezes each terminal in its observations.
Unresolved, refused, timed out and reset endpoints leave status 502. Each
outbound exchange has a thirty-second deadline and uses a numeric loopback address.

`Config::rate_quota` and `rate_window` select token admission. Hosts complete
`Outcome::Admit` against the domain's bounded windows and execute its domain
continuations through `step_admitted`. Local token reads also pay once.
Responses carry the five core rate fields; JWT routes, including conditional
log reads, carry none. Primary refusals report zero remaining; a crowded
window table reports the secondary message and `Retry-After`.

Person tokens charge their user's bucket; installation tokens charge their
installation's separate bucket. Admission resolves the credential once to a
bounded token-store slot or person-table index and retains no credential bytes.
Revocation is admitted before the slot is removed: a rate refusal leaves the
token usable after reset. A token presented on an App route is charged before
its credential-class refusal; App JWT calls stay uncharged.
