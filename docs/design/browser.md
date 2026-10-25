# Browser

Implemented, 2026-10-06, revised 2026-10-19; the lab's look reads (5.1)
provisional, 2026-10-22. The design of
`skein-browser`: a testing kit that drives a real, headless Chromium from
a service's loop, so that a service with a web can test it as a person
uses it. It is a step machine over the child's pipes, speaking the
DevTools protocol (CDP), with no dependency beyond skein. Its first user
is temper's browser shell (temper's `client/browser.md`, section 4); the
person's browser face in `skein-fake-person-browser`, and the UI
example's browser tier through it, are others (ui-web.md, section 14).
The lab's browser (lab-browser.md) uses it too, and is alone in using
its look reads (5.1).

## 1. In one page

- **A browser is a child process.** The owner spawns Chromium through io
  with the command the kit gives. The kit speaks CDP over two of the
  child's pipes: no WebSocket, no WebDriver, no driver binary, no Node.
- **One loop, one thread.** The kit is a step machine like skein's
  others (programming-model.md): requests from the test, the pipes'
  events from below, deadlines fired by the loop. It never sleeps, and
  it has no async runtime.
- **A test sees what a person sees.** It finds things by role and
  accessible name in the browser's accessibility tree, the name exactly
  or by words in it, presses them, types into them, and waits for what
  should show. It reads no page state, and the one script it runs in the
  page is the expression `document`, which names the page's current
  document as a find's root (sections 4 and 6).
- **The look's reads are the lab's, never a test's.** The lab's browser
  (lab-browser.md) asks what a person's eyes take in but no role names:
  a part's computed style, its box and the faces that drew it, the tree
  with its headings' levels, a shot clipped to a part at twice the scale,
  and the viewport and media the page is drawn for. They are one request
  of their own (5.1). A test that leaned on them would hold a page to
  its look, not to its use, and its shots would become goldens.
- **Actions check what a person would.** A press lands only on something
  on screen, enabled and not covered by something else. Otherwise it is
  refused, saying why, and the test can wait and try again.
- **A go answers the page's closing warning.** A person who navigates,
  reloads or moves through the history has chosen to leave, so the kit
  accepts the page's `beforeunload` prompt then, and says that it did.
  It dismisses any other native dialog, and reports it as trouble.
- **Waiting is a deadline, never a sleep.** An expectation is looked for
  again at a short interval until it is met or its deadline passes. A
  missed expectation reports what was there instead.
- **Trouble is reported, never judged.** The kit reports the page's
  exceptions, console errors, log errors (a content security policy
  violation among them), crashes and the native dialogs it dismissed.
  Whether one fails the test is the test's call.
- **Bounded like the rest of skein.** Persons, pages, operations,
  commands in flight and each message are counted under limits, and
  `worst_case` covers them.
- **Held to the real browser.** The kit is tested against a fake browser
  in a machine world, and the facts that fake relies on are checked
  against real Chromium, as the simulator is held to the kernel.

## 2. In skein

`testing/skein-browser` depends on lib and `skein-json`. Like `skein-llm`,
it is a composition crate rather than a primitive machine. It sits in
`testing/` because services use it only in tests, but it is step code
under the Rust subset, so it runs in a loop beside the service it
tests.

```
the test: a scripted person, a referee
  ▲  events                ▼  requests
skein-browser             persons, pages, operations; CDP's commands and replies
  ▲  stream::Up            ▼  stream::Down
io: the child's pipes     descriptor 3 (commands), 4 (replies and events), stderr
  ▲                        ▼
Chromium, headless
```

