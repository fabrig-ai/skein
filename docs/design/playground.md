# A world in a browser

Provisional, 2026-10-26. skein's kit for running an application's
simulated world whole in a visitor's browser, with nothing installed and
no server: the world in a Web Worker, stepped in slices paced by the
browser's clock; the application's browser shell on the page, its
transport carried to the world over a bridge; a visitor in the world
speaking HTTP to the application's listener; and a session log that
replays a visit natively. The world is world.md's; the page's parts are
ui-web.md's, the worker's platform among them (section 18). temper's
playground is the first user (temper's `docs/design/playground.md`).

## 1. In one page

- **Two modules, one origin, static files.** The page runs the
  application's browser shell on skein's parts as it ships, but for its
  platform; a Web Worker runs the application's world as a second
  module. No threads and no shared memory, so the page needs no
  cross-origin isolation. Two modules, because the world then steps
  without holding the page's turns, and a reset is the worker ended and
  started again.
- **The bridge carries the transport's plain data,** one message per
  item, as `skein-ui-web`'s fetch takes and gives it: exchanges, the
  stream opened, read on and closed, abandons; their answers, the
  stream's pieces and end; the lines the world prints; and the world's
  clock with each message (section 5). On the page it is a `Platform`,
  the worker's (ui-web.md, 18), so an application's shell changes the
  type of its platform and nothing else.
- **The visitor is a program of the world:** for each exchange, and for
  the stream, it opens a simulated connection to the application's
  listener and speaks HTTP with skein's client, sending the Host the
  application names and the cookies its jar keeps, as a browser does.
  The page never holds a credential (section 4).
- **Stepped in slices, paced by the browser:** the worker's loop steps
  the world while it is busy, up to a slice's budget, and moves its
  clock toward the browser's elapsed time times the speed, 0 while
  paused; idle with nothing due, the worker sleeps until the next due
  instant or the next message (section 3).
- **One clock:** the page's platform gives the application the world's
  clock, never back, and moving between the world's reports only while
  the world waits idle (ui-web.md, 18).
- **Reproducible:** each input enters the world between steps, stamped
  with its step and its instant; the page keeps the session's log, and
  the harness replays a log natively as a test (section 6).
- **Fail-stop, no reload:** a panic in either module ends the visit; the
  page says so and offers the log (section 7).
- **One bundle:** skein's bundler puts the worker's module and its glue
  beside the page's, under one digest directory and one policy (section
  7).

## 2. In skein

| Crate | Where | Holds |
|---|---|---|
| `skein-playground` | `crates/` | the visitor, the slices' loop over a world, the session log's replay feed; on wasm32, the worker's parts: its messages in, its timer and its posts |
| `skein-ui-web` | `crates/` | gains the bridge's messages and the session log's format and keeping, portable, and the worker's platform for the page (ui-web.md, 18) |
| `skein-ui-bundle` | `crates/` | gains a bundle's worker module (section 7) |

- **`skein-playground` depends on** lib, io, http, the simulated world
  (world.md) and `skein-ui-web`'s portable half for the bridge's
  messages; on wasm32, the glue. It is ordinary Rust, as a shell is
  (programming-model.md, 10.2), and builds natively, where the harness
  replays a log, and for wasm32, where a worker runs it.
- **The application's own:** its world's composition, its documents,
  its worker's start (the module's entry, which makes its world and
  hands it to the loop) and the page's frame, the controls around its
  client. skein ships the parts; it holds no application.

## 3. The slices

The worker's loop, `skein-playground`'s, over a world (world.md) whose
processes include the visitor:

```
on a message from the page:
  pace (speed, pause): kept; the timer set again
  an input for the visitor: queued
on the timer, or after a message:
  each queued input enters the visitor, stamped (step, instant); the page is told
  while the world is busy and the slice's budget lasts: a step
  idle: target ← the clock + browser time since the last slice × speed
    the next due instant at or before the target: the clock moved to it, then steps
    otherwise: the clock moved to the target, alone (world.md, 4)
  the visitor's outputs and the lines printed, posted, each with the clock
  the timer: busy, at once, whatever the speed; idle, the browser instant
    at which the next due instant comes at this speed, and none while
    paused or with nothing due
```

