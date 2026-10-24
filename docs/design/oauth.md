# OAuth client

Provisional, 2026-10-09, revised 2026-10-24. `skein-oauth` is a bounded,
sans-IO OAuth client for one exchange at a time. It prepares
authorization-code sign-in, refresh and revocation requests, accepts the
resulting HTTP answers, and hands token records to its caller. It knows no
account, provider, token store, browser, or service policy, and discovers
nothing but an issuer's published keys (section 2.2). The caller supplies endpoints, client
registration, clocks, limits, and fresh random bytes. It owns the redirect
listener, HTTP/TLS connection, durability, and the decision to grant a token
after keeping it. skein's driver, `skein-oauth-accounts` (section 6), is that
caller for any service that wants one: it owns those for its owner, so a
service writes no listener, connection or token store of its own.

The client covers refresh; public sign-in with PKCE and a loopback redirect;
and confidential sign-in with a client secret and the web's redirect. Where a
registration asks for them, it also covers a client id the issuer issues at
the first sign-in (section 2.1), an OpenID ID token checked against the
issuer's published keys (section 2.2), and revocation (section 2.3). Its
records may also be access-only, never refreshed: a token obtained
elsewhere, or a sign-in's whose issuer gave no refresh token. Device authorization is outside this component. The same HTTP
records travel through `skein-http` as in `skein-llm`.

## 1. Boundary and entities

The caller supplies a `Registration` with an authorization URL, token
endpoint, client id, redirect URI, requested scope, token wire format, and
optional client secret. The authorization URL and token endpoint are `https`,
or `http` on loopback for an issuer on the same machine, such as a replaying
tier's fake (testing-strategy.md, 4.4). It admits one `Client` machine for an
exchange. The endpoint binding is kept through the exchange; a redirect cannot
choose a new token endpoint. A public client requires an already bound
loopback listener and a redirect URI whose loopback address, port, and path
match that listener. The confidential client has a secret supplied beside its
registration. Its redirect URI is `https`, or `http` on loopback, written as a
public client's is (below: `127.0.0.1`, `[::1]` or `localhost`, an explicit
port that is not zero, and a path), for a caller that serves its sign-in on
its own loopback address; the caller's server receives it, never a listener
of the client's, and its port never varies. The caller routes the browser's redirect to the same in-progress
client and checks the request's actual URI against the bound redirect URI
before giving its query parameters to the machine. A caller using multiple
issuers keeps each sign-in bound to its issuer and redirect; the machine never
infers an issuer from the redirect or token response.

**A registration's additions.** A registration may also carry one optional
value, the issuer's terms; a registration without it is exactly the one
above. The terms are:

- **parameters added to the authorization URL,** each a name of at least
  one byte and a value, which may be empty, sent on every sign-in, or only
  on a new registration (section 2.1);
- **a resource,** sent in the authorization URL, the code exchange and
  every refresh (RFC 8707);
- **an issued client id:** the registration's client id is then only the
  entry point of a new registration, and the issuer issues the client id
  that every later request carries (section 2.1);
- **OpenID:** the issuer's identifier, and its key set's URL, configured or
  found in the issuer's metadata; the client then asks for an ID token and
  checks it (section 2.2);
- **the scopes that must be granted,** among those requested (section 2.2);
- **a revocation endpoint** (section 2.3);
- **the error names that mean sign in again,** each of at least one byte,
  beside `invalid_grant` (section 4);
- **a redirect whose port varies** (below).

Every endpoint the terms name, the metadata's and the key set's among them,
is `https`, or `http` on loopback, as the token endpoint is.

A public client's redirect URI is `http://127.0.0.1:PORT/path`,
`http://[::1]:PORT/path`, or `http://localhost:PORT/path`, the last for an
issuer that registered its public client with that name, as some do. The path
is present, and the port is explicit and not zero, except where it varies
(below). A `localhost` redirect is
bound to 127.0.0.1 and never resolved: the name is a spelling the issuer
requires, not an address to look up. It is compared exactly, byte for byte
with the registered URI, in the registration and in the browser's redirect:
the redirect's authority must be `localhost:PORT` as registered, so
`127.0.0.1:PORT`, `localhost.`, `localhost.example`, another case, another
port or another path is refused before the machine sees the query.

**A redirect whose port varies,** for an issuer that accepts any port on its
loopback redirect (RFC 8252, section 7.3). Only a `127.0.0.1` or `[::1]`
redirect may vary; a `localhost` one never does, since its issuer compares it
byte for byte. Its port is zero, meaning any free port, or a preferred port,
used when it can be bound. Each sign-in is given the port its listener bound
(section 6.5), and that port replaces the registration's for the sign-in: the
authorization URL, the redirect's check and the code exchange all carry the
same URI, and a redirect to any other port is refused. A redirect whose port
does not vary is bound as written, or the sign-in fails. A confidential
client's redirect never varies: its registration refuses a varying port.

`Client` owns only a sign-in or refresh in progress: its registration, private
state value and optional PKCE verifier, one authorization code or prior token
record, deadlines, retry count, and at most one pending HTTP exchange. It
does not own an account table. The caller may create several clients, under
its own admission cap. One accepted exchange emits one terminal `Tokens` or
`Failed`; `Visit` and `Http` are demands, not terminals.

Token and authorization-code bytes are opaque secrets. The saved record is a
versioned encoding of access token, refresh token if it has one, expiry,
caller's opaque record name and generation, and optional bounded claim
metadata; and, each where the exchange gave one, the issued client id, the
granted scopes, the subject, the ID token and the earliest refresh time
(sections 2 to 2.2). It never contains a client secret, state, nonce, code,
or PKCE verifier. The ID token is a secret as the tokens are: it is kept only
to be sent back as a hint (section 2.1), and never logged.
The caller keeps a candidate record durably before lending its access token.
On refresh, an omitted replacement refresh token preserves the previous one,
and generation increases once. A crash before keeping a rotated record may
leave the old refresh token unusable; the caller reports that according to its
own policy.

**The record's version is 3.** The client writes version 3 only, and reads
versions 2 and 3. A version-2 record reads as a version-3 record with none
of the five additions: the registration's own client id, the requested scope
as granted (as a token response without `scope` means, RFC 6749, section
5.1), no subject, no ID token and no earliest refresh time. It is lent and
refreshed as any record, and its refresh is kept as version 3. Under a
registration whose client id is issued, a record without an issued client id
is unreadable instead (section 6.4: a sign-in replaces it), since its refresh
would carry the entry point's client id. Any other version is unreadable.

