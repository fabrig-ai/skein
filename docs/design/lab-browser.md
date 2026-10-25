# The lab's browser

Provisional, 2026-10-22. The lab's one program over skein's browser kit
(browser.md), and the look's capture that drives it: a product's pages
captured beside their mockups, as three views a check and a judge read.
temper's draft `docs/design/draft/qa-web.md` is its first user: its
scenes (section 3 there), their three views (section 4) and their
judging (section 5). The lab's records, runner and checks are
`lab.md`'s; this document says what the browser adds to them. The hand,
a driver's browser in a cluster (qa-web.md, section 6), is this same
program in a second form (section 8).

## 1. In one page

- **One program on the kit:** `skein-lab-browser`, the lab's browser.
  It runs headless Chromium as its child through `skein-browser`, from
  one loop, and answers calls. The capture uses it now, and the hand
  later, so that both find what they find as a test's person does: by
  role and accessible name, with the kit's bounds and its trouble
  (section 2).
- **Calls, not scripts:** one canonical JSON line in, one out, on its
  standard input and output. A call opens a page at a size, goes to an
  address, awaits a part, reads the page's tree, a part's style or a
  shot. Nothing evaluates script in a page (section 3).
- **The look's reads are the program's alone.** Its style, tree,
  selector, shot and emulation calls are the kit's look reads
  (browser.md, 5.1), which no test sends: a test sees what a person sees.
- **It serves what it captures:** each directory a call names, read
  whole, on a loopback origin of its own, so that each side keeps its
  origin's rules (2.3).
- **A closed network:** Chromium reaches the program's own origins and
  nothing else. Every other request goes to a proxy the program holds,
  which refuses it and records what was asked (2.4).
- **The lab's fonts:** Chromium sees only the fonts of one directory
  the lab names, through a font configuration of its own, so that a
  shot depends on files the key names, not on the machine (2.5).
- **The capture** is Python's standard library in `scripts/lab/look/`,
  under the runner's "for now" (lab.md, 10.1): a scene file in, the
  program's calls, a record and three views out (sections 4 and 5).
- **Three views of each scene at each width:** its structure, an
  outline of each side's tree, diffed as text; its style, a table of
  the paired regions' computed values and boxes; its picture, the two
  shots side by side, each region cropped from both at twice the scale,
  and a difference image with its PSNR where a region's content matches
  (5.3).
- **Three checks hold a capture to its scene:** `look-structure` (the
  outlines' entries, the regions' order on the screen, readiness and
  trouble), `look-style` (the regions' values) and `look-words` (the
  words of the regions a scene names, its data cut by its own
  declaration), each against the
  mockup as captured, overridden by the scene's expectations and
  excused by its departures, which name what they accept and why
  (5.4). They read only the scene file and the capture's record.
- **The hand,** the program's second form: one person's page on a
  product's origin in its cluster, a person's calls on a loopback
  listener, finds and acts as the fake person's through the face, the
  sign-in link redacted, the page's trouble logged; and `browse`, the
  tool that sends them (section 8).
- **Live scenes read the hand's page:** a scene whose page is live is
  captured from the hand's own page as the seed left it, through the
  look's reads the hand answers to the lab alone (8.10), its viewport
  changed for each width and given back; the mockup's side as ever, in
  the capture's own program, with the same Chromium and switches
  (5.1). The look's checks hold it as any scene, and where its focus
  is too (5.4).
- **Tested where each part lives:** the kit's reads in the kit's own
  tests, the program by one story on real Chromium in skein's browser
  suite, the capture's decisions in the runner's unit tier and its
  whole run in the runner's process tier (section 6).

## 2. The program

### 2.1 Its place and its rules

- **`testing/skein-lab-browser`,** a binary crate in skein's workspace,
  named in no product's dependencies. It builds on `skein-browser`, and
  on `skein-fake-person-browser` for its origins' server (ui-web.md,
  14.2) and, with the hand, its face.
- **Ordinary Rust,** as the face's harness is (ui-web.md, 14.2): one
  thread, one loop, skein's real loop under it, Chromium and the
  origins its children and listeners; the test crates' rules
  (`clippy.toml`) and no others.
- **Generic:** it names no product, no page and no path. What it opens
  and reads is its caller's.

### 2.2 Its command line and its life

The capture's form; the hand's is 8.1's.

```
skein-lab-browser --chromium PATH --fonts DIR --scratch DIR --out DIR
```

- **`--chromium`:** the browser's program, by absolute path. The lab
  names one version, by path and digest (section 7).
- **`--fonts`:** the fonts' directory (2.5).
- **`--scratch`:** an empty directory of the run's: Chromium's profile
  (browser.md, section 3) and the font configuration's cache. Nothing
  in it is evidence.
- **`--out`:** where shots are written, each by a name relative to it
  that climbs nowhere.
- **Its life:** it binds its proxy (2.4), spawns Chromium with the
  kit's command, the proxy's and fonts' arguments and environment
  added, and once the kit is ready prints `{"ready":{"browser":V}}`,
  V being the version the kit reports. Then it reads calls, one at a
  time: it answers each before it reads the next. The end of its
  standard input closes every page, then the browser, and it exits 0
  once Chromium's exit is confirmed. A browser that ends by itself, or
  a call it cannot answer, ends it with status 1 and a line on its
  standard error.
- **Its limits** are the kit's (browser.md, section 8), its origins'
  (2.3) and a call's line, each fixed at start and stated on its
  standard error when a call passes one.

### 2.3 Origins

- **`serve` reads a directory whole:** its regular files, under the
  limits of a file's and the origin's bytes and their count (8 MiB,
  64 MiB and 4096 files), refused `limit` past any, and links and
  special files refused `origin`; at most 8 origins. It answers every
  `GET` of a file's path with the file and a type by its extension, and
  anything else not found.
- **One origin each:** each `serve` binds a listener of its own on
  `127.0.0.1`, port 0, so that a page's icons, masks and fonts are
  fetched under their own origin's rules, as a product's bundle serves
  them. The origin's address is its answer.
- **Only served origins are opened:** `go` refuses an address on none
  of them.

### 2.4 The closed network

- **The proxy:** a listener the program binds on `127.0.0.1`, port 0,
  before Chromium starts, named in Chromium's arguments
  (`--proxy-server`). Chromium sends it every request but those to a
  loopback address, which go direct (its implicit bypass): so a page
  reaches its own origins and no other host, whether named or by
  address.
- **Refused and recorded:** the proxy reads a request's first line and
  answers `403`, closing the connection, and keeps the host and port
  asked. `trouble` reports each as `network`, beside the page's own
  (browser.md, section 7), so a page that wants the network says so;
  Chromium's failed load shows there too, as a log error. The proxy
  cannot tell pages apart, so a `trouble` reports every refusal since
  the last, whichever page asked: the capture keeps one page open at a
  time.
- **No resolver:** Chromium's host resolver maps every name to none
  but `127.0.0.1`, the origins' own address, which the rule would map
  too (`--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1`), so
  nothing is looked up.
- **Loopback goes direct,** whatever its port: a page could reach
  another listener on the machine's loopback, which no lab page names.
  The program does not close it; its caller does, starting it in a
  network namespace of its own, whose loopback holds only the program's
  listeners. The capture does (5.1).