- **A slice's budget is steps,** so its work is bounded and a page's
  message waits at most one slice. Its size is the application's, and
  its browser tier measures a slice's latency against its bar.
- **No catch-up.** Time the world could not keep up with is dropped: the
  world runs slower than asked, and never jumps past what it did not
  step.
- **Paused, it is still:** speed 0 moves no clock, but a busy world
  still steps, and inputs still enter and are answered at the paused
  instant, so a paused world still answers what does not wait on time.
- **The pace changes when, never what** (world.md, 4): a world moved in
  pieces is the world moved at once, so the log ignores the pace.
- **No verdicts:** no step cap and no idle failure (world.md, 6). A world
  that settles, every process ended, has nothing more to step, and the
  page says the world stopped.

## 4. The visitor

- **A `Host` of the world,** a step machine over skein's HTTP client on
  io's sockets, which an application's composition hosts as one of its
  processes. Its inputs come from the loop between steps (world.md, 5);
  its outputs go back to the loop after each slice.
- **Its inputs** are fetch's (ui-web.md, section 10): an exchange (its
  id, a path, the method, headers and body, a bound on its answer's
  body); an abandon of one; the stream opened (a path, its headers, a
  bound on a piece), read on, or closed.
- **An exchange** is a connection of its own to the listener, one
  request, its answer read up to the exchange's bound and past it
  failed, then the connection closed; abandoned, the connection is
  closed and nothing is reported. **The stream** is one connection, its
  body read one piece per read-on, each at most the piece's bound, so
  the page paces it as fetch does. A refused connection or a reset is
  the exchange failed, or the stream ended.
- **What it adds, as a same-origin fetch would:** the Host the
  application names, its deployment's address for its clients'
  listener, and an `Origin` of the same; each cookie its jar holds whose
  path the request's is under.
- **The jar** keeps the cookies the answers set, by name, with their
  path and their expiry against the world's wall, and drops one cleared
  (`Max-Age=0`) or expired. It holds a few, the application's bound, and
  refuses one past it, as a full jar. The page never sees a cookie, so
  an HTTP-only one keeps its promise.
- **Its outputs** are fetch's reports: an exchange answered (its status
  and body) or failed, the stream opened, refused, a piece, its end.
- **Bounded:** the exchanges in flight, the page's fetch bound; one
  stream; the jar; bodies by their bounds. Its worst case is its own
  (programming-model.md, 6).

## 5. The bridge

- **Messages, the page's:** a visit's start (the application's story,
  and the seed and the start the page drew: section 6); an exchange; an
  abandon; the stream opened, read on, closed; and the pace, a speed
  with 0 for paused.
- **Messages, the world's:** a fetch report; a line a process printed,
  with its stream (standard output or error) and its process's name;
  an input's stamp; and the world's clock, its step, monotonic instant
  and wall, whether it waits idle and its next due instant, the
  worker's memory (its module's linear memory, which only grows, so its
  last size is its high water) and its last slice's duration in browser
  time, with each of them and after each slice; the world stopped; and
  the world failed, with its panic's words. Natively, the memory and
  the duration read 0. The page keeps the high water of each, so the
  world says how big and how quick it is, which the application may
  show and its browser tier reads as a person does (section 8).
- **Plain data, encoded as bytes,** one message per `postMessage`, its
  buffer transferred. Both modules come from one bundle (section 7), so
  the encoding has no version and no negotiation: a page and a worker of
  different builds cannot meet.
- **Its types are `skein-ui-web`'s,** in its portable half (the
  exchange, the stream's duty, the fetch report), so the page and the
  worker share them and both build natively for their tests.
- **Natively, the bridge is in memory:** `skein-playground` gives a
  `Platform` that hands the transport's duties to the slices' loop in
  the same thread, as the same messages, and raises the loop's reports
  as the worker's platform does. So an application's test drives its
  client's host, built natively, against its world, through the
  visitor, as its page does in a browser, with no worker and no
  browser.

## 6. The session log

- **The world is a function of** its build, its seed, its start (the
  wall at its start) and each input with its stamp, its step and its
  instant (world.md, 5). The page draws the seed and reads the wall at
  a visit's start and gives both to the worker in the start message.