An **access-only record** has no refresh token. It holds a token obtained
elsewhere and handed in by the caller, such as another program's sign-in that
the caller may read but must not rotate: a shared refresh token, once
rotated, would sign the other program out. A sign-in whose token answer
carries no refresh token, as an OpenID issuer's may for a scope without
offline access (RFC 6749, section 5.1, makes it optional), gives an
access-only record too, holding what the sign-in gave beside it: the issued
client id, the granted scopes, the subject and the ID token. An access-only
record has no refresh state, so no refresh can be asked of it; it is lent
until it expires, and only its source renews it (section 6.3): a sign-in's,
by signing in again.

## 2. Sign-in and refresh

The public flow uses a fresh transaction-specific verifier of 43–128 allowed
ASCII bytes, and `S256`: base64url without padding of SHA-256 over the
verifier. The verifier is supplied by the caller's seeded cryptographic
generator (lib.md, section 8: `CryptoRng`); the machine never draws
entropy. It also takes an independent fresh state
value. The authorization URL carries `response_type=code`, client id,
registered redirect URI, state, requested scope, challenge, and
`code_challenge_method=S256`; and, where the registration's terms ask, its
added parameters, the resource, a nonce and the hints of section 2.1. `Visit`
gives the URL to the caller to show or open. The caller's loopback listener
remains bound while awaiting redirect.

**Each sign-in's own values** come in its `SignIn` event, so that the
machine draws nothing and keeps nothing between attempts: the state, the
verifier and, for OpenID, the nonce, each fresh from the caller's generator;
the redirect with the port its listener bound (section 1); and, when the
caller holds a record of the account, that record's issued client id, ID
token and subject, for signing in again (section 2.1). The registration
holds what every attempt shares: its added parameters and its resource. An
authorization URL that carries an ID token is a secret, as the URL's state
is (section 4).

The confidential flow carries the same state and redirect binding. It uses
its client secret in the token exchange. PKCE with `S256` is enabled when the
registration says the issuer supports it; this is recommended even for
confidential clients. A registration cannot ask for an unsupported challenge
method. The secret never enters the authorization URL. The caller presents
the `Visit` URL through its web sign-in flow.

On redirect, the client compares the returned state in constant time with
the in-progress state, refuses duplicates or missing parameters, and consumes
the code only once. OAuth `error` on a matching redirect is a typed failure.
A wrong state or redirect URI is refused without an HTTP exchange. The
machine sends a bounded POST to the configured token endpoint with
`grant_type=authorization_code`, code, client id, redirect URI, plus verifier
for PKCE and secret for a confidential client. Refresh sends
`grant_type=refresh_token`, client id, prior refresh token, and client secret
when needed. Both carry the resource where the registration has one, and the
issued client id where it is issued (section 2.1). The caller chooses JSON or form encoding for this endpoint;
both encoders measure before allocating. The response is a bounded JSON token
or error document. Unknown extension fields are ignored only within the
document, nesting, token-count, and string bounds. Recognized fields may not
occur twice. The bearer token type is checked before a record is returned.

Refresh receives a complete prior record or refresh state, which only a record
with a refresh token has. It does not start two exchanges for the same record;
the caller serializes each account's refreshes. A response yields a candidate
with an absolute expiry calculated from the injected wall clock, shortened by
any read-only JWT `exp` claim the caller requested. `remaining` is
recalculated when durability completes or when a grant is encoded. A JWT
claim read from an access token is metadata from a token obtained at the
configured endpoint, never proof of authentication. The one token this crate
authenticates is an OpenID ID token, only as section 2.2 says, and it never
uses claims to choose an issuer or account.

A token response's earliest refresh time (`earliest_refresh_at`), where it
gives one, is kept in the record, and no refresh is asked before it (section
6.3). Its granted scopes are the response's `scope`, or the requested scope
where the response has none (RFC 6749, section 5.1); a refresh that names
none keeps the record's.

### 2.1 A client id the issuer issues

A registration whose client id is issued signs in in one of two forms, chosen
by whether the caller holds a record of the account with an issued client id:

- **A new registration,** with no such record: the authorization URL
  carries the registration's client id, the entry point, and the added
  parameters sent on every sign-in and those sent only on a new
  registration (such as a name hint).
- **Signing in again,** with such a record: the URL carries the issued
  client id, the added parameters sent on every sign-in and none of the
  others, and, from the record, `id_token_hint` with its ID token and
  `login_hint` with its subject.

The redirect then carries the issued `client_id` beside `code` and `state`.
The machine reads it from a redirect whose state matched, within the client
id's bound. A matching redirect without it is an incomplete registration: the
sign-in fails as incomplete, and no exchange is made. Signing in again keeps
the client id the redirect gives, which is the record's unless the issuer
issued another. The code exchange, every refresh and the revocation carry the
issued client id, never the entry point, and the record keeps it.

### 2.2 The ID token

A registration with OpenID asks for an ID token: its requested scope includes
`openid`, and its authorization URL carries the sign-in's nonce.

- **The keys come first.** Before the visit, the machine fetches the
  issuer's keys with bounded GETs: the key set at its configured URL, or
  else the issuer's metadata (`<issuer>/.well-known/openid-configuration`)
  and then the key set its `jwks_uri` names. A metadata document whose
  `issuer` differs from the registration's is refused. A sign-in whose keys
  cannot be fetched or read fails before its `Visit`, so a person is never
  sent to an issuer whose answer cannot be checked. The keys are held for
  that sign-in only: sign-ins are rare, and a key set held longer would need
  an expiry of its own.
- **The code exchange's answer must carry an ID token,** a compact JWS,
  checked before `Tokens` is emitted:
  - **its signature** is RS256 (RSASSA-PKCS1-v1_5 with SHA-256) only, by
    the key whose `kid` its header names, or by the key set's one RSA
    signing key where the header names none. Any other `alg`, `none` among
    them, is refused. A key is an RSA JWK (`kty` `RSA`, with `n` and `e`)
    whose `use` is `sig` or absent, of 2048 to 8192 bits; other keys in the
    set are ignored.
  - **its claims:** `iss` equals the registration's issuer; `aud` holds the
    client id the exchange used, and `azp`, where `aud` holds several,
    equals it; `exp` is after the injected wall clock, less the skew the
    limits allow; `nonce` equals the sign-in's, compared in constant time;
    and `sub` is present, within its bound.
  - **its subject,** `sub`, becomes the record's subject, and the token
    itself the record's ID token (section 1). Claims beyond these are not
    read; an email is never a subject.