```rust
// skein_browser, the machine
pub fn up(browser: &mut Browser, env: &Env<Limits>, ev: Below,
          above: &mut Queue<Event>, below: &mut Queue<Down>)
pub fn down(browser: &mut Browser, env: &Env<Limits>, rq: Request,
            above: &mut Queue<Event>, below: &mut Queue<Down>)
pub fn fire(browser: &mut Browser, env: &Env<Limits>, now: Time,
            above: &mut Queue<Event>, below: &mut Queue<Down>)

pub enum Below { Commands(stream::Up), Replies(stream::Up), Errors(stream::Up) }
pub enum Down  { Commands(stream::Down), Replies(stream::Down), Errors(stream::Down) }
```

Each entry point declares its `MAX_OUT`. At startup the owner checks
that the kit's largest read and room fit the pipes' intake and output
caps, as for any machine stacked on a stream (io.md, 2).

## 3. The browser

- **The command.** `skein_browser::command` gives what the owner spawns
  (io.md, 6): Chromium's program path (the owner's configuration), its
  arguments, its environment, and the pipes to make, which are the
  child's descriptors 3 and 4 and its stderr. The arguments are:
  - `--headless` and `--remote-debugging-pipe`;
  - `--user-data-dir`, a fresh directory beneath the owner's scratch
    root, so that no two browsers share a profile and nothing survives
    a run;
  - quiet and offline: no first run, no default-browser check, no
    background networking, sync, extensions, component updates or
    default apps;
  - `about:blank` as the first page.

  The environment is minimal, and it is the child's whole environment
  (io.md, 6): `LANG` and `LC_ALL` are `C.UTF-8`, which Chromium takes
  as its `en-US`, and `TZ` is `UTC`. So a page renders dates and
  numbers the same everywhere, and a native date or time field shows
  its parts in one order (section 6).
- **Ready** comes once the browser answers its first command
  (`Browser.getVersion`). The event carries the version, which the test
  writes into its report.
- **stderr is read and kept,** its last `Limits::stderr` bytes, for the
  failure report. Chromium writes there freely, so nothing in it is a
  fault.
- **Closing** sends `Browser.close`, then tells the owner to end the
  child, whose exit the owner awaits through io, with a deadline and
  then a kill, as for any child (io.md, 6). It is a lifecycle
  notification, as `skein-llm`'s `Close` is: the kit stops, and the
  pipes and the child stay the owner's to settle.
- **A browser that ends on its own** (its replies' pipe ends, or it
  breaks) fails every operation in flight with `Crashed`, closes every
  page and person, and is `Closed`.

## 4. The wire

- **Framing.** Each message is one JSON document ending in a NUL byte,
  both ways. The kit reads replies with a scan to the NUL under
  `Limits::message` (lib.md, 7), and writes each command whole, sized,
  within the room granted. A message past the limit fails the browser:
  the limit is the test's to raise.
- **Commands, replies and events.** A command carries an id, a method,
  its parameters and, for a page, its session. Its reply carries the id,
  with a result or an error. An event carries a method and its
  parameters. Ids are drawn in order and never reused. Each command
  in flight is an entry in a bounded table, holding the operation it
  belongs to and the shape of reply it expects.
- **One pipe, many pages.** A page is a target attached in flattened
  mode, so its commands and events travel on the same pipe, tagged with
  its session id.
- **Decoding keeps only what it needs.** A bounded JSON document view
  selects the fields for each expected reply and enabled event. Unknown
  fields, and events it has not enabled, are skipped. Ids and node ids
  are parsed as integers, and coordinates as whole CSS pixels with their
  fractions dropped, so no float enters the kit. A screenshot's base64
  is decoded into its PNG's bytes.