- **The page keeps the log,** in its memory, not the worker's, so a
  worker's panic leaves it whole: each input it sent, with the stamp the
  worker answered, and a digest of what the world said between two
  inputs: its fetch reports in their order, the lines it printed in
  theirs, folded apart, and its stop, the three hashed together; never
  its clock, whose messages depend on the pace. A slice enters its
  inputs before it steps and posts after, so what the page folds
  between two stamps is what the world made between their steps,
  whatever the slices; but how a slice interleaves reports and lines
  depends on where it ends, so their interleaving is not checked, only
  each stream's order. Bounded, the application's
  parameter: past it, the log stops and says it was cut, and a replay
  runs to the cut.
- **Offered as a file** on a failure, or by a key the page's frame
  gives: never sent anywhere.
- **Its format,** a stored one: a first line naming the format and its
  version, the bundle's digest, the seed and the start; then a line per
  input, its stamp, the input, and the digest of what the world said
  since the one before, each input in a text form of its own (its
  fields; bodies in base64, lib.md, 6.1), never the bridge's unversioned
  encoding, so the stored format stands alone. A log of another format
  version is refused, naming it. The build it names is the bundle's,
  which a native replay has not: a replay of another build goes on, the
  digests deciding, and a failure names the build the log came from.
- **Replayed natively,** a test: the harness's run (world.md, 3) over
  the application's composition built from the same documents, the
  log's seed and start, each input entering at its stamp, and each
  digest checked against what the replayed world posted. The first
  that differs fails the replay at that input. The application's
  referee runs as in any world, so a bug a visitor met is in the lowest
  world from the start.
- **What changes a log:** a change that makes a kept log's replay
  differ is a new log, recorded again, or a finding (temper's
  `docs/design/playground.md`, testing).

## 7. The page, the worker and the bundle

- **The worker's module** is the application's world built for
  `wasm32-unknown-unknown`, with its glue: its start makes the world from
  the start message's story, seed and start, then runs the slices. Its
  parts, `skein-playground`'s on wasm32: the worker's messages in, as
  reports to its loop; one timer; its posts.
- **The bundle** gains a worker module: the bundler takes it beside the
  page's module and writes its glue and its module into the same digest
  directory, so the page starts it by its address under the directory
  (`new Worker(…, {type: "module"})`). The page's policy adds
  `worker-src 'self'`, and nothing is fetched after the load: the
  documents the world needs are in its module.
- **A panic is fail-stop:** the worker's hook posts its words before the
  module traps; the page's hook stops both. The page then says the
  playground failed, offers the log and a start over, and never reloads
  by itself, as skein's restart part would (ui-web.md, 5): a new visit
  is a new world, and the log is the evidence.
- **A reload, a reset or a new story** ends the worker and starts a
  fresh one: nothing is saved.

## 8. Testing

- **The visitor,** in `tests/playground` (skein-playground-world): a
  simulated world of the visitor and skein's HTTP server, scripted:
  every exchange answered, refused past its bound, failed on a refused
  connection, abandoned; the stream read one piece per read-on; the
  jar set, sent under its path, cleared and expired; the Host and
  `Origin`; its worst case against the counting allocator.
- **The slices:** a world paced at speeds and pauses drawn from the seed
  ends as the same world run whole; an input entered at a stamp replays
  at it; a paused world answers an input at its paused instant.
- **The log:** written and read back; refused for another build or
  version; a replay that differs fails at its first input.
- **The worker's platform:** its pure half (the clock, the timer in the
  world's time, the messages' encoding) in `skein-ui-web`'s step tests.
- **In a browser:** the application's browser tier walks its playground
  in Chromium, reading the world's memory and slices from what the page
  shows, since the browser kit reaches no worker, (temper's `docs/design/playground.md`); skein's own
  example in a playground waits (section 9).
- **For the portable target:** clippy over `skein-playground` for
  wasm32 in the gate (testing.md, 10).

## 9. Open questions

- **skein's own example in a playground,** so that the bridge's page
  half is proved in skein's browser tier before an application's:
  waits for a second application, or for a bug that shows only there.
- **Saving across a reload:** later, the application's.