- **Granted scopes are reported, not enforced here.** A sign-in that was
  granted fewer scopes than the registration requires still returns its
  record, and `Tokens` names the missing ones; the caller decides (section
  6.3). A person who declines part of a grant has still signed in.
- **A refresh's ID token** is not checked, and the record keeps the
  sign-in's (section 7).

The check is the one cryptography this crate does. It runs in `skein-tls`,
over rustls's ring provider, which the workspace already builds for TLS
(tls.md, section 4): the client depends on `skein-tls` for this check alone,
and adds no crate. RSA verification draws no randomness and reads no clock,
so the machine stays deterministic. With `skein-json` and `skein-tls`
beneath it, the client is a protocol composition of machines
(programming-model.md, section 4), as it already is over `skein-json`;
`skein-tls` exposes the check as one function over the key's modulus and
exponent, the signed bytes and the signature, and the client names no
ring or rustls type.

### 2.3 Revocation

A registration with a revocation endpoint can revoke a record's refresh
token (RFC 7009): a bounded form POST with
`token`, `token_type_hint=refresh_token`, the client id (the issued one where
it is issued) and the client secret where there is one. A 200 answer, empty
or not, means revoked. Any other answer, or none within the attempt budget,
leaves it unconfirmed, and the terminal, `Revoked`, says which. The caller
forgets the record either way (section 6.3). An access-only record has no
refresh token and is never revoked by the client.

## 3. Machine and deadlines

`step` consumes one event and writes bounded requests. `resume` emits held
work when output room exists. The owner reserves the published maximum output
before either call. Every deadline is compared with an injected monotonic
`Time`; wall time is used only to calculate saved expiry. The caller bounds
the absolute sign-in time, each HTTP attempt, retry count and backoff ceiling.
No delay extends the absolute deadline. A retry is armed only for a transient
failure and only when the entire attempt budget still fits; its backoff grows
to the configured ceiling. An HTTP outcome includes whether a request was
proved unsent, may have been sent, or received a response. Refresh after an
ambiguous send is allowed only as a caller policy decision because a rotating
refresh token may already be spent.

| State | Holds and waits for | Event → state and output |
|---|---|---|
| Idle | limits and registration | `SignIn` → AwaitRedirect, emit `Visit`, or, with OpenID, AwaitKeys, emit a keys `Http`; `Refresh` or `Revoke` → AwaitHttp, emit `Http` |
| AwaitKeys | the sign-in's values, the metadata if fetched, sign-in deadline | metadata answer → AwaitKeys, emit the key set's `Http`; key set answer → AwaitRedirect, emit `Visit`; unreadable answer, deadline or cancellation → Done, `Failed`, no `Visit`; transient failure → Backoff, as AwaitHttp |
| AwaitRedirect | state, redirect, optional verifier, sign-in deadline | matching `Redirected` → AwaitHttp, emit code-exchange `Http`; OAuth error → Done, `Failed`; wrong state, replay, cancellation, or deadline → Done, `Failed`; `admits` → unchanged, answering whether a redirect would be taken |
| AwaitHttp | exchange and HTTP deadline | successful `Http` → Done, emit `Tokens`, or, for a revocation, `Revoked`; invalid grant, malformed answer or a failed ID-token check → Done, `Failed`; transient answer or transport failure → Backoff or Done; deadline → Backoff or Done; a revocation that fails or runs out of attempts → Done, `Revoked`, unconfirmed |
| Backoff | exchange, retry time, absolute deadline | `Tick` at retry time → AwaitHttp, emit `Http`; absolute deadline or retry budget exhausted → Done, `Failed` |
| Done | terminal already emitted | late redirect, HTTP answer, or tick → ignored; explicit reset → Idle |

`admits` is a question, not an event: it answers whether a redirect, at a
time, would be taken by `Redirected`, by the checks `Redirected` makes before
it ends the sign-in or sends the exchange, in their order: not waiting (any
state but AwaitRedirect, or the deadline reached), another address (the URI
not the one the sign-in bound; a code and an error both; neither; no issued
client id beside a code, where one is issued), or another state (compared
in constant time). A redirect it admits with an error is taken, and ends the
sign-in as `Redirected` does. It changes nothing, so its caller can refuse a
redirect without ending the sign-in (6.5).

`Cancel` in any active state emits one cancellation failure and goes to Done.
An event for a different exchange cannot complete the current one. The HTTP
transport always produces a terminal outcome for each emitted request;
otherwise the injected deadline closes it. The machine does not own socket
settlement. A successful token response consumes the code or refresh attempt;
it is never replayed by the machine. A retry of a sign-in code exchange is
permitted only if the prior request was proved unsent. The caller can start
another sign-in after a failure with a new state and verifier.

An `Http` demand names its method: a POST, or a GET for the issuer's
metadata and keys, which carries no secret and may be retried as any
transient failure is. A `Revoke` takes the record whose refresh token it
revokes; `Revoked` is its terminal, confirmed or not, and only a cancellation
ends it as `Failed`.

## 4. Failure vocabulary and secrets

`SignInAgain` means an authorization code or refresh grant was rejected or
revoked (`invalid_grant`), or the matching redirect returned denial.
`RetryLater` holds a bounded retry delay for rate limits, server failures,
transport failure or deadline exhaustion after the machine's retry budget.
`Malformed` means a bounded peer answer failed syntax, required-field, type,
duplicate-field, or token checks. `InvalidRedirect` and `Limit` identify
local admission failures. Client authentication failure is a separate
`ClientRejected` so a wrong server registration is not presented as a
person's revoked grant. Untrusted descriptions are never placed in a domain
record or log. A status alone does not prove a grant was revoked.

- **The names a registration adds** (section 1) are `SignInAgain` as
  `invalid_grant` is, whatever the status they come with: an issuer's
  terminal refresh errors, such as `invalid_refresh_token`, `token_expired`,
  `refresh_token_expired`, `refresh_token_invalidated` and
  `refresh_token_reused`. Only the error name decides; a status with no
  name decides as above.