- **The domains used:** `Browser` and `Target` (contexts, pages,
  sessions); `Page` (navigation and its lifecycle, screenshots);
  `Emulation` (a fixed viewport; the look's metrics and media); `CSS`
  (the look's computed style and faces); `Accessibility` and `DOM` (finding
  nodes, a scope's object, their boxes, the hit test, focus, scrolling
  into view); `Input` (mouse, keys, text, composition); and `Runtime`,
  `Log` and `Inspector` (the document's object, exceptions, console and
  log entries, crashes). Several of these commands are experimental in
  CDP, which is why the real browser is checked (section 9).
- **A selector is a parameter, never an expression.** The kit sends an
  attribute read's selector as `DOM.querySelector`'s own, and Chromium
  parses it as CSS; no page script runs, and no selector becomes code.
- **The one expression the kit evaluates is `document`.** It sends
  `Runtime.evaluate` with that expression alone, for a find's root
  (section 6), and it never sends `Runtime.callFunctionOn`.
  - The expression calls nothing and reads no page state. A page cannot
    change what it names: a window's `document` can be neither redefined
    nor shadowed.
  - Its result is a handle, as a node id is: an object of the execution
    context that holds the page's current document. The kit releases it
    (`Runtime.releaseObject`) after its find's query.

## 5. The vocabulary

The kit's vocabulary is its own; the test translates its person's
actions into it.

```rust
pub enum Request {                                       // from the test
    Person { person: Token },                            // a browser context: its own cookies and storage
    Page   { person: Token, page: Token, url: Box<[u8]> },
    Go     { page: Token, op: Token, to: Go },           // an address, reload, back, forward
    Find   { page: Token, op: Token, query: Query },     // one look
    Await  { page: Token, op: Token, query: Query, expect: Expect, within: Duration },
    Press  { page: Token, op: Token, node: Node, at: Option<Point> },  // none: the box's centre
    Type   { page: Token, op: Token, node: Node, text: Box<[u8]> },
    Compose { page: Token, op: Token, node: Node, text: Box<[u8]> },  // an IME composition, left open
    Keys   { page: Token, op: Token, node: Node, text: Box<[u8]> },  // each character a key: a native date or time field
    Key    { page: Token, op: Token, key: Key, shift: bool },  // Enter, Escape, Tab, Backspace, arrows, select-all
    Address { page: Token, op: Token },                  // the current address, from the history
    Attribute { page: Token, op: Token, selector: Box<[u8]>, name: Box<[u8]> },  // of the first element a CSS selector finds
    Snapshot   { page: Token, op: Token },               // the accessibility tree, as text
    Screenshot { page: Token, op: Token },
    Look   { page: Token, op: Token, look: Look },       // the lab's alone (5.1)
    Close  { entity: Token },                            // a page, a person, or the browser
}

pub enum Event {                                         // to the test
    Ready   { version: Box<[u8]> },
    Opened  { page: Token },
    Found   { op: Token, seen: List<Seen>, more: u32 },  // more: matches past the limit, counted
    Met     { op: Token, seen: List<Seen> },
    Missed  { op: Token, seen: List<Seen>, more: u32 },  // what was there when the deadline passed
    Warned  { op: Token },                               // a go accepted its page's closing warning
    Done    { op: Token },                               // a navigation, a press, typing, a key
    Refused { op: Token, why: Refusal },                 // Gone, Hidden, Covered, Disabled, Crashed
    Snapshot   { op: Token, text: Box<[u8]> },
    Screenshot { op: Token, png: Box<[u8]> },
    Address { op: Token, url: Box<[u8]> },               // the history's current entry
    Attribute { op: Token, read: Read },
    Looked  { op: Token, looked: Looked },               // a look's answer (5.1)
    Trouble { page: Token, trouble: Trouble, text: Box<[u8]> },  // an exception, a console or log error, a crash, a dialog
    Closed  { owner: Token },                            // terminal
}

pub struct Query { role: Box<[u8]>, name: Box<[u8]>, matching: Match, within: Option<Node>, boxes: bool }
pub enum Match   { Exact, Words }                        // Words: the name contains the query's bytes
pub struct Point { x: u32, y: u32 }                      // whole CSS pixels from the box's top left
pub struct Seen  { node: Node, role: Box<[u8]>, name: Box<[u8]>, value: Box<[u8]>,
                   states: States, rect: Option<Rect> }   // focused, disabled, checked, expanded, selected
pub enum Expect  { Present, Count(u32), Absent }
pub enum Read    { NoElement, NoAttribute, Value(Box<[u8]>) }  // Value: whole, perhaps empty
```

- **A person is a browser context,** with its own cookies and storage.
  Two pages of one person share sign-ins and local storage, and each has
  its own session storage, as two tabs do.
- **A node is the browser's backend node id,** which names one element
  for as long as it lives. A node since removed is `Gone`, as a stale
  handle is.
- **Names match exactly or by words,** as the page computes them for
  assistive technology: an element's label, or its text. `Exact`: the
  name is the query's name. `Words`: the name contains the query's bytes,
  unbroken and with case kept, as a person finds a sentence by a few
  words in it. Other patterns are out (section 11). A query may be scoped
  `within` a node found before, such as a button within a card. `boxes`
  asks for each match's place on screen too.
- **Text is found by words,** as a static text node's name: Chromium
  gives each run of text a node of role `StaticText`, named by its text.
  Words such as "3 failing" find "Checks: 3 failing, 2 passed", and keep
  finding it while its other numbers change. Words match within one
  node's name: a sentence that markup splits, such as one with a strong
  word in it, is several nodes.
- **A press lands at its node's centre, or at a point the test names**
  within the node's box, in whole CSS pixels from its top left. A point
  reaches what has no accessible node of its own: a shape of an inline
  SVG, hidden from assistive technology, pressed through the named node
  that holds it.
- **A key may be pressed with Shift held,** as a person makes a new line
  with Shift+Enter in a text area, where Enter alone sends.
- **Select-all is a key of its own,** Control and A as the kit's
  platform takes them, as a person selects a field's whole value before
  typing over it: a `Type` after it replaces the value (the face's
  typing, ui-web.md 14.2). It is named by what it does, so that no
  other chord is asked for.
- **An attribute is read by a CSS selector,** of the first element it
  finds in the page's current document, as the document holds it. It is
  how a test reads what holds no role a person finds it by, such as a
  hidden field, whose `value` attribute is its value. It reads markup,
  not state: a field a person types into is read by a find, whose `Seen`
  carries its value as the person sees it. No element, an element
  without the attribute, and an empty attribute are three answers.
- **The current address** is the page's history's current entry, as the
  browser's address bar shows it, so that a test sees the address the
  page's own code put there.
- **Every operation has one terminal event:** `Found`, `Met`, `Missed`,
  `Done`, `Refused`, `Snapshot`, `Screenshot`, `Address`,
  `Attribute` or `Looked`. Every
  person, page and the browser itself ends with one `Closed`. `Warned` is
  not terminal: a go reports it at most once, before its terminal
  (section 6).

### 5.1 The look's reads

```rust
pub enum Look {
    Viewport { width: u32, height: u32, mobile: bool, scheme: Scheme, motion: Motion },
    Tree,                                                // the accessibility tree, as entries
    Select { selector: Box<[u8]> },                      // the first element a CSS selector finds
    Style  { node: Node, properties: List<Box<[u8]>> },  // computed values, faces, border box
    Shot   { clip: Option<Rect>, scale: u8 },            // none: the viewport; scale 1 or 2
}
pub enum Scheme { Light, Dark }
pub enum Motion { Reduce, NoPreference }

pub enum Looked {
    Viewport,
    Tree(List<Entry>),
    Selected(Option<Node>),
    Style { values: List<Property>, faces: List<Face>, rect: Option<Rect> },
    Shot(Box<[u8]>),                                     // PNG
}
pub struct Entry { node: Option<Node>, depth: u32, role: Box<[u8]>, name: Box<[u8]>, level: u32,
                   value: Box<[u8]>, states: States, multiline: bool }
pub struct Property { name: Box<[u8]>, value: Box<[u8]> }       // a computed value, whole
pub struct Face  { family: Box<[u8]>, custom: bool, glyphs: u32 }   // custom: a web font
```

- **Refused at once,** before any command: a style naming more than
  `Limits::properties` properties, or a property name or a selector
  past `Limits::text`, is refused `Limit`; a viewport or a clip without
  width or height, or a shot's scale other than 1 or 2, `Protocol`.
- **For the lab's browser alone** (lab-browser.md), which captures a
  page's look beside its mockup. No test sends `Look`: each test crate's
  `clippy.toml` names `skein_browser::boundary::Look` among its
  disallowed types, and a product's test crates do the same (section
  10). Every other request stays a person's.
