# Runtime protocol upload refused by the pinned Forgejo

The 2026-10-27 attempt at `bb35b8ca` used a host-shell `run:` step with
`curl` and the Actions runtime artifact protocol used by `upload-artifact@v3`.
It fetched no action and used no Node runtime. The pinned Forgejo 17.0.0
and forgejo-runner 13.2.0 ran through `scripts/proof-forgejo-conformance.sh`.

The step received `ACTIONS_RUNTIME_URL`, `ACTIONS_RUNTIME_TOKEN` and
`GITHUB_RUN_ID`. Creating the `tests` container returned HTTP 200 with
`fileContainerResourceUrl`. The first PUT to that returned URL, with
`itemPath=tests/failed.txt`, `Content-Range: bytes 0-3/4`,
`x-tfs-filelength: 4`, and the four-byte file, returned HTTP 500:

```text
Error save upload chunk
```

The step stopped before size finalization or the second artifact. Run 8
and job 14 finished with status `failure`. The named, absent-name and
unfiltered listings answered HTTP 200 with `[]`; the unknown run answered
404. No ZIP request or redirect could be recorded.

These nine request/response pairs are copied byte for byte from the suite's
scanned and masked output. The job-log response contains the curl request
and response headers and the refused upload's body. The workflow redacts
its runtime token in diagnostics before the recorder reads the job log;
the synthetic script test checks redaction in headers and response bodies.

The ordinary recording exited 1 after 98 seconds in the gate lane
(95 seconds alone); the component recording did not run. Its log is
`/home/free/src/rust/tmp/logs/ee11375a494a4e169b1fb6128778c21b.log`.
This failed attempt establishes no populated-listing or archive facts;
the successful facts/transcripts are retained separately.

The K1e diagnostic reproduction retained `server.log` before fixture cleanup.
Its final line names the cause: `Error save upload chunk: md5 not match`.
Forgejo 17's `routers/api/actions/artifacts_chunks.go`, `saveUploadChunkBase`,
saves the chunk before comparing `x-actions-results-md5` with the base64-encoded
binary MD5. Thus the chunk storage was writable; the curl request omitted this
required checksum header. The probe supplies it and percent-encodes the item
path separator in its corrected request. The diagnostic heavy log is
`/home/free/src/rust/tmp/logs/03f9df9f83ad47718f5dc9b952f91a27.log`.
