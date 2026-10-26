//! The reference shell: start the parts, carry reports, apply the view's patches.
#![cfg(target_arch = "wasm32")]
#![forbid(unsafe_code)]
use skein_lib::Queue;
use skein_ui::{Act, NodeId, Owner, Patch, Role, Style, View, look::Names};
use skein_ui_example_view::{Duties, Report as ViewReport, State, limits, places};
use skein_ui_web::{
    Look, fetch, history,
    inbox::Inbox,
    page::{Page, web::Dom},
    platform::{self, Platform},
    store::Store,
    timer, title,
    transport::{self, Carried, Requests},
    wasm_bindgen,
};
use std::{cell::RefCell, rc::Rc};

mod grow;
#[cfg(debug_assertions)]
mod tree;

const NAMES: &[Names<'_>] = &[Names {
    owner: Owner { number: 1 },
    prefix: b"notes",
    styles: &[b"notes-card", b"notes-bar", b"notes-grow"],
    regions: &[],
}];
// Each part wraps its happening in the application's report type.
enum Report {
    Start,
    Due,
    Act(NodeId, Act),
    Fetch(fetch::Report),
    Carried(Carried),
    Place(Box<[u8]>),
}
struct Shell {
    state: State,
    view: View,
    patches: Queue<Patch>,
    page: Rc<RefCell<Page<'static>>>,
    dom: Dom,
    platform: platform::Browser<Report>,
    store: Store,
    history: history::web::History,
    timer: timer::web::Timer,
    title: title::web::Title,
}

