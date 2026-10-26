use std::cell::RefCell;
use std::rc::Rc;

use crate::inbox::Inbox;

#[derive(Debug, PartialEq, Eq)]
pub enum Call {
    Write(Vec<u8>, Vec<u8>),
    Remove(Vec<u8>),
    Send(u32),
    Abandon(u32),
    ReadOn,
    Navigate(Vec<u8>),
    Stream(crate::transport::StreamDuty),
}
#[derive(Default)]
pub struct FakePlatform {
    pub records: Vec<crate::store::Record>,
    pub calls: Vec<Call>,
    pub quota: Option<usize>,
}
impl crate::platform::Platform for FakePlatform {
    fn read_all(&self) -> Vec<crate::store::Record> {
        self.records.clone()
    }
    fn write(&mut self, key: &[u8], value: &[u8]) -> bool {
        self.calls.push(Call::Write(key.into(), value.into()));
        let held: usize = self
            .records
            .iter()
            .filter(|record| record.key.as_ref() != key)
            .map(|record| record.key.len() + record.value.len())
            .sum();
        if self.quota.is_some_and(|quota| held + key.len() + value.len() > quota) {
            return false;
        }
        self.records.retain(|record| record.key.as_ref() != key);
        self.records.push(crate::store::Record { key: key.into(), value: value.into() });
        true
    }
    fn remove(&mut self, key: &[u8]) {
        self.calls.push(Call::Remove(key.into()));
        self.records.retain(|record| record.key.as_ref() != key);
    }
    fn send(&mut self, exchange: &crate::fetch::Exchange) {
        self.calls.push(Call::Send(exchange.id));
    }
    fn abandon(&mut self, exchange: u32) {
        self.calls.push(Call::Abandon(exchange));
    }
    fn read_on(&mut self) {
        self.calls.push(Call::ReadOn);
    }
    fn stream(&mut self, duty: crate::transport::StreamDuty) {
        self.calls.push(Call::Stream(duty));
    }
    fn navigate(&mut self, to: &[u8]) {
        self.calls.push(Call::Navigate(to.into()));
    }
}

pub fn exchange(id: u32) -> crate::fetch::Exchange {
    crate::fetch::Exchange {
        id,
        path: b"/api/notes".as_slice().into(),
        method: "POST".into(),
        headers: Vec::new(),
        body: b"record".as_slice().into(),
        bound: 1024,
    }
}

pub fn collecting() -> (Rc<Inbox<u8>>, Rc<RefCell<Vec<u8>>>) {
    let inbox = Inbox::new();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let output = Rc::clone(&seen);
    inbox.start(Box::new(move |inbox| {
        while let Some(report) = inbox.next() {
            output.borrow_mut().push(report);
        }
    }));
    (inbox, seen)
}

pub const OWNER: skein_ui::Owner = skein_ui::Owner { number: 1 };
pub const NAMES: &[skein_ui::look::Names<'_>] =
    &[skein_ui::look::Names { owner: OWNER, prefix: b"test", styles: &[b"test-mark"], regions: &[b"test-region"] }];
pub const LIMITS: skein_ui::Limits = skein_ui::Limits {
    nodes: 32,
    depth: 8,
    text_per_node: 256,
    text_bytes: 4096,
    drawing_bytes: 128,
    styles_per_node: 4,
    patches: 128,
    bindings: 32,
    slots: 4,
    markdown_depth: 8,
    scene_marks: 16,
    scene_bytes: 256,
};

pub fn made(role: skein_ui::Role) -> skein_ui::Made {
    let mut builder = skein_ui::Builder::new(&LIMITS);
    builder.open(role);
    builder.close();
    let tree = builder.finish();
    let mut made = skein_ui::Made::of(tree.get(0).expect("fixture root"));
    made.id = skein_ui::NodeId(1);
    made
}
pub fn look() -> crate::Look<'static> {
    crate::Look {
        components: &[],
        elements: &[],
        names: NAMES,
        pictures: &[(OWNER, &[b"picture.png"])],
        draw: no_drawing,
    }
}
fn no_drawing(_: skein_ui::DrawingKind, _: &[u8], _: u16, _: u16, _: &mut skein_ui::scene::Scene) -> bool {
    false
}

pub fn patches(
    view: &mut skein_ui::View,
    shown: u64,
    build: impl FnOnce(&mut skein_ui::Builder),
) -> Vec<skein_ui::Patch> {
    let mut builder = view.builder();
    build(&mut builder);
    let mut out = skein_lib::Queue::with_capacity(LIMITS.patches);
    view.finish(builder, shown, &mut out);
    let mut patches = Vec::new();
    while let Some(patch) = out.pop() {
        patches.push(patch);
    }
    patches
}

pub fn binding(number: u64) -> skein_ui::Binding {
    skein_ui::Binding { owner: OWNER, token: skein_lib::Token::new(number) }
}

pub fn drawing_look() -> crate::Look<'static> {
    crate::Look { draw: drawing, ..look() }
}
fn drawing(_: skein_ui::DrawingKind, data: &[u8], width: u16, height: u16, scene: &mut skein_ui::scene::Scene) -> bool {
    use skein_ui::scene::{Anchor, Point};
    if data == b"refuse" {
        scene.text(Point { x: 0, y: 0 }, Anchor::Start, &[b'x'; 300], &[]);
        return true;
    }
    let radius = if data == b"two" { 2 } else { 1 };
    scene.rect(Point { x: 0, y: 0 }, Point { x: width, y: height }, radius, &[])
}