- **Chromium's own requests are off:** the program starts Chromium
  with the switches that stop its background services (component
  updates, sync, safe browsing, autofill's server, field trials, the
  default apps, metrics and pings), and with a preference for the one
  no switch stops (spellcheck), beside the kit's command, so that
  a `trouble`'s `network` entries are the pages' own. Its scrollbars
  are hidden too, so that no shot depends on them: a viewport's shot is
  its width by its height, whether the page scrolls or not. Each switch
  stands for a service that asks the network on its own:
  - **The order:** the initial address comes last, after every switch.
    A distribution's launcher (Debian's) moves the switches written
    before the first address to the end, so a feature list given
    there would yield to the kit's own `--disable-features`.
  - **The network time:** the tracker's query for the time
    (`clients2.google.com`) stays off, by the background networking
    switch with the order above.
  - **The component updater:** its source is a blocked loopback port
    with no pings (`--component-updater=url-source=...,disable-pings`),
    since Optimization Guide asks for its on-device model's manifest
    whether its own features are on or not.
  - **The search preconnector:** the default search engine's
    preconnections (`www.google.com`) are off by their features.
  - **Accounts and messaging:** Gaia's and GCM's addresses are a
    blocked loopback port, which Chromium refuses before connecting.
  - **Spellcheck:** its dictionary's download
    (`redirector.gvt1.com`), asked once a typed field is checked, has no
    switch that stops it, so the program writes the fresh profile's
    preferences (`Default/Preferences` in the profile, in its scratch
    directory) before it starts Chromium: spellcheck off
    (`browser.enable_spellchecking`) and no dictionary
    (`spellcheck.dictionaries` empty). A typed field shows no spelling
    marks.

  These hold for the lab's Chromium, the one its settings name. That
  a blank page asks for nothing is the capture's process tier's to
  show, on that Chromium; the browser program's own story runs on
  whatever Chromium the machine has, and does not assert it.
- **Whole raster:** the program starts Chromium with partial raster
  off (`--disable-partial-raster`), so a page drawn again after a
  press, a typed field or a shot at another scale is drawn whole, as a
  fresh one is: on the lab's Chromium, a shot after a click otherwise
  differs between fresh browsers in a few dozen pixels by a shade or
  two, and W1's look met the same drift between its seeds (temper's
  `qa-web.md`, 3.4). A page's first drawing is the same either way,
  byte for byte. Switching off the GPU, its compositing or its raster
  as well changed no shot, so they stay as the kit sets them.
- **One tile a viewport:** the program also starts Chromium with its
  raster tiles and its untiled layers 2048 pixels square
  (`--default-tile-width=2048`, `--default-tile-height=2048`,
  `--max-untiled-layer-width=2048`, `--max-untiled-layer-height=2048`),
  so a viewport the lab shoots is drawn in one tile, with no seams:
  with partial raster off alone, a viewport's shot still differed
  between fresh browsers at the tile seams, a few pixels on one row or
  column, a shade or three apart. The tile size alone does not hold,
  since Chromium tiles a layer past its untiled limits whatever the tile
  size (`cc/layers/tile_size_calculator.cc`). With the four, six
  captures of temper's look held one digest for every viewport and
  crop, in 63.9 s a capture against 65.9 s; they draw edges and corners
  a shade apart from the default tiling, so a first drawing differs
  from the one before them, once.
- **Its bounds:** a request's first line is read up to 8 KiB, and a
  `trouble` reports at most 64 refusals, the rest as one `network`
  entry counting them.

### 2.5 Fonts

- **A font configuration of its own:** the program writes
  `fonts.conf` in its scratch directory, naming the fonts' directory as
  the one font directory and the scratch directory as fontconfig's
  cache, and its rules for the families a page or the browser names but
  the directory lacks (the generic ones, and the browser's defaults such
  as Arial and Times New Roman) from the directory's own
  `aliases.conf`, when it has one. It gives
  Chromium `FONTCONFIG_FILE` naming it, beside the kit's environment
  (browser.md, section 3).
- **So a page's fonts are files:** a page's web fonts, from its own
  origin, and the directory's for every family a page names without
  one. The machine's installed fonts are not seen.
- **The directory is the lab's,** named in its settings and digested in
  its key (section 7). Its first holds the DejaVu families (Sans, Sans
  Mono, Serif) and their aliases.
- **A font from another host is not drawn.** A page that loads its fonts
  from elsewhere draws with the directory's in their place, and its
  trouble names the request refused (2.4). The capture records both;
  how a product's scenes treat it is theirs.

## 3. The calls

A call is `{"call": NAME, ...}` on one line; its answer one line, a
member named for its result, or `{"refused": WHY, "detail": TEXT}`.
Lines are canonical JSON (lab.md, 2.2). A page is named by the caller,
a node by the kit (browser.md, section 5).

| Call | Its members | Its answer |
|---|---|---|
| `serve` | `origin`, a name; `dir`, an absolute path | `{"served":{"origin","address"}}` |
| `open` | `page`, a name; `width`, `height`, `mobile` | `{"opened":PAGE}`: a new person and page at `about:blank`, its viewport at scale 1, light scheme, reduced motion |
| `go` | `page`; `url`, on a served origin | `{"done":PAGE}`, once the load event fires |
| `await` | `page`; `role`, `name`, `match` (`exact` or `words`); `seconds` | `{"met":[SEEN]}` or `{"missed":[SEEN]}` |
| `find` | `page`; `role`, `name`, `match` | `{"found":[SEEN],"more":N}` |
| `tree` | `page` | `{"tree":[{"node","depth","role","name","level"}]}`, the accessibility tree in document order, ignored nodes left out, `level` a heading's or a tree item's, or 0 |
| `select` | `page`; `selector`, CSS | `{"selected":NODE}` or `{"selected":null}` |
| `style` | `page`; `node`; `properties` | `{"style":{"values":{NAME:VALUE},"faces":[{"family","custom","glyphs"}],"box":RECT}}` |
| `shot` | `page`; `clip`, a `RECT` or null for the viewport; `scale`, 1 or 2; `file` | `{"shot":{"file","bytes","sha256","width","height"}}` |
| `trouble` | `page` | `{"trouble":[{"kind","text"}]}`, what the page reported since the last |
| `close` | `page` | `{"closed":PAGE}` |

- **`SEEN`** is `{"node","role","name","box"}`, and **`RECT`**
  `{"x","y","width","height"}`, in whole CSS pixels of the document.
- **`style`'s values** are Chromium's computed values, as strings, for
  the properties named; `faces`, the faces that drew the text within
  the node, its descendants' among them, by family, whether a web font (`custom`), with their glyphs'
  count; `box`, the node's border box.
- **`trouble`'s kinds** are the kit's (`exception`, `console`, `log`,
  `crash`, `dialog`) and the proxy's `network`.
- **A refusal** names the kit's refusal (`gone`, `hidden`, `limit`,
  `timeout`, `protocol`, `crashed`) or the program's: `page` (no such
  page), `origin` (an address on no served origin), `call` (a call it
  does not know, or a member missing, unknown or of the wrong type).

## 4. The scene file

`look-scene/1`, canonical JSON (lab.md, 2.2), one file a scene, kept by
the product beside its look experiment. The capture reads it; the
look's checks read it too, with the members they add (`expectations`,
`departures`, and `words` where the file has it). Shown spread out here; the file holds its canonical
bytes.

```json
{"format": "look-scene/1",
 "name": "chat",
 "widths": [{"name": "desktop", "width": 1440, "height": 900, "mobile": false},
            {"name": "phone", "width": 390, "height": 844, "mobile": true}],
 "mockup": {"path": "/chat.html?world=solo&task=T10&bare",
            "ready": {"role": "main", "name": "", "match": "words"}},
 "page": {"path": "/chat-t10.html",
          "ready": {"role": "main", "name": "", "match": "words"}},
 "regions": [
   {"name": "banner", "mockup": {"role": "banner", "name": "", "match": "words"},
                      "page": {"role": "banner", "name": "", "match": "words"}},
   {"name": "composer", "mockup": {"selector": "form textarea"},
                        "page": {"role": "textbox", "name": "Message", "match": "exact"}}],
 "names": ["Chats", "Inbox", "Accounts", "Send"],
 "expectations": [],
 "departures": [],
 "words": [{"id": "banner-words-desktop", "width": "desktop", "region": "banner",
            "data": [], "cites": "qa-web.md, 3.2: the frame's words are the mockups'"}]}
```