- **A viewport** is the page's metrics at scale 1 (its width, height,
  and `mobile`, a phone's handling of the page's viewport tag) and its
  media: the colour scheme and the motion it prefers. It holds until the
  next, or the page's close. The lab opens a page at `about:blank`, sets
  its viewport, then goes to its address.
- **The tree** is the accessibility tree under the document, in document
  order, ignored nodes left out and their children lifted to their
  depth: each entry its backend node (none for a node without one), its
  depth from the root, its role, its name cut at `Limits::text`, and its
  level, a heading's or a tree item's, 0 for none; its value, cut the
  same, and its states, as a find's `Seen` has them, and whether a text
  box takes several lines (`multiline`), for the hand's outline
  (lab-browser.md, 8.4). Past `Limits::entries` it is refused `Limit`,
  never cut, since the lab reads it whole.
- **A select** finds an element by a CSS selector, as an attribute read
  does (section 5): for a mockup's part that has no role of its own.
  Its answer is the node, or none.
- **A style** reads a node's computed values for the properties named,
  in the order named, each whole, as the browser computes them (`16px`,
  `rgb(17, 17, 17)`), refused `Limit` past `Limits::text`, and empty for
  a property the browser does not compute; the faces that drew the text
  within the node, its descendants' among them, by family, whether a web font, and their glyphs; and its
  border box in the document's CSS pixels, none for a node without a box
  (one not displayed). Its faces past `Limits::entries` are refused
  `Limit`. A find's boxes are content
  boxes in the viewport, as a press needs them; a style's is what a crop
  needs.