- **`Unverified`** names the ID-token check that failed (section 2.2): the
  keys (not fetched, unreadable, or none that fits), the signature, the
  issuer, the audience, the expiry, the nonce, or a missing ID token or
  subject. It is the issuer's fault or an attacker's, never the person's:
  nothing of the sign-in is returned, and its code is spent.
- **`Incomplete`** is a matching redirect without the issued client id
  (section 2.1): no exchange is made, and the person signs in again.

Every type carrying a token, authorization code, verifier, state, nonce, ID
token or client secret omits its bytes from `Debug` and logs. HTTP request bodies and headers
that contain them are treated as secrets by the owner. Authorization URLs
contain state, challenge and nonce, and when signing in again an ID token,
and are handed only to the person's visit flow;
the URL is not a durable record. Error descriptions and JWT claims are
untrusted metadata. Only an authenticated TLS token endpoint, or an
`http` one on loopback (section 1), can supply a usable bearer token.

## 5. Bounds and verification

`Limits` caps authorization URL, endpoint and redirect URI, client id and
secret, scope, state, verifier, code, request body, response document, JSON
depth and tokens, string, access and refresh tokens, error detail, record,
and optional JWT payload bytes. For the issuer's terms it also caps, in
one sum, the added parameters' names and values and the added error names;
and the resource, issued client id, nonce, subject, granted scopes,
ID token, the metadata and key-set documents, the keys a set holds, and an
RSA modulus of at most 8192 bits; and the skew allowed on an ID token's
expiry. Encoders measure the complete escaped URL or
body before allocating. Readers reject overflow before storing. A
registration whose terms hold an empty parameter name or an empty error
name is refused as `Limit`: each entry then holds at least one byte of the
terms' sum, so that sum also bounds how many entries there are. `worst_case`
counts the in-progress machine with its registration (each of the terms'
entries with its own storage, beside its bytes), one request and answer,
parsing temporaries, candidate record and prior record, with checked
arithmetic. The caller adds HTTP/TLS and its own account table and queues.
The maximum output per step is one demand plus one terminal in the case
where an answer completes an
exchange; exact values are exported with the implementation.

Focused tests cover both sign-in types, state and redirect mismatch, duplicate
and late redirect, PKCE challenge and form/JSON encodings, refresh rotation
with and without a new refresh token, a sign-in answered without one and
kept access-only, expiry and skew, refusals, malformed
answers, and every limit boundary. Redirect tests admit `localhost` with an
explicit port and path, and refuse `localhost.evil`, `localhost.`,
`localhost:0`, a missing port, and a redirect whose authority is `127.0.0.1`
for a `localhost` registration; a confidential registration admits each
loopback redirect as a public one does, refuses the same neighbours and
plain `http` to any other host, and keeps its `https` one. Record tests round-trip access-only records. A
separate fake issuer drives sign-in, refresh, revocation, delay, replay, and
transport faults through the real client machine. It also runs as a process on
loopback, as the fake LLM peer does (fake-llm.md, 3). It mints no token: each
token response, to a sign-in or a refresh, is one its script queues, so its
script lists every access token it grants, each refresh's included, before
a world starts. A world that runs it beside the fake LLM peer gives the peer
that list (fake-llm.md, 6). Randomized cases use the fuzzy profile. Memory
high water is checked against `worst_case`.

