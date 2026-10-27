# Artifact recording blocked on the host runner's Node runtime

The 2026-10-27 attempt at `6f965c8e` ran the pinned Forgejo 17.0.0 and
forgejo-runner 13.2.0 through `scripts/proof-forgejo-conformance.sh`.
The ordinary recording stopped with exit 1 after 93 seconds alone; the
component recording did not run. Its heavy log is
`/home/free/src/rust/tmp/logs/ee11c8c5cf0e4b2dbaccbe249e5d3fba.log`.

These nine request/response pairs are copied byte for byte from that suite's
output. `Recorder::send` scanned and masked them before writing. They are
supporting evidence for the failed attempt; they do not establish populated
artifact-listing, ZIP-download or redirect facts. The existing `facts.txt`
and successful transcript set are retained separately.

The upload action was fetched from
`https://code.forgejo.org/actions/upload-artifact@v3`. The inline command
created `failed.txt`; the next step failed with `Cannot find: node in PATH`.
The runner's private command environment sets `PATH=/usr/bin:/bin`.
`actions-artifact-job-14-log/response` records this failure. Run 8 and job 14
both finished with `status:"failure"`.

The known run's unfiltered, named and absent-name artifact reads answered
HTTP 200 with `[]` and `X-Total-Count: 0`. The unknown-run read answered
HTTP 404. The run's `commit_sha` matches the separately recorded dispatch
head. No artifact id existed to request a ZIP.