- **A shot** is a PNG of the clip, in the document's CSS pixels, drawn
  at its scale, whether or not the clip is in the viewport; refused
  `Limit` past `Limits::screenshot`. Scale 2 draws the clip again at
  twice its pixels, as a crop to compare closely needs: no image is
  scaled up after it is drawn.
- **A shot draws the caret steady:** before its first shot of a
  document, the look adds the document a stylesheet of the inspector's
  (`CSS.createStyleSheet`, no page script) holding
  `* { caret-animation: manual !important }`, so a focused field's
  caret is drawn whole in every shot rather than in its blink's phase.
  The caret's colour and the field's computed values stay the page's;
  a later document gets its own, at its first shot.

## 6. The operations

Each operation is an entity with its own state, stepping from one reply
to the next command, never a closure waiting on a reply:

| Operation | Its steps |
|---|---|
| opening a page | create a target in the person's context, attach to it, enable the domains it reports on, set the viewport, navigate; `Opened` once the load event fires |
| go | navigate, reload, or move through the history; the page's `beforeunload` prompt, if it opens meanwhile, accepted, and `Warned`; `Done` once the navigation commits: the load event for a new document, the same-document event for one that stays |
| find | the accessibility nodes matching the role and the name, under the scope or the page's current document, either asked by its object in that document (below): exactly, both asked of CDP; by words, the role's nodes asked without a name, their names compared in the kit; at most `Limits::matches` kept, the rest counted in `more`; their properties; their boxes, if asked |
| await | a find, again every `Limits::poll`, until the expectation holds (`Met`) or `within` passes (`Missed`) |
| press | the node scrolled into view, its named point with it; its box's centre, or the named point in the box; layout metrics turn the viewport point into document coordinates for the hit test; refused `Hidden` without a box or with a point outside it, `Disabled` if its accessibility node says so, `Covered` unless the hit test at that point lands on the node or inside it; then the mouse moved, pressed and released at the viewport point |
| type | text with CR/LF first describes its node; for a single-line `input`, CR and LF are removed as on a person’s paste (CDP’s `Input.insertText` would change them to spaces). Text areas and editable content keep their line endings. Then the node is focused and the text inserted |
| compose | the node focused; a composition set and left open, which a later `Type` commits |
| keys | the node focused; for each character of the text, in order, its key down and up, the character its key and its text |
| key | the key down and up, with the text it makes; with `shift`, Shift's modifier on both; select-all, A's key down and up with Control's modifier and no text |
| address | the page's navigation history (`Page.getNavigationHistory`); its current entry's URL, `Address` |
| attribute | the current document's root (`DOM.getDocument`, its root alone); the first element under it the selector finds (`DOM.querySelector`), `NoElement` if none; its attributes (`DOM.getAttributes`), the named one's value, or `NoAttribute`; refused `Protocol` for a selector Chromium cannot parse, and `Limit` for a value past `Limits::text`, which is never cut, since a test uses it whole |
| snapshot | the accessibility tree under the document, as indented lines of role, name, value and states, cut at `Limits::snapshot` |
| screenshot | the viewport as PNG, under `Limits::screenshot` |
| look: viewport | `Emulation.setDeviceMetricsOverride` with scale 1, then `Emulation.setEmulatedMedia` with `prefers-color-scheme` and `prefers-reduced-motion`; `Looked` |
| look: tree | `Accessibility.getFullAXTree`; its nodes in document order from the root, by their children, with depth and level |
| look: select | as an attribute read's first two steps; the element's backend node, or none |
| look: style | the page's `CSS` domain enabled, once (it needs `DOM`'s, which every page has); the node pushed to the front end by its backend id (`DOM.pushNodesByBackendIdsToFrontend`, after `DOM.getDocument`); `CSS.getComputedStyleForNode`, the named properties kept; `CSS.getPlatformFontsForNode`; `DOM.getBoxModel`'s border quad and the layout metrics, for the document's box; refused `Gone` for a node since removed |
| look: shot | `Page.captureScreenshot` as PNG, its clip with its scale, beyond the viewport |