**The issuer's terms** are tested the same way: a new registration and
signing in again, each sending what section 2.1 says and nothing else; an
incomplete redirect; the resource in the authorization, the exchange and the
refresh; a redirect whose port varies, refusing another port; every ID-token
check failing alone (a key the set does not hold, `alg` `none` and `HS256`,
no `kid` with two keys, another issuer, another audience, an expiry at the
skew's edge, another nonce, a modulus past the bound); a metadata document
naming another issuer; fewer scopes granted than required; a version-2
record read, refreshed and kept as version 3, and refused under an issued
client id; revocation confirmed and unconfirmed; each added error name on
a refresh, with each status; an empty parameter name or error name refused,
and a parameter with an empty value sent; and terms at their most entries
within `worst_case`.

**The fake issuer's additions,** each scriptable and observed:

- **a registration** that issues its client id in the first sign-in's
  redirect, or scripted to leave it out, and checks that each later request
  carries it;
- **the added parameters, `nonce` and `resource`,** read from each request
  and observed by name, so that a test sees which a request carried;
- **an ID token signed with a test key,** with its metadata and key set
  published, and scripted faults in each: another key, another `alg`, a
  wrong claim, an unreadable document;
- **the account that signs in:** its subject, and the claims an identity
  provider such as Google adds, an address (`email`, with
  `email_verified` true or false) and a name, each in the ID token where
  the account has it. A world sets the account for the authorizations
  that follow, and each code keeps the account its authorization had,
  so that one world signs in several people, or one address under two
  subjects; until one is set, the account is the OpenID terms' subject
  alone, with no other claim;
- **granted scopes,** all those asked or fewer, in the token response;
- **revocation,** answered as scripted: confirmed, refused, or never;
- **the added error names,** on a refresh or an exchange, with the status
  the script gives.

The test key is the fake's own, never a secret. The fake signs through
rustls's ring provider, as the client verifies. RSA signing in ring blinds
with randomness it draws from the kernel, outside a world's seed; the
signature's bytes do not depend on it (RSASSA-PKCS1-v1_5 is deterministic),
so neither does a world's trace (section 7).

The protocol choices follow [RFC 7636](https://www.rfc-editor.org/rfc/rfc7636.html),
[RFC 8252](https://www.rfc-editor.org/rfc/rfc8252.html), and
[RFC 9700](https://www.rfc-editor.org/rfc/rfc9700.html); the issuer's terms
follow [OpenID Connect Core 1.0](https://openid.net/specs/openid-connect-core-1_0.html)
for the ID token and its checks,
[RFC 7009](https://www.rfc-editor.org/rfc/rfc7009.html) for revocation and
[RFC 8707](https://www.rfc-editor.org/rfc/rfc8707.html) for the resource. Current
[Gitea OAuth2 documentation](https://docs.gitea.com/development/oauth2-provider/)
describes authorization-code public and confidential registrations, PKCE,
and JSON or form token POSTs. The LLM subscription endpoints are caller
configuration; provider-specific admission remains with those consumers.

## 6. The driver

`skein-oauth-accounts` runs sign-in, refresh, keeping and sign-out end to
end for the protocol layer that owns it, and lends access tokens: a local
host's accounts with an LLM provider, or a service's sign-in of people
through an issuer.

### 6.1 In skein

- **A component of a protocol layer,** as `skein-llm-connection` is
  (llm-connection.md, 1): a step machine with its own state, limits, worst
  case and most outputs per entry point, which its owner routes io's events
  to by the tokens it gave (programming-model.md, 4.2). It depends on lib,
  io's vocabulary, `skein-http`, `skein-tls` and the client of sections 1 to
  5. It knows no service's domain, account names or policy.
- **Its owner** reports its earliest deadline and fires its deadlines where
  it fires its own, and seeds its cryptographic generator (lib.md,
  section 8), from which each sign-in's state, verifier and nonce are drawn
  (programming-model.md, 9).

### 6.2 Accounts

An account is configuration, named by a number. Those given when the
component is made are named by their index, 0 to n − 1, and the owner adds
others while it runs, each under a number of its choosing that no live
account holds (6.3). A name is stable for the account's life: the component
never renames or reuses one on its own. An account has:

- **its registration** (section 1), and the token endpoint's address,
  resolved before the loop (io.md, 4), with its transport: TLS, with the
  roots it trusts (shell.md, 6.2), or plaintext on loopback. Where the
  registration's terms name endpoints on other origins (the metadata, the
  key set, revocation), the account has each origin's address and transport
  too. A key set's URL found in the metadata on an origin the account was
  not given is refused, and the sign-in fails as unverified: the component
  resolves no name while it runs;
- **its source:** sign-in, whose records come from the issuer through this
  component; or handed in, whose access-only records the owner supplies;
- **its keeper** (6.4), and whether its record is shared with other
  processes (6.4).

### 6.3 Requests and events

| From the owner | What it does |
|---|---|
| sign in | starts an account's sign-in, optionally naming the subject it expects; the owner shows the visit |
| redirected | a redirect handed in: the one the owner's web server received, for a confidential client; or the address a person pasted, for a public client's sign-in whose browser is on another machine (6.5); answered `taken`, or refused with the sign-in still waiting |
| return | where a public client's listener sends the browser on once it has taken the redirect: an account's return location, for its sign-in under way (6.5) |
| hand in | an access-only record, for a handed-in account |
| grant | asks for an account's access token, held until released |
| rejected | the provider refused a lent token |
| release | the owner no longer holds the account's grant |
| kept | the answer to a keep, for an account the owner keeps: kept, or not |
| cancel | ends an account's sign-in, refresh or sign-out; its terminal follows |
| add | a new account, under a name and with its configuration (6.2), while the component runs |
| remove | ends an account; `removed` follows |
| sign out | revokes a signed-in account's record and forgets it; `signed out` follows |
| want | marks an account wanted, or not wanted: a wanted account is refreshed though nobody holds its grant |
| locked | the answer to a lock, for an account whose record is shared: the lock is held, with the record read under it; or busy (6.4) |
| close | the owner's close: refuse new requests, drain the rest (6.6) |
| abort | a close that does not wait: cancel every exchange, abort every socket (6.6) |

| To the owner | What it says |
|---|---|
| visit | the authorization URL to show or open, once per sign-in |
| taken | the sign-in took the redirect the owner handed in: `redirected`'s terminal; the sign-in's own follows (6.5) |
| signed in | a sign-in that completed: its record kept (6.4), its grant ready to ask for, its subject, and the required scopes not granted, if any; the sign-in's success terminal |
| granted | an account's access token, its generation and how long it is valid |
| keep | a record to keep durably, for an account the owner keeps (6.4) |
| expiring | an access-only token has reached its lead before expiry, once |
| failed | a sign-in, refresh, sign-out or grant failed: section 4's class, or expired; a sign-in whose record could not be kept fails as not kept, and nothing is lent; an account whose private record did not load fails as unloaded, saying where and why (6.4); a grant of a record without a required scope fails as not granted; a sign-in or a shared record of another subject fails as another subject; a shared record whose lock was not taken fails as busy (6.4) |
| signed out | a sign-out that settled: revoked, or unconfirmed; the record is forgotten either way |
| removed | a removal that settled: the account and its sockets are gone, and its name is free |
| lock | take the shared record's lock, and read the record under it (6.4) |
| unlock | release that lock: once for each `locked` |
| closed | the component has settled: its one terminal |

- **One exchange per account at a time** (section 2). A grant asked for
  while the account's refresh runs is answered by that refresh.
- **Kept before lent.** A new record is kept (6.4) before its token is
  granted. An owner's `kept` that arrives after an abort, or for a keep
  no longer pending, is stale and dropped. One that cannot be kept is not lent, and the previous generation
  stays usable while it is valid (section 1).
- **Refreshed while held.** While a grant is held, the component refreshes
  the account's record its refresh lead before expiry and tells the owner
  the new generation, `granted` again. A grant asked for when the token
  expires within the lead refreshes first. An account whose grant nobody
  holds is not refreshed, unless it is wanted. A `rejected` token refreshes once; a second
  rejection of the same generation fails the grant. No refresh is asked
  before the record's earliest refresh time (section 2); a grant asked for
  meanwhile is answered with the token held while it is valid.
- **A wanted account** is refreshed at its refresh lead before expiry
  whether or not a grant is held, so that an idle account keeps its refresh
  token alive and a run finds a fresh token. With no grant held, the new
  record is kept and nothing is lent; a refresh that fails tells the owner
  `failed`, so that it learns the account needs a sign-in without waiting
  for a run. A wanted account that holds no record, or an access-only one,
  is not refreshed. `want` is not an exchange: it answers only when refused.
- **Granted scopes.** The required scopes a sign-in was not granted are
  named in `signed in`; its record is kept and the sign-in has succeeded. A
  grant of a record that lacks a required scope fails as not granted, and
  nothing is lent, until the person signs in again with them.
- **Another subject.** A sign-in that names the subject it expects, and
  returns a record of another, keeps nothing and fails as another subject:
  signing in again never puts another person's record under an account's
  name.
- **Accounts come and go.** `add` admits an account under a name no live
  account holds, within the accounts limit, or is refused (the name taken,
  or full); an account kept in private files starts loading its record
  (6.4). It answers only when refused. `remove` refuses the account's new
  requests; a sign-in waiting for a person, a grant held or asked, and a
  refresh not yet sent end as cancelled; an exchange already sent, and its
  keep, finish first, so that a rotated refresh token is never lost; then
  its listener and sockets settle, and `removed` follows. The name is then
  free for `add`. Removing an account forgets nothing it kept: its record
  stays with its keeper. The configured accounts are accounts like any
  other, and may be removed.
- **Sign-out** is an exchange, for a signed-in account that holds a
  record. It revokes the record's refresh token where the registration has
  a revocation endpoint (section 2.3), then forgets the record: an account
  kept in private files removes its file (io.md, 5.3), and for one the
  owner keeps, `signed out` tells the owner to drop its own. A held grant
  ends as expired. `signed out` says whether revocation was confirmed: an
  unconfirmed one, or a registration without revocation, leaves the issuer
  holding a grant that the owner tells a person to disconnect at the
  provider. A file that cannot be removed fails the sign-out as not kept,
  after revocation; the account holds no record either way, and a record it
  loads again later has a revoked refresh token, which its first refresh
  finds. A handed-in account is refused (its source is the owner's), and so
  is an account with no record.
- **Access-only records are lent, never refreshed.** A grant of one is
  answered while it is valid. At its lead before expiry the owner hears
  `expiring`, once, so that it can ask a person to renew the token at its
  source (a sign-in's, by signing in again); past expiry, or once its token
  is rejected, a grant fails as expired, until a newer record comes: handed
  in by the owner, or from a sign-in. The component never
  writes to the record's source.

### 6.4 Keeping records

- **The keeper is the owner's choice, per account:**
  - skein's private files (io.md, 5.3): one file per account, replaced
    whole and durable before a grant (io.md, 5.2), and loaded at the start;
  - or the owner's own store, as a service keeps its secrets among its own
    records: the owner hands in the kept record when it makes the
    component, and each new one goes out as `keep`, lent only once the
    owner answers that it is kept.
- **A handed-in record is not kept:** its source is the owner's, which
  hands in a fresh one.
- **Nothing else is kept:** a sign-in's state, nonce, code and verifier,
  the issuer's keys, and the client secret, live only in the exchange.

**Loading a private record.** At the start, an account kept in private
files opens its private directory, making it if absent, then loads its
file (io.md, 5.3). A grant or a sign-in that arrives meanwhile waits for
the load, which ends in one of three ways:

- **A record:** the account holds it, as if the owner had handed it in
  as kept.
- **No file:** the account holds no record. A grant fails as expired,
  as for an owner's store with nothing kept. A sign-in goes ahead, and
  stores its record expecting the name absent.
- **Unloaded,** with where (the directory or the file) and why:
  - **Unreadable:** the file loaded but holds no record this version
    reads, either damaged or of another version (6.2), or, under a
    registration whose client id is issued, without one (section 1). A grant fails as
    unloaded. A sign-in goes ahead and replaces the file, stating the
    digest of the bytes it loaded. This is how a person signs in again
    after a record's format changes.
  - **Refused:** io refused the directory or the file unread, naming
    what it found (a link, the wrong kind, group or other bits, two
    links, another owner).
  - **Failed:** io's error.
  - **Stalled:** no answer within the keeper's file deadline.
  - **Too large:** a file larger than a record can be.

  For these four, a grant and a sign-in both fail as unloaded. The
  sign-in fails before its `visit`, so a person is never sent to an
  issuer for a record that cannot be kept. Nothing is replaced unread,
  and nothing a check refused is overwritten.

**Loading again.** An account that holds no record loads again before
it answers its next grant or sign-in. A refresh that fails as sign in
again drops the account's record, whose refresh token is then dead, so
the account holds none and loads again before its next grant. A person who fixes or removes the
file, or signs in from another process, is then heard without a
restart. An account that holds a record never reloads it on its own: it
owns that record, and a store states the record's digest, so a writer
elsewhere shows up as a conflict, a keep that failed. The one exception is
a shared record, which its owner opts into (below): it is read again under
the owner's lock before each write. Each account runs one load
at a time, and requests that arrive during one wait for it.

**A shared record.** An owner whose several processes share one account's
record, each with a component of its own, marks the account's record
shared (6.2). Each write of it then runs under the owner's lock, and starts
by reading the record again, so that a refresh token another process
rotated is never spent twice:

- **Before each write,** a refresh's exchange, a sign-in's keep and a
  sign-out's revocation, the component emits `lock`. The owner takes its
  lock, as with io's `Lock` on a directory beside the record (io.md, 5),
  and answers `locked`: for an account it keeps, with the record it read
  under the lock, or none; for one kept in private files, the component
  then loads its file again, as above. An owner that cannot take the lock
  in its own time answers busy, and the write fails as busy, a transient
  failure that leaves the account holding what it held; so does a lock
  with no answer within the lock wait (6.7).
- **Under the lock,** the component compares what it read with what it
  holds:
  - **a newer generation of the same subject:** it holds that record and
    makes no exchange; a refresh's grant is answered with its token where
    it is valid past the lead, as `granted`, and refreshed under this lock
    where it is not;
  - **no record:** another process signed out. The account holds none, a
    refresh fails its grant as expired, a sign-out ends as signed out with
    nothing left to revoke, and a sign-in goes ahead;
  - **another subject:** another process signed in as someone else. A
    refresh or sign-out fails as another subject, and the account drops
    what it held and loads again before its next grant; a sign-in goes
    ahead, subject to the subject it expects;
  - **otherwise** it makes its exchange and keeps its record, a sign-in's
    at a generation above the one read.
- **Then `unlock`,** once the write is kept or has failed. Each `locked`
  is answered by exactly one `unlock`, whatever happens meanwhile: a
  cancel, a removal, a close or an abort. A `locked` that arrives for no
  pending lock is answered by `unlock` at once. `closed` ends this: the
  component answers nothing after it, so a lock the owner takes and
  answers `locked` after `closed` is the owner's to release, as it
  releases any lock it holds when the component is gone. With io's
  `Lock`, the owner releases it by closing the root it opened (io.md, 5).
- **Only writes lock.** A grant, a load and lending never wait for the
  lock; a record another process wrote is heard at this account's next
  write.

**Close and abort.** A load is the component's own work for requests it
has admitted, so a close lets it finish within its deadline, and the
waiting requests then end as above. An abort cancels it, and they fail
as cancelled. A `cancel` of a sign-in that waits for a load ends that
sign-in at once. The load goes on for the account.

### 6.5 The redirect

- **A public client's redirect** reaches a loopback listener the component
  binds on the redirect's address and port (127.0.0.1 for `localhost`) only
  while a sign-in waits for it. It reads one request head with skein-http's
  server, within its limits, checks the request's URI exactly against the
  registration (section 1), answers a small fixed page, and closes the
  connection. A request for another path, or past the limits, is answered
  with a refusal and closed, the sign-in still waiting. The listener closes
  with the sign-in's terminal.
- **The return.** The owner may give a sign-in under way a return location
  (`return`), the address the person's browser goes on to once the listener
  has taken the redirect: the page the person left, in an application that
  sent its own tab to `visit`. With one, the listener answers the redirect
  it takes `303 See Other`, its `Location` the return, with
  `Cache-Control: no-store` and `Referrer-Policy: no-referrer`; with none,
  the small fixed page above. It answers every redirect it takes so,
  whatever the redirect says: the provider's error ends the sign-in, which
  the page gone back to reads from its owner, as it reads a success. The listener's answer never says how the
  sign-in ended. A return is the owner's alone: the listener sends the
  browser only where its owner said, never to an address of the request's
  (its Host, its query), and a request it refuses gets no return.
  - **Its bounds:** an absolute `http` or `https` address of at most
    `RETURN_BYTES`, 2048 bytes, of visible ASCII (no space, no control
    byte). One that is not, or one for an account with no sign-in under
    way, is dropped, with no event: a return is a setting, not a request
    with a terminal.
  - **Its life:** it lives as long as its sign-in. A later `return` for
    the same sign-in replaces it; the sign-in's terminal drops it, and a
    new sign-in starts with none. It is never kept: a sign-in its owner starts
    again after a restart has none, and its listener answers the fixed page. A confidential client's redirect reaches the
    owner's own server, which answers it as it likes: `return` changes
    nothing there.
- **A redirect whose port varies** (section 1) binds a fresh listener for
  each sign-in: on port zero, or on its preferred port, and on port zero
  when binding that one fails. The port io's `Listening` reports (io.md,
  3.2) is the sign-in's: the component gives it to the client in `SignIn`,
  before any `visit`. Two sign-ins at once on such a redirect each have a
  port of their own. A redirect whose port does not vary binds only its
  port, and a sign-in that cannot bind it fails before its `visit`.
- **A redirect is taken only when it is the sign-in's,** wherever it comes
  from, the listener or the owner's `redirected`: its address within the
  client's bounds (section 5), of visible ASCII, with no fragment and no
  field the client reads given twice; then whatever the client's `admits`
  refuses (section 3): its address the one the sign-in bound (the port the
  listener bound among it, for a port that varies), a code or the
  provider's error but not both, an issued client id (2.1) beside a code,
  its state the sign-in's, and the sign-in still waiting, its deadline not
  reached. The component judges each, and only then does the listener
  answer: a listener holds no state, so it hands up the redirect it read
  and answers as the component says.

  Any other is refused, and the sign-in still waits: the listener answers
  its refusal page, the same for every reason; `redirected` is answered
  `refused`, as another address (`redirect`), another sign-in's state
  (`state`), or not waiting. A redirect at or past the deadline ends the
  sign-in first, as its tick would, and is refused as not waiting. The
  state is the sign-in's alone, drawn fresh for each, at least 24 random
  bytes, and never logged: a client state bound under 32, which would
  draw fewer, makes the component unusable, as its limits (6.2). A refusal so tells its sender only
  that its state is not this sign-in's. The owner's pastes are bounded as
  well: after `STATE_REFUSALS`, 4, `redirected`s refused for their state,
  a sign-in refuses every later one as full (`Full { bound: 4 }`), and
  still waits for its listener and its deadline.
- **One redirect per sign-in.** Taking one leaves waiting in the step that
  takes it: the listener stops listening, a redirect it has read and not
  yet answered is refused, and so is any after it; a later `redirected` is
  refused as not waiting. A sign-in whose browser reaches the listener and
  whose person pastes its address too makes one exchange, with whichever
  the component judged first. Its deadline is its own from its start: no
  redirect, taken or refused, starts a timer.
- **A pasted redirect,** for a public client's sign-in whose browser is on
  another machine than the listener: the provider sends that browser to the
  redirect on its own loopback, where nothing answers. The person copies
  that address, and the owner hands it in (`redirected`) while the sign-in
  waits, once its `visit` is out. `taken` answers it in the step that
  takes it, and the sign-in's terminal follows as for any redirect; a
  confidential owner's `redirected` is answered `taken` the same way. The
  component checks it as the listener's (above), never fetches it, and
  never reads `localhost` for `127.0.0.1` or the reverse. The listener
  stays up meanwhile, so a browser on the engine's machine, or one through
  a tunnel to its port, still returns as before. Nothing of a paste is
  kept: a sign-in a restart ended is started again.
- **A fixed port, one listener at a time.** A sign-in on a redirect whose
  port does not vary binds once any listener of the component's on that
  port has settled, so that an owner that cancels a sign-in and starts
  another at once, as for a person starting again, gets the port, rather
  than a sign-in failed before its `visit`.
- **A confidential client's redirect** reaches the owner's web server. The
  owner hands it in (`redirected`), and the component checks it the same way.
- **The component never opens a browser:** the owner shows `visit`.

### 6.6 Owner close

The component follows programming-model.md, 5.2.

- **Close drains.** From the owner's close on, it refuses every request
  at its entrance, as closed, never as the request's fault. What is idle
  closes at once: an account with no exchange holds nothing, and no
  refresh starts after the close, as a refresh lead is policy for a live
  service. An exchange in flight with the issuer finishes, its record is
  kept, and then its connection closes. A sign-in that waits for a person is the owner's
  request too, and still waits, within its sign-in deadline: the
  component never cancels its owner's work on a close. An owner that
  will not wait for a person cancels the sign-in first (`cancel`), which
  ends it at once, failed as cancelled, and closes its listener.
- **`closed`** goes up once, when no exchange is left and every socket
  the component made has settled at io.
- **Abort** is a close that does not wait: every exchange ends failed as
  cancelled, and every socket, the listeners' among them, is aborted at
  io. A rotated refresh token whose answer an abort cuts off may be lost,
  as after a crash (section 1). Every shared record's lock still held, or
  answered `locked` before `closed`, is released with `unlock` before
  `closed`; one answered after it is the owner's to release (6.4). An
  abort after a close turns what still closes into aborts, as a
  termination signal does for the process; a close after an abort
  changes nothing.
- **No keep time.** Each exchange opens its own connection and closes it
  when the exchange ends: exchanges are minutes apart, and a pool would
  only add a keep time to close.

### 6.7 Limits and the worst case

| Limit | What it bounds |
|---|---|
| accounts | the accounts at once, configured and added |
| exchanges | sign-ins, refreshes and sign-outs at once, across accounts |
| listeners | loopback listeners at once |
| refresh lead | how long before expiry a held or wanted grant is refreshed, and an access-only one announced |
| lock wait | how long a write waits for the owner's `locked` before it fails as busy: a `Duration` from the `lock` it emits |

The owner gives these limits when it makes the component. The lock wait
is a deadline on the injected monotonic clock (section 3), as each other
deadline is, fired by the owner; once it passes, the write fails as busy,
and a `locked` that comes later is answered by `unlock` at once, until
`closed`, after which it is the owner's to release (6.4).

The client's own limits are section 5's. The worst case is each account's
registration, its terms' entries counted as section 5 counts them, its
current and candidate records, a shared record's read under its lock beside
them, per sign-in its return (`RETURN_BYTES`), and per exchange the client's worst case, the issuer's key set among it,
`skein-http`'s, TLS's and io's buffers for one connection; per listener, one
accepted connection and its request head. The trusted roots are shared
configuration, counted once.

### 6.8 Testing

- **A protocol world** (testing-strategy.md, 2.5): the component, its
  owner's routing, the fake issuer (section 5), and a scripted browser that
  follows each visit to the listener, over streams cut at random. Sign-in of
  both clients, refresh while held, rotation, a rejected token, a keep that
  fails, an access-only record expiring and handed in again, a `localhost`
  redirect, wrong redirects, a public sign-in's pasted redirect (taken; a
  paste and the listener's redirect in both orders, one exchange; a second
  paste; another host, port or path, a fragment, a field twice, past its
  bounds, a space or a control byte, a code and an error both, without a
  code or an issued client id, each refused with the sign-in still
  waiting; another sign-in's state refused, and after `STATE_REFUSALS`
  every paste refused as full, the listener still taking its own; a paste
  before the `visit`, at the deadline (the sign-in timed out), after the
  cancel or the terminal, refused as not waiting; a paste judged between
  the listener's read and its answer), the listener's redirect with
  another state refused with the sign-in still waiting; a cancelled
  sign-in on a fixed port and a new one started in the same step, which
  binds once the first's listener settles; a state bound under 32,
  unusable; a sign-in's return (a `303` to it; the fixed
  page without one; one given for no sign-in, or after its terminal,
  dropped; one past its bounds dropped), and the owner's close in every state, a
  sign-in waiting for a person among them, cancelled by the owner or
  still waiting; an abort after a close, and a close after an abort; one
  terminal per request, and `closed` once and last.
- **A simulated world** (testing-strategy.md, 2.7), with io over the
  simulator, the fake issuer as a simulated server, and private files on the
  minimal fake machine (testing.md, 4): a filesystem that fails or stalls a
  keep, and a private directory or file with the wrong mode. Every world
  ends under the teardown invariant (testing-strategy.md, 6).
- **The additions,** in both worlds: a new registration and signing in
  again, with the issuer's keys fetched before each visit; a redirect whose
  port varies, its preferred port taken; required scopes declined, then a
  grant refused as not granted; another subject; accounts added and
  removed in every state, a name reused after `removed`; a wanted account
  refreshed with no grant held, and its refresh failing; sign-out
  confirmed, unconfirmed and without revocation, for both keepers; a shared
  record under a scripted owner lock that is busy, slow, or answered after
  a cancel, a removal or an abort, finding a newer generation, no record,
  or another subject; exactly one `unlock` for each `locked` before
  `closed`, and none after it, a `locked` answered past `closed` left to
  the owner.
- **Memory:** every limit at once, against the worst case.
- **Transports:** the replaying tiers run plaintext issuers on loopback; TLS
  to an issuer runs in the real loop (tls.md).
- **For an application's real loop** (testing-strategy.md, 2.8), the fake
  issuer runs in its process, on loopback, with no added latency, for the
  stories that sign a person in: the test drives the redirect, following
  each `visit` to the listener as the scripted browser does. Stories that
  need an account but no sign-in give it a key instead (llm.md, 2.6), and
  stories of a program that signs in by itself use a fake program; neither
  needs the issuer.

## 7. Open questions

- **A `localhost` redirect and IPv6.** The listener binds 127.0.0.1 only. A
  browser that resolves `localhost` to `::1` and does not fall back would
  find nothing there; whether to bind `[::1]` beside it, for the same
  redirect.
- **Device authorization,** for a person whose browser is on another
  machine, beside the pasted redirect (6.5): whether it joins the client.
- **A credential helper,** a program that prints a token, as another source
  of access-only records beside handing them in.
- **The fake issuer's signing in a simulated world.** ring's RSA signing
  blinds with randomness from the kernel, which no world seeds, and ring
  takes no generator from its caller. The signature's bytes do not depend
  on it. Default: the fake signs as it answers; if a world must own every
  draw, the fake signs its ID tokens once, before the world starts.
- **A refresh's ID token.** Default: not checked, and the record keeps the
  sign-in's. Checking it would fetch the issuer's keys for refreshes too.
- **What `login_hint` carries.** Default: the record's subject. An issuer
  that expects another claim would need the registration to say which.
- **How `earliest_refresh_at` is written.** Default: seconds since the
  epoch, as a JWT's `exp`; a value the client cannot read is ignored.
- **A wanted account's interval.** Default: its refresh lead, as for a held
  grant, so an issuer whose access tokens last an hour sees a refresh an
  hour; whether a longer interval keeps a refresh token alive as well.
