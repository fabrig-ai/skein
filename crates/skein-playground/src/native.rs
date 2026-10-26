//! The bridge's encoded messages carried in one thread (playground.md, 5).
use crate::slices::{Composition, Slices};
use skein_lib::{Time, Wall};
use skein_sim_world::World;
use skein_ui_web::{
    bridge::{Start, ToWorld, decode_to_world, encode_to_page},
    fetch::{Exchange, Report},
    platform::{
        Platform,
        worker::{Bridge, Figures, Heard, Limits, Memory, Speed},
    },
    store::Record,
    transport::StreamDuty,
};
use std::collections::VecDeque;

/// A page and its world on a browser clock its caller moves.
pub struct Native<C: Composition> {
    page: Bridge,
    slices: Slices<C>,
    browser: Time,
    heard: VecDeque<Heard>,
    memory: Memory,
    first: bool,
}

impl<C: Composition> Native<C> {
    #[must_use]
    pub fn new(
        world: World<C::Process, C::Machine>,
        start: Start,
        budget: u32,
        build: &[u8],
        limits: Limits,
        speed: Speed,
    ) -> Self {
        let mut page = Bridge::new(start, build, limits, speed, Time::ZERO);
        let first = page.outgoing().expect("start is first");
        assert!(matches!(decode_to_world(&first).expect("encoded start"), ToWorld::Start(_)), "start is first");
        Self {
            page,
            slices: Slices::new(world, budget, speed, Time::ZERO),
            browser: Time::ZERO,
            heard: VecDeque::new(),
            memory: Memory::default(),
            first: true,
        }
    }

    /// Carry every message, then each slice due at the browser's instant.
    pub fn run(&mut self) {
        loop {
            self.carry();
            while let Some(message) = self.slices.outgoing() {
                let heard = self.page.incoming(&encode_to_page(&message), self.browser).expect("encoded world message");
                self.heard.push_back(heard);
            }
            let first = std::mem::take(&mut self.first);
            if !first && self.wake().is_none_or(|due| due > self.browser) {
                break;
            }
            self.slices.slice(self.browser);
        }
    }

    /// The page's session log, kept independently of the world.
    #[must_use]
    pub fn log(&self) -> Box<[u8]> {
        self.page.log()
    }

    /// The world's figures; the native loop reports zeros.
    #[must_use]
    pub fn figures(&self) -> Figures {
        self.page.figures()
    }

    #[must_use]
    pub fn browser(&self) -> Time {
        self.browser
    }
    #[must_use]
    pub fn wake(&self) -> Option<Time> {
        if self.first { Some(self.browser) } else { self.slices.wake(self.browser) }
    }
    pub fn advance(&mut self, to: Time) {
        assert!(to >= self.browser, "browser time never moves backwards");
        self.browser = to;
    }
    pub fn heard(&mut self) -> Option<Heard> {
        self.heard.pop_front()
    }
    pub fn now(&mut self) -> (Time, Wall) {
        self.page.now(self.browser)
    }
    #[must_use]
    pub fn timer(&self, deadline: Time) -> Option<Time> {
        self.page.timer(deadline)
    }
    pub fn pace(&mut self, speed: Speed) {
        self.page.pace(speed, self.browser);
        self.carry();
    }
    #[must_use]
    pub fn slices(&self) -> &Slices<C> {
        &self.slices
    }
    fn carry(&mut self) {
        while let Some(bytes) = self.page.outgoing() {
            self.slices.heard(decode_to_world(&bytes).expect("encoded page message"), self.browser);
        }
    }
    fn report(&mut self, report: Option<Report>) {
        if let Some(report) = report {
            self.heard.push_back(Heard::Report(report));
        }
    }
}

impl<C: Composition> Platform for Native<C> {
    fn read_all(&self) -> Vec<Record> {
        self.memory.read_all()
    }
    fn write(&mut self, key: &[u8], value: &[u8]) -> bool {
        self.memory.write(key, value)
    }
    fn remove(&mut self, key: &[u8]) {
        self.memory.remove(key);
    }
    fn send(&mut self, exchange: &Exchange) {
        let report = self.page.send(exchange);
        self.report(report);
        self.carry();
    }
    fn abandon(&mut self, exchange: u32) {
        self.page.abandon(exchange);
        self.carry();
    }
    fn read_on(&mut self) {
        let report = self.page.read_on();
        self.report(report);
        self.carry();
    }
    fn navigate(&mut self, to: &[u8]) {
        // There is no browser location to change in the bridge in memory.
        let _allowed = skein_ui_web::platform::worker::navigable(to);
    }
    fn stream(&mut self, duty: StreamDuty) {
        let report = self.page.stream(duty);
        self.report(report);
        self.carry();
    }
}
