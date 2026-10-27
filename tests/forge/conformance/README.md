# Throwaway Forgejo conformance

The Rust tool starts only the pinned Forgejo binary named by
`--forgejo`, with fresh SQLite state on loopback. It creates the
fixture and removes its instance and scratch directory when it ends.
`VERSION` and `RUNNER_VERSION` in
`skein_throwaway_forgejo` pin both binaries.

```sh
heavy --gate scripts/proof-forgejo-conformance.sh \
  --forgejo <absolute path to the pinned Forgejo> \
  --runner <absolute path to the pinned runner> \
  --scratch target/forgejo-conformance --out target/conformance-recording
```

`heavy --gate` requires the candidate to stay clean. Record into ignored
`target/` storage, then copy `facts.txt`, `transcripts/` and `deliveries/`
from that output into this directory after the receipt succeeds.

Scratch must be empty or absent, outside `/tmp`. The binary's first version
line and SHA-256 are kept in `transcripts/binary.txt`.

Each case is `transcripts/<case>/{request,response}`: raw HTTP/1.1 bytes.
Authorization is `token MASKED` or `Basic MASKED`. Responses are unframed, with chunking
removed and Content-Length recomputed. JSON key order, whitespace and types
remain as received. Cases replace their own directory only.

The setup group records admin user and token routes under the minimum
admin scope set. It registers newly made token values and their eight-digit
tails before masking, and checks every kept capture for those secrets.
The component facts run under `read:user`, `write:issue`, and `write:repository`.

Maps mask object ids, UUIDs and RFC3339 times by first appearance across the
run, preserving id width and time offset. Quoted and unquoted timestamps share
the same time map keyed by the original timestamp without quotes. Quoted
fractions are dropped; unquoted log fractions retain their width with zero
digits. Numeric resource ids and unrelated text stay unchanged. Ports become 3000; Date and
Last-Modified become 2020-01-01. Numeric resource ids remain stable because
provisioning has a fixed order. Every output is checked for all fixture
secrets before it is written, and response bodies are also checked before
masking. A leak reports the case and secret kind without printing its value.

The same run writes `facts.txt`, with Forgejo and runner provenance followed
by `holds`, `fails` or `unchecked` lines. Pass `--runner` naming the
pinned runner for Actions facts and their transcripts.
The runner connects through its private token file. The base Actions fixture
executes two inline host shell jobs, one succeeding and one failing. The artifact
fixture uses a host-shell `run:` step and `curl` to speak the Actions runtime
artifact protocol used by `upload-artifact@v3`; it fetches no action. Without a runner those facts remain unchecked. Webhook deliveries are recorded
in `deliveries/<event-type>/{request,response}` with their own mask map;
the listener holds its answer while status and mergeability probes run.

`--component` drives the forge component through real io sockets on the shell's
ring against a fresh fixture. It replaces only the `component-` facts in
`facts.txt`; it records no transcripts. The same runner flag exercises
Checks and Job through the component; without it those two facts are unchecked.
Start requests are counted separately from logical calls. Every call's cost
is compared with the HTTP request lines counted from socket sends.

The `cli-` facts check migration, users, scoped tokens and dumps through Forgejo’s command line, on a fresh work path and the running fixture. They record only outcomes, never command output or credentials.

The `runner-` facts check secret-file registration, the deprecated offline file command, and the runner’s connection through its token file. A final workflow checks cloning a private repository with the job’s token. No registration file is used by the running runner.

Response unit arrays are sorted. Git pkt-line length prefixes stay intact while
their payloads are masked. Unquoted runner log times use the same canonical
2020 date and first-appearance ordinal as quoted timestamps. Delivery CI requires `--runner`;
recording refuses it when the flag is absent.

`--admin-scopes` records just the setup routes' least-scope grants and
403 refusals on a fresh loopback instance, without Actions or deliveries:

```sh
heavy cargo run -p forgejo-conformance -- --admin-scopes \
  --forgejo <absolute path to Forgejo 17.0.0> \
  --scratch target/admin-scopes --out <output directory>
```

The missing-scope token carries unrelated `read:issue`: Forgejo refuses
empty scope lists. The settings route is also checked anonymously. The
normal setup group records these cases too.

`--create-repository` records repository and contents creation alone, including
README initialization, Actions' default, repeated creation and raw reads:

```sh
heavy cargo run -p forgejo-conformance -- --create-repository \
  --forgejo <absolute path to Forgejo 17.0.0> \
  --scratch target/create-repository --out <output directory>
```

The two focused recording modes write their own facts only. When refreshing the
shared captures, merge those named facts into `facts.txt`, retaining its binary
provenance and the other groups. Both use the same pinned throwaway fixture and
secret masking as the full recording.

The turn group records one named transcript per fact, with supporting captures
for branch tips, tree pages and rewritten commit parents. Its isolated `turn`
repository has two dispatch-only workflows to test both run filters, three
runs across a page, and a branch moved during a dispatched job. Each workflow
has two inline host-shell jobs: one passes and one fails in a workflow step.
One input is echoed and read through the passing job's log. The group also
records rerun attempts, fast-forward-only merges, rebase updates, status words
and links, and root/directory tree paging in git order. A public bare clone of
only the throwaway repository supplies an independent root-tree id. Without
`--runner`, this group's facts are unchecked.

The artifact workflow writes `failed.txt` and uploads it under two names, `tests`
and `other`. Its scanned captures include the dispatch, run, jobs and job log;
all artifacts, the `tests` filter, an absent name and an unknown run; and the ZIP
download, including a redirect target when it stays on the fixture origin.
The run read records the dispatch commit independently of the artifact listing.
The upload creates an `actions_storage` container through `ACTIONS_RUNTIME_URL`,
uploads the file with `Content-Range`, then finalizes its size. The upload
also supplies `x-actions-results-md5`, the chunk's binary MD5 encoded as base64,
as Forgejo 17's v3 handler requires. The item path encodes its slash as `%2F`.
Authentication uses the step's `ACTIONS_RUNTIME_TOKEN`; captured curl diagnostics redact it.
An upload failure stops recording with those supporting captures retained.

`artifact-curl-attempt/` keeps the failed runtime-protocol upload: container
creation returned 200, but Forgejo refused the first file chunk with 500,
`Error save upload chunk`. It contains no populated artifact or archive facts.