#[wasm_bindgen(start)]
pub fn start() {
    let window = web_sys::window().expect("browser window");
    let document = window.document().expect("browser document");
    let mount = document.get_element_by_id("app").expect("the bundle supplies #app");
    if !skein_ui_web::restart::Restart::new(
        3,
        skein_lib::Duration::from_secs(60),
        "skein-ui-restarts",
        "The application failed.",
    )
    .install("app")
    {
        return;
    }
    // Read the person's facts once. Later turns read only the two clocks.
    let facts = platform::facts();
    let location = window.location();
    let path = location.pathname().expect("page path") + &location.search().expect("page query");
    #[cfg(debug_assertions)]
    if path.starts_with("/test/tree/") {
        tree::start(mount, &path, facts.primary);
        return;
    }
    let inbox = Inbox::new();
    let fetch = fetch::web::Fetch::new(9, Rc::clone(&inbox), Report::Fetch);
    let platform = platform::Browser::new(fetch);
    let bound = None;
    #[cfg(debug_assertions)]
    let bound = if path.split('?').next() == Some("/test/small-store") { Some(0) } else { bound };
    let store = Store::new(b"notes-", bound);
    let records: Vec<_> = store
        .read_all(&platform)
        .into_iter()
        .map(|record| skein_ui_example_view::Record { key: record.key, value: record.value })
        .collect();
    let state = State::start(facts.seed, path.as_bytes(), &records, facts.offset, &facts.languages);
    let limits = limits::limits();
    let page = Rc::new(RefCell::new(Page::new(
        Look {
            names: NAMES,
            pictures: &[],
            components: &[(Owner { number: 1 }, Role::TextArea, Style { owner: Owner { number: 1 }, token: 2 })],
            draw: skein_ui_example_view::draw::draw,
            elements: &[(Owner { number: 1 }, Role::Region, Style { owner: Owner { number: 1 }, token: 0 }, "article")],
        },
        &limits,
        b"notes",
        facts.primary,
    )));
    let dom = Dom::new(mount, &page, &inbox, Report::Act, &[grow::new]);
    let history = history::web::History::new(Rc::clone(&inbox), Report::Place);
    let timer = timer::web::Timer::new(Rc::clone(&inbox), || Report::Due);
    let title = title::web::Title::default();
    let mut shell = Shell {
        state,
        view: View::new(&limits),
        patches: Queue::with_capacity(limits.patches),
        page,
        dom,
        platform,
        store,
        history,
        timer,
        title,
    };
    // The inbox keeps the shell and its listeners alive and forbids a turn inside a turn.
    inbox.start(Box::new(move |inbox| shell.turn(inbox)));
    inbox.raise(Report::Start);
}
impl Shell {
    fn turn(&mut self, inbox: &Inbox<Report>) {
        let (now, wall) = platform::clocks();
        let mut new_page = false;
        while let Some(report) = inbox.next() {
            let mut read_on = false;
            let report = match report {
                Report::Start => Some(ViewReport::Due),
                Report::Due => {
                    self.timer.fired();
                    Some(ViewReport::Due)
                }
                Report::Place(path) => Some(ViewReport::Place(places::parse(&path))),
                Report::Act(node, act) => self.view.act(node, act).map(ViewReport::Act),
                Report::Fetch(fetch::Report::Answered { exchange, status, body }) => {
                    Some(ViewReport::Answered { exchange, status, body })
                }
                Report::Fetch(fetch::Report::Failed { exchange }) | Report::Carried(Carried::Failed { exchange }) => {
                    Some(ViewReport::Failed { exchange })
                }
                Report::Fetch(fetch::Report::StreamOpened) => {
                    read_on = true;
                    Some(ViewReport::StreamOpened)
                }
                Report::Fetch(fetch::Report::StreamPiece { bytes }) => {
                    read_on = true;
                    Some(ViewReport::StreamPiece { bytes })
                }
                Report::Fetch(fetch::Report::StreamRefused { .. } | fetch::Report::StreamEnded) => {
                    Some(ViewReport::StreamEnded)
                }
                Report::Carried(Carried::Written { .. }) => None,
                Report::Carried(Carried::Refused { .. }) => Some(ViewReport::Failed { exchange: 0 }),
            };
            let Some(report) = report else { continue };
            let mut duties = Duties {
                writes: Queue::with_capacity(8),
                removes: Queue::with_capacity(8),
                exchanges: Queue::with_capacity(8),
                show: None,
                stream: None,
            };
            self.state.step(report, now, wall, &mut duties);
            if let Some((place, replace)) = duties.show {
                self.history.show(&places::bytes(place), replace);
                new_page = !replace;
            }
            let mut requests = Requests::default();
            while let Some(record) = duties.writes.pop() {
                requests.writes.push(skein_ui_web::store::Record { key: record.key, value: record.value });
            }
            while let Some(key) = duties.removes.pop() {
                requests.removes.push(key);
            }
            while let Some(exchange) = duties.exchanges.pop() {
                requests.exchanges.push(fetch::Exchange {
                    id: exchange.id,
                    path: exchange.path,
                    method: if exchange.post { "POST" } else { "GET" }.into(),
                    headers: vec![("Content-Type".into(), "application/octet-stream".into())],
                    body: exchange.body,
                    bound: exchange.bound,
                });
            }
            requests.stream = duties.stream.map(|duty| match duty {
                skein_ui_example_view::StreamDuty::Open { path, headers, bound } => {
                    transport::StreamDuty::Open { path, headers: headers.into_vec(), bound }
                }
                skein_ui_example_view::StreamDuty::Close => transport::StreamDuty::Close,
            });
            let mut carried = Vec::new();
            transport::carry(requests, &mut self.store, &mut self.platform, &mut carried);
            for report in carried {
                inbox.raise(Report::Carried(report));
            }
            if read_on {
                self.platform.read_on();
            }
        }
        // Build once after all waiting reports; document work can raise the next turn's reports.
        self.state.build(&mut self.view, &mut self.patches);
        let patches: Vec<_> = std::iter::from_fn(|| self.patches.pop()).collect();
        let mut ops = Vec::new();
        self.page.borrow_mut().apply(&patches, &mut ops);
        // Give each link the codec's path, so the browser can follow it without acting.
        let links = self.page.borrow().links().to_vec();
        for node in links {
            if let Some(place) = self.view.address(node).and_then(|binding| self.state.address(binding)) {
                self.page.borrow_mut().set_href(node, &places::bytes(place), &mut ops);
            }
        }
        self.dom.apply(&ops);
        self.finish_page(&ops, new_page, now);
    }
    fn finish_page(&mut self, ops: &[skein_ui_web::page::Op], new_page: bool, now: skein_lib::Time) {
        let window = web_sys::window().expect("window");
        let document = window.document().expect("document");
        let heading = document.query_selector("main h1").expect("heading selector");
        let words = heading.as_ref().and_then(|element| element.text_content()).unwrap_or_default();
        self.title.set(words.as_bytes(), b"Notes", b"en", self.state.closing_loses());
        if new_page {
            window.scroll_to_with_x_and_y(0.0, 0.0);
            if !ops.iter().any(|op| matches!(op, skein_ui_web::page::Op::Focus { .. })) {
                use wasm_bindgen::JsCast;
                if let Some(target) = heading.or_else(|| document.query_selector("main").expect("main selector")) {
                    target.set_attribute("tabindex", "-1").expect("focus target");
                    target.unchecked_ref::<web_sys::HtmlElement>().focus().expect("mounted target");
                }
            }
        }
        self.history.restore();
        self.timer.set(self.state.deadline(), now);
    }
}