- **A find by words reads each name whole,** as the reply carries it,
  before a kept name is cut to `Limits::text`. Its reply holds every node
  of the role under its scope, and a reply past `Limits::message` fails
  the browser (section 4): a test scopes a find by words `within` a node,
  or raises the limit.
- **A find asks only under the current document.** Its root is never a
  node id, whose document the query does not check, but an object that
  Chromium checks against the document that holds it:
  - without a scope, the object of `document`, evaluated in the page's
    current execution context (section 4);
  - with a scope, the scope's object, resolved in the current document
    (`DOM.resolveNode`). Chromium refuses a node of an earlier document,
    and the find is refused `Gone` (section 5).

  The kit releases the object after the query. Chromium refuses an
  object once its execution context is gone, as when the page navigates
  by itself between the object's command and the query: the find then
  looks again at its next poll, and an `Await` keeps its deadline.
- **A find waits out a navigation.** While a navigation of the page is
  in flight, the kit's go or the page's own, the kit holds its DOM and
  accessibility commands to the page, and the evaluation of `document`,
  until the navigation commits, as a go's does. A find in flight when the
  navigation begins starts again, and asks once it has committed. An
  attribute read waits and starts again the same way, so that it reads
  one document from its root to its answer, and so do the look's tree,
  select and style; a look's viewport and shot do not wait.
- **A press at a point reaches a shape of an inline SVG.** The hit test
  there lands on the shape, inside the node that holds it, and the mouse
  goes down on the shape; something over the shape covers it, as it would
  cover a button. The kit checks only that the hit lands inside the node:
  where no shape is painted, such as inside a shape with no fill, the
  press goes to what is beneath, and the test sees that by its effect.