- **`name`** matches `[a-z][a-z0-9-]*`, as every name in the file does,
  and names the scene's files.
- **`widths`:** at least one, each a viewport: its `name`, its `width`
  and `height` in CSS pixels, from 1 to 4096, and `mobile`, a phone's
  handling of the page's viewport tag.
- **`mockup` and `page`:** each side's `path` on its origin, and its
  `ready`, a part awaited, by role and name. `mockup` is null for a page
  no mockup draws, which is captured alone; `page` is never null.
- **A live page:** `page` is `{"live", "ready", "focus"}` instead, for
  a page captured from the hand's (5.1, 8.10): `live`, the path the
  hand's page must be at, a path as below, with no query; `ready`, as
  above; `focus`, a finder the part holding the focus must match, or
  null where the scene holds no focus. A scene is live or served
  whole, and a capture takes scenes of one kind (5.1).
- **A path** starts with `/` but not `//`, and holds printable ASCII
  without spaces: a path on the side's origin, and its query. The
  origin serves the file the path names, its query left to the page's
  script.
- **A finder** is `{"role", "name", "match"}`, `match` being `exact` or
  `words` (section 3's `find`), or, on the mockup's side of a region
  alone, `{"selector"}`, CSS, where the mockup's part has no role of its
  own. The page's side is found by role and name only, as a person
  finds it. An empty name by words matches any name; a finder that
  matches several takes the first in document order, and the record
  counts the rest.
- **`regions`:** pairs of a part found on each side, each named. With
  no mockup, a region's `mockup` is null. `page` is no region's name:
  its crop would be the viewport's shot (5.2).
- **`names`:** the words the structure shows (5.3): the frame's and the
  controls', never content. Each nonempty, none repeated.
- **`expectations` and `departures`:** lists, the look's checks'
  (5.4), which the capture checks and does not use. Each entry has an
  `id`, a name unique among the scene's expectations and departures; a
  `width`, one of the scene's widths' names; and `cites`, nonempty
  text naming the decision or the section that holds it.
  - **An expectation** is `{"id", "width", "region", "property",
    "value", "cites"}`, the page's region having that value of the
    property, or `{"id", "width", "entry", "count", "cites"}`, the
    page's outline holding that entry `count` times (0 for none). A
    `property` is one of 5.3's, `faces`, `x`, `y`, `width`, `height` or
    `shown`, and its `value` is text as `style.md` shows it: a
    computed value, the faces as their families (` (web)` after a web
    font's) sorted and joined by `, `, a box member in decimal, `yes`
    or `no`.
  - **An entry** is an outline line's text without its indentation,
    after its enclosing landmarks' texts, each followed by ` > `:
    `banner > navigation "Main navigation" > link "Chats"`.
  - **A departure** is `{"id", "width", "signature", "cites"}`, a
    difference accepted by its signature's part after
    `<scene>/<width>/` (`style/banner/faces`), or `{"id", "width",
    "entry", "cites"}`, any difference in that entry's count.
- **`words`,** a list the file may leave out (none then), the word
  check's (5.4): `{"id", "width", "region", "data", "cites"}`, a region
  whose words are held to the mockup's at that width. `id`, `width` and
  `cites` as above, the id unique among the expectations', the
  departures' and the words' ids; `region` one of the scene's regions,
  named once a width; `data`, a list of the page's data in that region
  (its counts, times and titles drawn from a run), each a regular
  expression (Python's `re`) that matches no empty text, none
  repeated: each span one matches is one word of data, `#` on both
  sides (5.4). The rule is the scene's declaration, never what a side
  happens to lack.
- **Refused,** every problem named with its member, before the browser
  starts. A file of another format is refused for that alone, before
  any other member is named. Otherwise: bytes not canonical, a missing, unknown or
  mistyped member, a name not matching or repeated (a scene's among the
  command's scenes, a width's or a region's in its scene, an
  expectation's or a departure's id), a finder with
  both a role and a selector or neither, a selector on the page's side
  or in a readiness, another `match`, a path or a size out of its rule,
  a region's mockup finder in a scene without a mockup, or none in one
  with, a region named `page`, a live page with a `path` or a served
  one with `live` or `focus`, a `focus` finder with a selector, an expectation, a departure or a words entry naming a
  width or a region the scene lacks, a property not listed above, a
  negative count, an empty `cites` or signature, a words entry in a
  scene without a mockup, a region's words named twice at a width, a
  data pattern that does not compile, matches empty text or is
  repeated.

## 5. The capture

### 5.1 Its run

```
scripts/lab/look/capture --browser PROGRAM --chromium PATH --fonts DIR
    [--mockups DIR] (--page DIR | --live ADDRESS --live-out DIR)
    --ready-seconds N --out DIR SCENE...
```

- **Its arguments:** absolute paths, the program and Chromium
  executable files, the fonts, mockups and page directories; `--mockups`
  given when a scene has a mockup; `--page` for served scenes, or
  `--live`, the hand's listener (a loopback address and port, 8.1),
  with `--live-out`, the hand's `--out`, for live ones (section 4),
  every scene of the kind its arguments name; `--ready-seconds` a positive integer;
  `--out` a directory the capture makes, `0700`, refusing one that
  exists. Every scene is read and checked before anything starts.
- **The program in a namespace of its own,** under bubblewrap (`bwrap`,
  found as the runner finds it, on the default path, and among the
  key's `programs`, lab.md 2.3):
  - a process namespace of its own (`--unshare-pid`), so that every
    process the program starts, Chromium's among them, ends with it;
  - a network namespace of its own (`--unshare-net`), whose loopback
    holds only the program's origins and proxy, so that a page reaches
    no other listener of the machine (2.4);
  - the file system read-only (`--ro-bind / /`), with `/dev`, `/proc`,
    `/tmp` and `/run` of its own, and its scratch and `--out` bound
    writable;
  - its environment `PATH=/usr/bin:/bin` alone, and its end bound to
    the capture's (`--die-with-parent`).

  Its scratch is a temporary directory the capture removes at its end,
  once the program has ended (on a failure, its input closed, and
  killed past its deadline); one it cannot remove fails the capture;
  its `--out` is the capture's; its standard error is kept as
  `browser.log` in `--out`. Its ready line gives the browser's version,
  which the record keeps.
- **The two directories served,** `mockups` (when given) and `page`,
  each its own origin. With `--live`, `mockups` alone: the page's side
  is the hand's page, on the product's origin.
- **For each scene, each width, each side, one page at a time,** since
  the proxy cannot tell pages apart (2.4), the mockup's side first:
  `open` at the width; `go` to the side's path on its origin; `await`
  its readiness within `--ready-seconds`; `tree`; for each region, its
  part found (`find`, or `select` on the mockup's side), its words from
  the tree (5.2), its `style` with the properties of 5.3, and its
  `shot` at scale 2 clipped to its box; the viewport's `shot` at scale 1; `trouble`; `close`. Then the
  width's views (5.3), from the files written.
- **A live page's side** is read from the hand's page as the seed left
  it, never opened, moved or reloaded, by the hand's look reads (8.10),
  one call a connection to `--live`: `look-viewport` at the width; the
  `address`; `look-await` of its readiness; `look-tree`; for each
  region, `look-find` and `look-style`, and `look-shot` at scale 2
  clipped to its box; `look-focus`; the viewport's `look-shot` at scale
  1; `trouble`. Each `look-shot` names a file under the hand's `look/`
  (8.10), which the capture copies to its own path in `--out` and
  checks against the answer's size and digest. The side is ready when
  its address's path is the scene's `live` and its readiness is met,
  `missed` otherwise. Its trouble is the hand's since the last
  `trouble` call, each entry's kind and text, so a seed that reads the
  trouble once before its first live capture leaves each capture the
  trouble of its own scene's steps and of itself. After the scene's
  last width, and on a failure, `look-viewport` with no size gives the
  hand back its own (8.10). A region's refused read is recorded as on a
  served side; a refused `look-viewport`, `address` or `trouble`, a
  hand that cannot be reached, or an answer past the deadline fails
  the capture as a refused `open` does (below).
- **It goes on:** a side that misses its readiness is recorded
  `missed`, and read all the same; a region not found, without a box,
  or whose call is refused (a shot past the program's limit, say) is
  recorded so, with the refusal, and the rest goes on.
- **Each answer is awaited** within `--ready-seconds` and 30 seconds
  more. Past that, at an answer it cannot read, at the program's end,
  or at a refused `serve`, `open` or `close` (with no origin or page,
  nothing after it can be read), the program has failed.
- **Exits** 0 once every scene is recorded, whatever it shows; 2 on a
  refusal (an argument, a scene, an `--out` that exists), before the
  program starts; 1 when the program fails, with its standard error's
  tail on the capture's, and `--out` left as it is, without
  `capture.json`.

### 5.2 The record

In `--out`, beside the files it names:

```
capture.json                          look-capture/1
browser.log                           the program's standard error
<scene>/<width>/<side>/tree.json      the tree call's answer, canonical
<scene>/<width>/<side>/outline.txt    the structure (5.3)
<scene>/<width>/<side>/page.png       the viewport, scale 1
<scene>/<width>/<side>/<region>.png   a region, scale 2
<scene>/<width>/structure.diff        the outlines' difference
<scene>/<width>/style.md              the style table
<scene>/<width>/pair.png              the page shots side by side
<scene>/<width>/pairs/<region>.png    a region's crops side by side
<scene>/<width>/diff/<region>.png     a region's difference image
```

`<side>` is `mockup` or `page`. `capture.json` is canonical JSON
(lab.md, 2.2), its lists in the scenes' order, as the command named
them, and in each scene's own:

```
{"format": "look-capture/1", "browser": VERSION, "scenes": [SCENE]}
SCENE   {"name", "scene": DIGEST, "widths": [WIDTH]}
WIDTH   {"name", "width", "height", "mobile", "mockup": SIDE or null,
         "page": SIDE, "structure": FILE or null, "style": FILE,
         "pair": FILE or null, "regions": [REGION]}
SIDE    {"address", "ready", "trouble", "tree", "outline", "shot"}
REGION  {"name", "mockup": PART or null, "page": PART,
         "pair": FILE or null, "content", "diff": FILE or null,
         "psnr", "differing", "pixels"}
PART    {"found": true, "more", "box", "values", "faces", "shot",
         "words"}
        or {"found": false} or REFUSED
FILE    {"path", "bytes", "sha256"}
REFUSED {"refused", "detail"}
```

- **A scene's `scene`** is its file's digest, `sha256:` and the hex of
  its bytes.
- **A live side** adds `focus`: the entry holding the focus, its
  `{"role", "name"}` as the tree gives them, or null when none does
  (the document's body) (8.10). A served side has no `focus`.
- **A side's** `address` is the URL it went to, a live side's the
  hand's page's; `ready` is `met`,
  `missed`, or the refusal of its `go` or `await`; `trouble` is the
  `trouble` call's list, the proxy's refusals among it; `tree`,
  `outline` and `shot` are files, or the refusal of their call.
- **A part's** `more` is how many others its finder matched; `box` is
  its style's border box (a `RECT`, or null for a part not displayed);
  `values` and `faces` are its style's; `shot` is its crop, null
  without a box, or the refusal; `words` is the names of the
  `StaticText` entries under its node in the tree, in document order,
  as the tree gives them: the text a person reads in the part, a
  control's or a heading's as well as a hint's or a line's. It is null
  when the tree was refused or the node is not in it (an ignored node,
  as 5.3's content has it). It is read from the tree the side already
  took, so it needs no call of its own; a record made before it leaves
  `words` out, and the word check is undecided on it.
- **A region's** `content` is `same`, `differs` or `unknown` (5.3);
  `diff`, `psnr`, `differing` and `pixels` are its difference image's,
  null where none is made.
- **Every file** is named by its path from `--out`, `/`-separated, its
  size and its digest (lab.md, 2.2).

### 5.3 The views

- **Structure:** `outline.txt`, from the tree, one entry a line, in
  document order: the landmarks (`banner`, `navigation`, `main`,
  `complementary`, `contentinfo`, `region`, `form`, `search`), the
  headings with their levels (`heading 2`), the lists with their items'
  count, the `listitem` entries one level deeper in the tree
  (`list (4)`), and the controls by role (`button`, `link`, `textbox`,
  `searchbox`, `checkbox`, `radio`, `combobox`, `tab`, `menuitem`,
  `switch`). Each line is indented two spaces for each landmark it is
  in, and ends with its name, quoted as JSON, when the name is one of
  the scene's `names`; otherwise it shows its role alone.
  `structure.diff` is the two outlines' unified difference, three lines
  of context, from `mockup/outline.txt` to `page/outline.txt`, empty
  when they agree; with no mockup, there is none.
- **Style:** per region, these computed properties: `font-family`,
  `font-size`, `font-weight`, `line-height`, `letter-spacing`;
  `color`, `background-color`; each side's `border-*-width`,
  `border-*-style` and `border-*-color`, each corner's
  `border-*-radius`, `box-shadow`; each side's `padding-*`, `row-gap`,
  `column-gap`; then the used faces and the box's `x`, `y`, `width` and
  `height`. `style.md` holds a table per region, a row per property,
  the mockup's value, the page's and a mark, `≠` where they differ by
  the default rule:
  - two values agree when, split into tokens (a number with `px`, or
    any other run of characters outside spaces, commas and
    parentheses), they have as many tokens, each `px` length within one
    pixel of the other's, and every other token equal: so colours,
    keywords and families exactly, lengths within one pixel;
  - the faces agree when their families and `custom` flags, sorted, are
    equal, their glyphs' counts left out; the box's members within one
    pixel;
  - a part missing on one side shows `—` there, and marks nothing.
- **Picture:**
  - `pair.png`: the two viewport shots side by side, the mockup's on
    the left, top-aligned, 16 pixels apart, on white; for each region
    found on both sides, its two crops the same way in
    `pairs/<region>.png`. A region found on one side only has its one
    crop; a scene without a mockup has no pairs.
  - **Content:** a region's content is the names of its node's entry in
    the tree and of the entries under it. It is `same` when the two
    sides' are equal, `differs` when not, and `unknown` when a part was
    not found or its node is not in the tree (an ignored node, such as
    an element no role names).
  - **The difference image,** `diff/<region>.png`, only where the
    content is `same` and both crops were taken: the two crops placed at
    their top left on a canvas of the larger width and the larger
    height; each pixel both cover with equal colours drawn as the
    mockup's in grey, faded to a quarter of its contrast, and each other
    pixel, those only one covers among them, drawn magenta
    (`#ff00ff`). `differing` counts those, of `pixels`, the canvas's.
  - **PSNR,** over the red, green and blue of every pixel of the
    canvas, a pixel one crop does not cover at the largest error, 255:
    `10 log10(255² / MSE)`, in hundredths of a decibel, rounded to the
    nearest; null when no pixel differs.
- **Images** are PNG, read as 8-bit RGB or RGBA without interlace,
  as Chromium writes them, and written as 8-bit RGB.

### 5.4 Its checks

```
scripts/lab/look/check structure|style|words --capture DIR [--capture DIR]... SCENE...
```

Three check commands by lab.md's contract (7.2): `structure`, which an
experiment names `look-structure`, `style`, named `look-style`, and
`words`, named `look-words`.
Each reads each `DIR/capture.json` and the scene files, and the
record's files it needs, and nothing else; it starts no browser. Each
scene is judged from the one record that holds it, so that live
scenes, each captured when the seed has made its state, are checked
together; a scene in no record, or in two, is could not judge.

- **What it reads must hold:** the record a `look-capture/1`, each
  scene named in it and its file's digest the record's `scene`, each
  file it reads its record's size and digest. Anything else (a file
  missing or unreadable, a scene absent or changed) is could not judge,
  status 2, an `undecided` line naming it; so is a part it needs whose
  call was refused, or a region's mockup part not found (the scene's
  finder, not the page, is wrong), reported for that region alone while
  the rest is judged.
- **Its signatures** are `<scene>/<width>/` and a tail naming a kind and
  what it names, never a value; the values go in the summary.
- **The expected page,** for each scene at each width:
  - **the base:** the mockup's side as captured, when the scene has
    one: its outline's entries with their counts, and each region's
    values where its mockup part was found. A scene without a mockup
    has none: what it holds the page to is its expectations alone, an
    outline entry they don't name expected no times and a region's
    property compared only where one names it;
  - **expectations** override the base, entry by entry and property by
    property;
  - **departures** accept: a difference whose signature, or whose
    entry, a departure of its width names is not reported. A departure
    that accepts nothing in the capture fails `departure-gone/<id>`, in
    the check whose kinds its signature names (`style` for `style/...`,
    `words` for `words/...`, `structure` for every other, and an
    entry's), so the list stays true once the page or the mockup comes
    to agree.
- **`look-structure`:**
  - `ready/<side>`: a side whose readiness was missed or refused;
  - `trouble/<side>/<kind>`, and `trouble/<side>/network/<host:port>`
    for each host the proxy refused, once each however many entries:
    what a side's `trouble` reported. A mockup's own trouble (a font
    stylesheet on another host) is a departure; the page's, a finding;
  - `missing/<role>[/<name>]` and `extra/<role>[/<name>]`: an entry the
    page's outline holds fewer times than expected, or more. `<role>`
    is the line's text before its name, spaces as `-` and parentheses
    dropped (`heading-2`, `list-3`); `<name>` is the name the line
    shows, if any, its bytes outside `[A-Za-z0-9._~-]` percent-encoded
    (`Adopt%20a%20repository`). Entries in different landmarks with
    one signature give one line, the summary naming each;
  - `missing/<role>[/<name>]` too for a region whose page part is not
    found while its mockup's is, or while an expectation names it, by
    its page finder's role and name;
  - `order/<region>`, with a mockup: the regions drawn on both sides,
    ordered by their boxes, region A before B when A's bottom is at
    most B's top, within a pixel, or, the two overlapping vertically,
    A's right at most B's left. A pair overlapping both ways (one
    nested in the other, or crossing) has no order on that side, and is
    never inverted. A pair ordered one way on the mockup and the other
    on the page is inverted. While a pair is inverted,
    the region in the most inverted pairs, the later in the scene's
    `regions` on a tie, is out of order and leaves the count: so the
    region that moved is named, not those it passed. Order is by the
    screen, since a stylesheet moves a part without moving it in the
    tree; a part moved in the tree alone shows in the outline's text
    and its diff.
  - `focus/page`, for a live scene whose `focus` is a finder: the
    side's `focus` null, or not matching the finder (its role equal,
    its name by the finder's `match`). The mockup is static, so focus
    is the page's alone, held by the scene, never by a base.
- **`look-style`:** for each region whose page part was found, each
  property with an expected value, `style/<region>/<property>` when the
  page's disagrees by 5.3's rule: 5.3's properties, `faces`, the box's
  `x`, `y`, `width` and `height`, and `shown`, the part drawn on one
  side and not on the other, whose box is then not compared.
- **`look-words`:** for each of the scene's `words` entries at the
  width, the region's words on the mockup's side against the page's,
  the mockup's being the base: the page's words are the mockups' where
  temper's model can hold them (qa-web.md, 3.2), so a difference is a
  finding, a departure or a question.
  - **Words, not runs:** each side's `words` joined by one space; each
    of the entry's `data` patterns, in order, replaces each span it
    matches with `#`; the text split at white space into words. Two
    pages that split one text into runs differently (an inline element,
    an icon's span) agree, and a time or a count across a run's end is
    still one span.
  - **Data never hides a word:** a span a pattern matches that is
    empty or holds white space is no datum: the entry is undecided,
    naming the pattern and the span, since a word beside a datum must
    still sign.
    A pattern that matches nothing on either side is named on the
    check's stderr (`<scene>/<width>/<id>: data <pattern> matched
    nothing`) and signs nothing, so a stale declaration shows.
  - **The comparison:** the two lists of words in order, by Python's
    `difflib.SequenceMatcher` without its junk heuristic. Each opcode
    other than `equal` signs once with its span's words: `delete`,
    `words/<region>/missing/<words>`, the mockup's words the page
    lacks; `insert`, `words/<region>/extra/<words>`, the page's the
    mockup lacks; `replace`, both. A word moved signs both, where it
    left and where it came.
  - **`<words>`:** the span's words joined by one space, percent-
    encoded as the structure's names; a span of more than eight words
    is its first eight, then `/`, its count, `w-` and the first eight
    hex digits of its joined text's SHA-256 (`/31w-1f0a9c3e`), so a
    long span keeps a stable, readable signature. The summary holds
    the span's words whole, and the other side's where it replaces.
  - **What it cannot judge,** for that entry alone: either part's call
    refused, the mockup's part not found, or either part's `words`
    null or absent (undecided); the page's part not found signs
    nothing here, since `look-structure` signs it.
  - **Not yet: a disclosure's words.** A part closed in a disclosure
    (the phone's Menu, the chat's model chooser) is out of the tree, so
    its words are neither side's. Opening it needs a call that presses
    a control, which section 3 lacks: a words entry naming a finder to
    press before the tree and again after it, a step of the capture's
    on both sides, and the program built anew. Until that call, a
    disclosure's words are judged by eye.
- **Pure decisions:** the expected page, the comparisons, the order,
  the words, the signatures and the departures are functions of the
  records and texts read, beside the command that reads them.

## 6. Tests

The capture's; the hand's are 8.9's.

- **The kit's reads** are the kit's (browser.md, 9 and 5.1): its step
  tests, its machine world, and its facts on real Chromium.
- **The program:** one story in skein's browser suite (browser.md,
  section 9): the built program, with the suite's Chromium
  (`SKEIN_TEST_CHROMIUM`) and a fonts' directory named as it is
  (`SKEIN_TEST_FONTS`, else Debian's `fonts-dejavu-core` directory,
  `/usr/share/fonts/truetype/dejavu`; neither holding DejaVu fails the
  suite at its start, naming both and the package), a fixture directory
  served, a page opened at the phone's size, a part awaited, its tree,
  a selector's node, a style, a clipped shot at scale 2, a page whose
  external stylesheet the proxy refuses (its `network` trouble, and the
  page drawn without it), a call refused, and the end of its input
  closing everything. Its blank page, opened first at each of the two
  sizes, reports no trouble once the story's other calls are done:
  Chromium's services asked for nothing.
- **The capture's decisions** in the runner's unit tier (lab.md, 10.3),
  pure: the scene file and each refusal of section 4; the outline from
  a tree, with landmarks nested, levels, a list's items and names shown
  or not; the structure's difference; the style rule from both sides of
  each bound (a length one pixel off and past it, a colour, a keyword,
  faces); the content's three answers; PNG's reading (RGB, RGBA, each
  filter) and writing; the side by side; the difference image and the
  PSNR on small images, equal, one pixel off, and of unequal sizes; the
  record's canonical form; the scene's expectations, departures and
  words and their refusals; a part's words from a tree, and null for a
  node the tree leaves out.
- **The checks' decisions** in the same tier, on records and texts
  written as fixtures, with no browser: each signature of 5.4 from a
  record that shows it and none from one that doesn't; the base, an
  expectation overriding it and a scene without a mockup; a departure
  accepting by signature and by entry, and gone; the order from both
  sides of its pixel, a region moved past two, a tie, a pair nested
  or crossing (no order), a part not drawn; the words joined across
  runs split differently, a datum cut on both sides, a changed word
  beside a datum still signing, a datum's span across white space
  undecided, a stale pattern on stderr, a moved word signing both, a
  long span's bound; each could-not-judge; the statuses and lines of
  lab.md, 7.2.
- **The capture's run** in the runner's process tier: the built program
  (`SKEIN_LAB_BROWSER`), Chromium (`SKEIN_TEST_CHROMIUM`) and the fonts
  (`SKEIN_TEST_FONTS`, else Debian's directory, as the program's story)
  on a fixture scene at one small width whose page side moves one
  region and changes one colour. Its record shows both: the moved
  region's entries in `structure.diff`, its box's `y` and the colour
  marked in `style.md`, the recoloured region's difference image with
  its pixels and PSNR, and the pair. The fixture's page also asks for a
  listener the story holds on the machine's loopback, which sees no
  connection. A missing program, Chromium or bubblewrap fails the
  story, naming what is missing.
- **The live side's decisions** in the unit tier, against a listener
  the test holds that answers as the hand does: a live scene and each
  of its refusals; the calls in their order, one a connection; a shot
  copied from `--live-out` and one whose digest differs failing; the
  address's path held to `live`; the viewport given back at the end
  and after a failure; the focus recorded; the trouble's entries
  mapped; a refusal of a region's read recorded and one of
  `look-viewport` failing. The checks', beside 5.4's: `focus/page`
  from both sides of the finder, and scenes judged from several
  records, one in none and one in two.
- **The live capture's run** in the process tier: the built program in
  its hand form on a fixture origin the story serves, its page moved
  by the hand's person calls (a field typed into and focused), then a
  live capture of it at two widths beside a fixture mockup: its record
  shows the focus, both widths' viewports and their pairs, and the
  hand's own viewport given back (a hand `shot` after it at its size).

## 7. In the lab

- **Named in the lab's settings,** `browser` (lab.md, 2.3): the
  program's path, Chromium's path, digest and version, and the fonts
  directory's path and tree digest. Validation refuses a Chromium or
  fonts directory that differs from them, and the run checks them
  again before it starts.
- **Digested in the key** of an experiment that names the browser by
  its templates (lab.md, 4.2): Chromium and the fonts directory in the
  lab's `browser`, the program among its `programs`. An experiment that
  names none keeps its key.
- **The lab's copies:** `~/src/rust/tmp/lab/browser/`, holding the
  program, built from skein (`cargo build --release -p
  skein-lab-browser`) and copied there, and `fonts/`, made by
  `scripts/lab/look/fonts` from fonts-dejavu-core's directory: the
  regular, bold and oblique faces of DejaVu Sans, Sans Mono and Serif
  it holds, the three regular ones required, and the runner's
  `look/aliases.conf` (the generic families and the browser's defaults,
  such as Arial, Helvetica, Times New Roman, Georgia and Courier New,
  to the three), each file `0400`, the directory `0500`. It prints its
  `fonts` member for the settings. Chromium is the one the lab names,
  by path; the machine's packaged browser is never used.
- **What the key leaves out:** Chromium's files beside its program
  (its `.pak` resources and ICU data) are stood for by its version, and
  the system libraries it loads (fontconfig, FreeType, the graphics
  stack) are the machine's, so they are not in the key. Shots under one
  key may still differ between machines; a capture's record says which
  machine made it only through the run's `about`.
- **In a product's look experiment:** the capture as its collection,
  `{runner}/look/capture` with the settings' browser by its templates,
  and the checks as its checks, `{runner}/look/check`, so that the key
  holds both through the runner's tree (temper's `lab/look/`). The
  capture also runs by hand, with the same paths. Neither runs in a
  gate, and the shots are evidence, never goldens.
- **The hand in a product's cluster:** the program as a process of the
  experiment's, and `{runner}/hand/` in the runner's tree, digested
  with it (8.8).
- **Live scenes in a product's cluster:** each live capture a seed
  action of the experiment's, after the actions that make its scene's
  state, `{runner}/look/capture` with the settings' browser, `--live
  {proc.browser.ready.address} --live-out {dir.D}` (the hand's
  `--out`), and `--out` under a run directory of its own; a
  collection that copies that directory, and the checks with one
  `--capture` for each. A seed action, not a collection: a collection
  runs once the seed is done, when only the last state is left.

## 8. The hand

A driver's browser in a product's cluster (qa-web.md, section 6): the
same program in a second form, holding one person's page on the
product's origin and answering a person's calls, and `browse`, the tool
an agent acts through there. The agent finds and acts as a world's fake
person does (ui.md, 17.1), through the face (ui-web.md, 14.2), so that
a transcript reads as a story.

### 8.1 Its form and its life

```
skein-lab-browser hand --chromium PATH --fonts DIR --scratch DIR --out DIR
    --origin URL [--peer URL …] --size WIDTHxHEIGHT [--mobile] --listen ADDRESS
```

- **`--origin`:** the product's origin, `http://` and a loopback
  address with its port, as the product's ready line gave it (lab.md,
  4.4): the origin for `go` and `sign-in` (8.5).
- **`--peer`:** repeatable, at most four distinct loopback HTTP origins,
  parsed and checked as `--origin`, distinct from it (8.5).
- **`--size` and `--mobile`:** the page's viewport, as a scene's width
  (section 4), at scale 1, light scheme and reduced motion, as the
  capture's.
- **`--listen`:** the calls' listener, a loopback address with port 0.
- **`--chromium`, `--fonts`, `--scratch` and `--out`** as 2.2 has
  them; `--out` holds the shots and the trouble's log (8.7).
- **Its life:** it binds its proxy and its listener, spawns Chromium as
  2.2 does, opens its one person and page at `about:blank` at its size,
  and once the page is open prints `listening ADDRESS` on its standard
  output, the listener's bound address, its ready line (lab.md, 4.3).
  Then it takes calls (8.2). Its standard input is not read: a
  cluster's process has none (lab.md, 4.3). A `SIGTERM` or a `SIGINT`
  closes the page, then the browser, and it exits 0 once Chromium's
  exit is confirmed, as the runner stops a process (lab.md, 5.2). A
  browser that ends by itself ends it with status 1 and a line on its
  standard error.
- **The capture's form is unchanged:** its command line is 2.2's, with
  no first word.

### 8.2 Calls on a listener

- **One call a connection:** a caller connects, writes one canonical
  JSON line (section 3's form), and reads one line, its answer; the
  program then closes the connection. Calls are answered one at a time,
  in the order their connections were accepted. A connection that has
  not sent a whole line within 10 seconds of its turn (when the program
  starts reading it, the connections before it answered or closed), or
  sends more than 128 KiB, is closed unanswered: a caller stalled ahead
  of it delays a queued call, and never costs it its answer.
- **A loopback listener, not a socket in a directory** (qa-web.md's
  6.1 named one): skein's io has no Unix sockets, and a listener on
  port 0 reported by a ready line is how the lab's processes meet
  (lab.md, 4.4). It is the tool's endpoint (lab.md, 6.2): an agent's
  own commands open no socket, and reach it only through `browse`,
  whose fixed argument names it.
- **A person's calls, and the lab's look reads:** the hand answers
  8.3's, and 8.10's look reads, which the lab's capture sends and no
  tool does; it refuses section 3's calls (`serve`, `open`, `tree`,
  `select`, `style`) `call`. The capture's form answers section 3's
  alone.

### 8.3 A person's calls

A part is found as the fake person finds it (ui.md, 17.1), through the
face: `{"role", "name", "match", "in"}`. `role` is a person's role by
the fake person's name for it (`button`, `link`, `text field`, `text
area`, `heading`, `text`, `check`, `select`, ...), refused `call` for
another; `name`, words in its name (`match` `words`) or the whole of it
(`exact`); `in`, a list of `{"role", "name", "match"}`, the parts it is
within, outermost first, perhaps empty. The first match in document
order is the one acted on, as the fake person's is, and the answer
names it. A **`PART`** is `{"role", "name", "value", "states"}`, its
role by the same names, its states a list of `focused`, `disabled`,
`checked`, `expanded` and `selected`.

| Call | Its members | Its answer |
|---|---|---|
| `sign-in` | `link`, an address on the origin with a query (8.6) | `{"opened":ADDRESS}`, once the load event fires |
| `outline` | `in` | `{"outline":TEXT,"left":N}` (8.4) |
| `press` | a part | `{"pressed":PART}` |
| `type` | a part; `text` | `{"typed":PART}`, the field's whole value replaced by `text` |
| `key` | `key`: `Enter`, `Escape` or `Tab`; `shift` | `{"keyed":KEY}`, into what has focus |
| `choose` | a part, a select; `option`, `{"name", "match"}` | `{"chose":PART}`, the option |
| `go` | `path` | `{"went":ADDRESS}` |
| `history` | `to`: `reload`, `back` or `forward` | `{"went":ADDRESS}` |
| `address` | | `{"address":ADDRESS}` |
| `wait` | a part; `gone`, a boolean; `seconds`, 1 to 300 | `{"met":PART}`, `{"gone":true}`, or `{"missed":[PART]}`: what of its role there was, at most 8 |
| `shot` | | `{"shot":{"file","bytes","sha256","width","height"}}`, the viewport at scale 1 |
| `trouble` | | `{"trouble":[ENTRY],"more":N}`, since the last `trouble` (8.7) |

- **The acts are the face's:**
  - a press on a check or a choice toggles it;
  - typing focuses the field, selects its whole value with the
    platform's select-all key, and inserts the text as one edit, so the
    field holds the text alone (ui-web.md, 14.2); a native date or time
    field takes its value by keys, as the face types it;
  - a choice of an option presses its select and reaches the option by
    the arrow keys and Enter, as a keyboard chooses one;
  - a go, a reload and a move through the history accept the page's
    closing warning, as a person who chose to leave answers it
    (browser.md, section 6).
- **Done is the browser's:** an act is answered once the browser has
  taken it, a move once its navigation commits (browser.md, section 6).
  What follows is the caller's to wait for.
- **An address** is the page's history's current entry, as the address
  bar shows it (browser.md, section 5), whole.
- **A path** starts with `/` but not `//`, and holds printable ASCII
  without spaces: a path on the origin, and its query.
- **A shot** is written to `--out` as `shots/NNNN.png`, numbered from
  0001 in the order taken, and named by that path.
- **A refusal** is section 3's form: the kit's (`hidden`, `covered`,
  `disabled`, `gone`, `timeout`, `limit`, `protocol`, `crashed`), or the
  hand's: `missing`, no such part, its detail the fake person's words
  ("no button named Save in the region named Draft"); `takes`, a part
  whose role takes no such act (a press on a text field, typing into a
  button: ui.md, section 6); `origin`, a path or a link that is not on
  the origin; `call`. A refused act changed nothing the browser had
  taken, and the caller may wait and try again.

### 8.4 The outline

What a screen reader meets, in order, as text: the page's accessibility
tree (browser.md, 5.1), one entry a line, indented two spaces for each
entry it is within.

- **Each line:** its role by the fake person's name; a heading's level
  after it (`heading 2`) and a list's items' count (`list (3)`); its
  name, quoted as JSON, when it has one; a field's value after ` = `,
  quoted the same; and its states, each in brackets (`[focused]`).
- **Roles mapped back** by the web mapping (ui-web.md, section 6): a
  browser role is shown as the person's role that lists it first, a
  text box as a `text area` when it takes several lines and a `text
  field` when one, and two others listing it first (`figure`) by the
  first in ui.md's order; a browser role no role lists first
  (`LayoutTableCell`), by the first that lists it. So a call naming a
  line's role and name finds that line's part. A browser role the
  mapping lacks is shown in angle brackets (`<article>`), which no call
  names.
- **Left out:** the document's root, ignored nodes, and the containers
  (`generic`, `none`), each with its children lifted to its depth; and
  the text within an entry whose name holds that text's words (a
  button's label, a heading's), which a reader says once.
- **Every name and every text,** unlike the look's structure (5.3),
  which keeps a scene's names alone: the hand reads the page, where the
  look compares its frame.
- **Bounded:** at most 64 KiB, cut at a line's end, `left` counting the
  entries cut; an outline `in` a part shows that part alone, its first
  match.

```
banner
  link "Chats"
  button "Sign out"
main
  heading 1 "Chats"
  text area "Start a chat" [focused]
  button "Start" [disabled]
  list (1)
    item
      link "Add a NOTICE file"
      text "running"
```

### 8.5 The origin and named peers

- **The lab names the app's own fakes:** an app's sign-in and forge
  flows leave its origin for its own fakes. Up to four `--peer URL`
  arguments name those origins; nothing else opens. Each is an
  `http://IP:PORT` origin checked exactly as `--origin`, distinct from
  it and every other peer.
- **The proxy takes all but the origin and peers:** the hand starts
  Chromium with 2.4's proxy, its implicit loopback bypass removed and
  the origin and each peer bypassing it
  (`--proxy-bypass-list=<-loopback>;ORIGIN;PEER…`, authorities only).
  Chromium applies bypass rules last-wins. Every other destination,
  including every unnamed loopback port, reaches the proxy, which
  refuses and records it (2.4). In a cluster the hand has no namespace
  of its own (lab.md, 4.3), and needs none.
- **A link outside those origins is refused:** its load reaches the
  proxy, whose `403` names the address and reason; `trouble` names it
  `network`, and `history` `back` returns.
- **A call names no other origin:** `go` stays relative to `--origin`,
  and a `sign-in` link off it is refused `origin` before the browser
  sees it. Only the page's own navigation reaches a peer.
- **Chromium's own requests** are off, as 2.4 has them.

### 8.6 The sign-in link

- **A credential,** given in a `sign-in` call alone. The lab's
  `hand/sign-in` command reads it from its standard input and sends it
  (8.8), so it is never an argument, and never offered to the agent: an
  experiment names it in its seed, and no tool runs it.
- **Its query is redacted:** the program keeps each link's query (at
  most 4 links, each query at least 16 bytes, else refused `call`), and
  writes `<link>` in its place in every answer, the trouble's log and
  its standard error. A page's script that replaces its address leaves
  nothing to redact; one that keeps it shows `<link>` to `address`.
- **What still holds it** is the run's own record of the seed (the
  stdin file of the action that sent it, lab.md 4.5), as the ctl
  level's seed holds its link (temper's `qa.md`, 4.3), the product's
  printed lines, and the captures of them in `run.json` (a ready line's,
  an awaited line's: lab.md 4.5 and 5.6); none is in an agent's view,
  and the link is spent before the driver starts. What shows a run to a
  reader (a report, a finding's evidence) shows each link's query as
  `<link>`.
- **A refused link is the page's:** the program opens what it is given
  and answers `opened`. Whether the page signed the person in is the
  caller's to wait for, by what the page shows.

### 8.7 The trouble

- **A log as it comes:** each entry of the page's trouble, the kit's
  kinds and the proxy's `network`, is appended to `--out`'s
  `trouble.jsonl`, a canonical JSON line each:
  `{"n","kind","text","address","call","during"}`. `n` counts from 1;
  `address` is the page's, read when the entry came; `call` is the
  number of the call in flight, counted from 1 in the order accepted,
  and `during` its name, or 0 and null between calls. So a product's
  check can name each entry's page by its address (qa-web.md, 8.1), and
  leave out what a seed's call caused.
- **The `trouble` call** answers the entries since the last one, at
  most 64, `more` counting the rest, each an `ENTRY`, `{"kind","text","address"}`.
- **Bounded:** a text at the kit's `Limits::text`, and the log at 4096
  entries, past which one last entry counts those not written.

### 8.8 `browse` and `sign-in`

```
scripts/lab/hand/browse ADDRESS VERB [OPERAND...] [--in ROLE NAME]... [--exact]
scripts/lab/hand/sign-in ADDRESS
```

- **Python's standard library** under the runner's "for now" (lab.md,
  10.1), as the capture is: one call an invocation, to the listener at
  `ADDRESS`, its answer printed for a reader.
- **`browse`'s verbs** are qa-web.md's (6.2), each one call:
  `outline`; `press ROLE NAME`; `type ROLE NAME`, its text the whole of
  its standard input but one final newline; `key KEY`, one of `Enter`,
  `Shift+Enter`, `Escape`, `Tab` and `Shift+Tab`; `choose ROLE NAME
  OPTION`; `go PATH`; `reload`, `back` and `forward`; `address`; `wait
  ROLE NAME [--gone] --timeout S`; `shot`; `trouble`. `--in ROLE NAME`,
  repeated outermost first, scopes a find, and `--exact` matches its
  name whole, the option's too.
- **What it prints:** the answer as lines a person reads (`pressed
  button "Send"`, `refused covered: ...`, the outline as it is, a
  trouble entry a line), and nothing else.
- **Its statuses:** 0 answered, 1 refused, or a wait missed, 2 a usage error (its usage
  on its standard error), 3 the browser unreachable or its answer
  unreadable.
- **Its endpoint is its own** (lab.md, 6.2): `ADDRESS` is its fixed
  argument, a loopback address and port, and no operand of the agent's
  names another: a path is checked as 8.3 has it, before any call.
- **`sign-in`** reads its standard input, one line, the link, sends
  `sign-in`, and prints the answer, redacted by the program. An empty
  input, or more than one line, is a usage error (2), and nothing is
  sent.
- **In a product's experiment:** the process `browser`,
  `["{browser.program}", "hand", ...]`, after the product's server, its
  ready line `listening ` captured as `address`; the seed's action
  `["{runner}/hand/sign-in", "{proc.browser.ready.address}"]`, the link
  on its standard input, a fresh one a product printed when an action
  before presented a spent one, by that action's awaited line (lab.md,
  4.5); and the tool `browse`, `["{runner}/hand/browse",
  "{proc.browser.ready.address}"]` (lab.md, 6.3).

### 8.9 Its tests

- **The kit's additions** (browser.md, 5.1 and 5): its tree's values,
  states and lines, and the select-all key, in the kit's step tests and
  its facts on real Chromium.
- **The face's:** typing into a field that holds words replaces them,
  in its story on real Chromium; the mapping back, pure, for every
  person's role and each browser role it maps to, the role shown finds
  that browser role.
- **The outline,** pure, in the program's unit tests: from trees as the
  kit gives them, nesting, levels, a list's count, values, states, text
  left out under a name that holds it, an unmapped role, the cut.
- **The program's hand:** a second story in skein's browser suite,
  beside the capture's (section 6): the built program in its hand form,
  with the suite's Chromium and fonts, against an origin the story
  serves, whose page holds a banner, a heading, a text field holding
  words, a text area, a button, a check, a select, a link to a second
  page, a link to another loopback listener the story holds, and a
  sign-in path whose script replaces its address. It shows: the ready
  line; a sign-in whose query appears in no answer, no trouble line and
  not on the standard error; the outline; each call answered; typing
  replacing; a key into the focused field; a choice; back and forward;
  the link off the origin shown refused, the other listener seeing no
  connection, its `network` trouble; each refusal of the hand's;
  `wait` met, gone and missed; and `SIGTERM` ending it with status 0.
  Its look reads (8.10): with the text area typed into and focused,
  `look-focus` names it; `look-viewport` to the phone's size and a
  `look-shot` of the viewport at that size, the focus kept; a region's
  `look-find`, `look-style` and clipped `look-shot` at scale 2, its
  file under `look/`; `look-tree`; `look-viewport` with no size giving
  back the hand's, a person's `shot` then at that size; a `look-shot`
  file that climbs or exists refused; and `browse` sending none of
  them.
- **`browse` and `sign-in`:** the runner's unit tier (lab.md, 10.3),
  against a listener the test holds: each verb's call, each answer's
  text, the statuses, and an operand naming another endpoint refused;
  and its process tier, the built program and Chromium against an
  origin the test serves: one walk, a sign-in, an outline, a typing, a
  press, a wait, an address and the trouble, which shows the two
  speaking, each verb's meaning being the program's story's.

### 8.10 The look's reads

A live scene's page is the hand's page as the seed left it (qa-web.md,
3.3): focus after a send, a page after a reload, a dialog in the top
layer, none of which a page opened afresh shows. So the capture reads
it in place, by the kit's look reads (browser.md, 5.1), which the hand
answers to the lab alone.

- **The lab's alone,** as `sign-in` is the seed's: they are calls on
  the hand's listener, which an agent reaches only through `browse`
  (8.2; lab.md, 6.2), and `browse` has no verb that sends them, nor
  does any tool. They read, and change nothing the person meets but the
  viewport, which the capture gives back before the seed goes on.
- **Section 3's reads, on the one page:** the same members, but
  `page`, and the same answers. Roles and names are the browser's, as
  section 3's, not the person's (8.3): a scene's finders are the look's.

| Call | Its members | Its answer |
|---|---|---|
| `look-viewport` | `width`, `height`, `mobile`; or none | `{"viewport":{"width","height","mobile"}}`: the page's viewport now, the size given, or with none the hand's own (`--size`, `--mobile`) |
| `look-await` | `role`, `name`, `match`; `seconds` | section 3's `await` |
| `look-find` | `role`, `name`, `match` | section 3's `find` |
| `look-tree` | | section 3's `tree` |
| `look-style` | `node`; `properties` | section 3's `style` |
| `look-shot` | `clip`, `scale`; `file` | section 3's `shot`, written to `--out`'s `look/` (8.1) |
| `look-focus` | | `{"focus":{"role","name"}}`, the tree's last entry in document order whose states hold `focused`, the document's root left out (Chromium reports it focused too); none: `{"focus":null}` |

- **A viewport change keeps the page:** the kit's viewport holds until
  the next (browser.md, 5.1), and changing it neither reloads nor
  moves the page, so focus, caret, scroll and an open dialog stay. The
  scheme and motion stay light and reduced.
- **A shot's `file`** is a relative path, `/`-separated segments of
  `[a-z0-9-]`, the last ending `.png`, written under `look/` in
  `--out`, apart from the person's `shots/` (8.3); one that exists is
  refused `call`.
- **Redacted and logged as any call:** their answers pass the sign-in
  link's redaction (8.6), and the trouble's log names them as the call
  in flight (8.7).

## 9. Open questions

- **A dark scheme** for a product whose look follows the system's.
- **Pages that animate** past reduced motion: readiness by a part
  shown holds for a static page; a page that moves after it would need
  an expectation of its own.
- **A part moved in the tree alone,** its box where the mockup has it:
  the outline's text and diff show it to the judge, and no check
  signs it, since the record does not say which region an entry is
  in.