- **A press is done when the browser has taken it,** not when its
  effects show. What follows a press (a request, a page redrawn) the test
  awaits.
- **Covered is not a failure.** A dialog that has not closed yet, or a
  toast over a button, covers it for a moment. The test decides whether
  to wait and press again.
- **Typing inserts; keys press.** `type` inserts its text as one edit,
  and commits a composition left open. A native date or time field
  takes no inserted text. Each of its parts, a number or the half of
  the day, takes keys, and a part filled moves the focus to the next.
  `keys` reaches it as a person's keyboard does.
  - The kit types the characters it is given, in order. Which
    characters a field's parts take, and in which order, is the
    test's: the browser's locale sets them (section 3), and the face
    checks them as its facts (ui-web.md, 14.1 and 14.2).
  - A key goes where the focus is. Focusing a field that does not have
    the focus puts it on the field's first part; a field that has it
    keeps it in the part it is in.
- **A go answers its page's closing warning.** A person who navigates,
  reloads or moves back or forward has chosen to leave the document. So
  the kit accepts a `beforeunload` prompt that opens on the page while a
  go of it is in flight (CDP's `Page.javascriptDialogOpening`, answered
  by `Page.handleJavaScriptDialog`), and reports `Warned` before the
  go's `Done`.
  - A test sees by `Warned`, or by its absence, whether the page warned
    when the person left it.
  - Chromium shows the prompt only on a page that has had a person's
    input since it loaded. A test that looks for no warning acts on the
    page first.
  - Closing a page closes its target, which runs no `beforeunload`
    handler, so a close is never warned.
  - No request answers a dialog. A person who stays on a page that
    warns, or who answers a confirm, waits for a test that needs one.
- **Every command has a deadline,** `Limits::answer`. A browser that does
  not answer in time fails the operation with what was pending, so a
  hung browser fails a test rather than holding it up.

## 7. Trouble

The kit enables, on every page, the events that say something went
wrong, and reports each as `Trouble` with its text, cut to
`Limits::text`:

- an exception thrown and not caught;
- a console message at error level;
- a log entry at error level, which is how a content security policy
  violation, a failed load or a blocked request shows;
- the page crashing, which also fails its operations in flight with
  `Crashed`;
- a native dialog, other than the closing warning a go accepts (section
  6): an alert, a confirm, a prompt, or a `beforeunload` prompt with no
  go of its page in flight. The kit dismisses it at once. Its text is
  the dialog's type, `: `, and its message (`confirm: Delete it?`).

A dismissed dialog commits the page to nothing: a confirm answers no, a
prompt gives nothing, and a `beforeunload` prompt keeps the page where
it is. The page goes on, and the test sees it as it stayed. Left open,
the dialog would hold every operation on the page until its deadline,
and no request would answer it.

The kit judges none of them. A test fails on any `Trouble` it did not
expect, and a scenario that provokes one (a page made to fail, for
example) expects it. That is the test's harness, not the kit.

## 8. Limits and memory

`Limits` bound persons, pages, operations in flight, commands in flight,
a message each way, matches kept per query, the bytes of a name or
value, a snapshot, a screenshot, a look's tree entries and properties
(`Limits::entries`, `Limits::properties`), a `Trouble`'s text and
stderr's tail.
They also set the poll interval and the answer deadline. Commands are
sized before they are written, and replies are kept only as their
decoders' fields. `worst_case` adds the slabs, the command table, the
queues, one message being read, the largest command and the largest
screenshot, and a look's largest answer; no size computation may wrap. The memory test drives the
kit to its limits against the fake browser and checks the counting
allocator's high water against `worst_case`.

## 9. Testing

- **Step tests:** framing (a message split across reads, several in one
  read, one past the limit), command encoding, bounded decoding, and
  operation transitions use synthetic CDP replies. The separate browser
  suite checks those assumptions against Chromium.
- **The machine world:** the kit against a fake browser, a scripted CDP
  peer over in-memory pipes. Its pages and accessibility trees come from
  the scenario, and the seed drives what goes wrong:
  - nodes that move, are covered, are disabled or vanish between the
    steps of a press;
  - replies that come late, interleaved with events, or never;
  - a page or the browser crashing in the middle of an operation;
  - a message past the limit.

  The look's reads are there too: a viewport, a tree with ignored nodes
  and headings, a select, a style of a node that vanishes between its
  steps, a shot past the limit.

  A referee holds the contracts: one terminal per operation, nothing
  after `Closed`, every command answered or failed, and a press never
  dispatched on a node that was refused. A few seeds run in the focused
  suite, and many in the fuzzy one.
- **The real browser:** the kit in the real loop against Chromium, opening
  fixed pages served by a small `skein-http` server in the same loop: a
  button that changes a heading, a form, a dialog that covers a button, a
  button far down a long page, a script that throws, a resource the content
  security policy blocks, a sentence whose text is found by a few of its
  words, a named figure holding an inline SVG hidden from assistive
  technology, whose shape, pressed through the figure at a point, changes a
  heading, and a page that holds a closing warning once a button is pressed
  and asks a confirm at another's press, and a page that reloads itself.
  These tests check the facts the fake browser assumes: the framing,
  flattened sessions, what the accessibility queries match, with a name and
  without one, boxes and the hit test, on an element and on a shape of an
  SVG, the events each step waits for, the dialogs (a reload accepting the
  closing warning, a close meeting none, a confirm dismissed), and a find
  across a reload (a page that reloads itself found again after it, a scope
  found before it `Gone`). The look's reads have theirs: a page at a phone's
  viewport whose media queries show the scheme and the motion it was given,
  by a part they name; a tree's depths and its headings' levels; a
  selector's node, and none; a styled part's computed values, its faces (a
  web font served by the fixture, DejaVu Sans from the fonts' directory the
  lab's browser's story names (lab-browser.md, 6), and a fallback) and its
  box; and a part's shot at scale 2, twice its box's size, far down the
  page. The keys a native date or time field takes, and their order, are the
  face's facts, checked with its others in this suite (ui-web.md, 14.1). A
  new Chromium version is a run of these, and a fact that changed is fixed
  in the fake first. They need Chromium and do not replay, so they form a
  suite of their own, beside the focused and fuzzy suites (`--profile
  browser`, within four minutes (two minutes before the user's doubling of
  10-22, provisional), which the browser parts' tier shares: ui-web.md,
  14.1). A missing Chromium fails that suite, naming what is missing; it is
  never a skip.

The kit's tests are a crate of their own, `tests/browser`
(`skein-browser-world`) (testing.md, 6).

## 10. What it asks of others

- **io:** a spawn that makes pipes beyond the standard three, as a list of
  the child's descriptors, each with its direction (io.md, 6).
- **A service using it:**
  - spawning the browser with `command`, and owning the child, its pipes
    and the scratch root its profile is made in;
  - Chromium's path, in its configuration;
  - a face that turns its person's actions into the kit's requests;
  - a harness that fails a test on unexpected `Trouble` and writes the
    snapshot, the screenshot and stderr's tail into the failure report;
  - `skein_browser::boundary::Look` among its test crates' disallowed
    types (5.1).

  An application on skein-ui takes the face and the harness from
  `skein-fake-person-browser` (ui-web.md, 14.2).

## 11. Open questions

- **Chromium's own sandbox** under a service's containment: whether the
  namespaces it needs are there, or the real loop's sandbox stands in for
  it.
- **Other browsers.** CDP is Chromium's; Firefox and WebKit would need
  WebDriver BiDi, a second wire, if a bug ever shows only there.
- **Text by pattern beyond words,** should words in a name prove too
  strict for text that carries numbers or times: case folded, or a number
  or a time matched by its shape rather than its digits.
