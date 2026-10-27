//! Browser-specific script steps over one semantic face.
use super::{Play, ServerAction};
use crate::BrowserFace;
use skein_fake_person::{Next, Person};
use skein_lib::{Duration, Time};
use std::collections::VecDeque;
use std::fmt::Write as _;
mod growth;

pub enum Step {
    Run(super::Command),
    Exited {
        code: i32,
        within: Duration,
    },
    Person(skein_fake_person::Step<Box<[u8]>>),
    Keep {
        selector: Box<[u8]>,
        attribute: Option<Box<[u8]>>,
        name: Box<[u8]>,
    },
    StopServer,
    StartServer,
    Title(Box<[u8]>),
    Warned(bool),
    Key(skein_browser::boundary::Key),
    KeyWithShift(skein_browser::boundary::Key),
    Focused(skein_fake_person::Find),
    Taller {
        find: skein_fake_person::Find,
        words: Box<[u8]>,
    },
    PressAt {
        find: skein_fake_person::Find,
        at: skein_browser::boundary::Point,
    },
    Compose {
        find: skein_fake_person::Find,
        words: Box<[u8]>,
    },
    Type {
        find: skein_fake_person::Find,
        words: Box<[u8]>,
    },
    Trouble {
        words: Box<[u8]>,
        within: Duration,
    },
    Request {
        method: skein_http::Method,
        path: Box<[u8]>,
        host: Box<[u8]>,
        headers: Vec<skein_ui_web::transport::Header>,
        body: Box<[u8]>,
        status: u16,
        within: Duration,
    },
    Reopen,
    Visit {
        origin: u32,
        path: Box<[u8]>,
    },
    Back,
    Forward,
    /// Read the page's address; the story fails unless its path, query kept, is this one.
    Address(Box<[u8]>),
}
enum Awaited {
    Trouble(Box<[u8]>),
    Status(u16),
}
pub struct Script {
    steps: VecDeque<Step>,
    person: Option<Person<Box<[u8]>>>,
    at: u32,
    action: Option<ServerAction>,
    title: Option<Box<[u8]>>,
    keeping: bool,
    commanding: bool,
    focused: Option<skein_fake_person::Find>,
    growth: Option<growth::Growth>,
    press: Option<(skein_fake_person::Find, skein_browser::boundary::Point)>,
    awaited: Option<(Time, Awaited)>,
    request: Option<super::Asked>,
    status: Option<u16>,
    request_failure: Option<super::RequestFailure>,
    text: Option<(skein_fake_person::Find, Box<[u8]>, bool)>,
}
impl Script {
    #[must_use]
    pub fn new(steps: Box<[Step]>) -> Self {
        Self {
            steps: steps.into_vec().into(),
            person: None,
            at: 0,
            action: None,
            title: None,
            keeping: false,
            commanding: false,
            focused: None,
            press: None,
            growth: None,
            text: None,
            awaited: None,
            request: None,
            status: None,
            request_failure: None,
        }
    }
    fn await_step(&mut self, face: &mut BrowserFace<'_>, now: Time) -> bool {
        let Some((until, awaited)) = self.awaited.take() else {
            return false;
        };
        let (met, words) = match &awaited {
            Awaited::Trouble(words) => (
                face.trouble()
                    .iter()
                    .any(|text| !words.is_empty() && text.windows(words.len()).any(|part| part == words.as_ref())),
                format!("trouble containing {}", String::from_utf8_lossy(words)),
            ),
            Awaited::Status(status) => {
                let mut words = format!("status {status}, saw {:?}", self.status);
                if let Some(failure) = self.request_failure {
                    write!(words, "; request failed: {failure:?}").expect("string write");
                } else if self.status.is_none() && now >= until {
                    words.push_str("; no response before the request deadline");
                }
                (self.status == Some(*status), words)
            }
        };
        if met {
            self.at += 1;
            self.status = None;
            self.request_failure = None;
        } else if now >= until {
            face.fail(format!("did not receive {words} within its bound"));
            self.at += 1;
        } else {
            self.awaited = Some((until, awaited));
        }
        true
    }
    fn check_title(&mut self, face: &mut BrowserFace<'_>) -> Option<Next<Box<[u8]>>> {
        if let Some(title) = self.title.take() {
            match face.title(&title) {
                skein_fake_person::Found::Pending => {
                    self.title = Some(title);
                    return Some(Next::Wait { until: None });
                }
                skein_fake_person::Found::Absent => {
                    return Some(Next::Failed {
                        step: self.at,
                        why: skein_fake_person::Why::Missing {
                            find: skein_fake_person::Find::exactly_named(skein_ui::Role::Page, &title),
                        },
                    });
                }
                skein_fake_person::Found::Node(_) => {
                    self.at += 1;
                }
            }
        }
        None
    }
    fn grow(&mut self, face: &mut BrowserFace<'_>, now: Time) -> Option<Next<Box<[u8]>>> {
        if let Some(mut growth) = self.growth.take() {
            if growth.next(face, now) {
                self.at += 1;
            } else {
                self.growth = Some(growth);
            }
            return Some(Next::Wait { until: None });
        }
        None
    }
    fn press_at(&mut self, face: &mut BrowserFace<'_>) -> Option<Next<Box<[u8]>>> {
        let (find, at) = self.press.take()?;
        match skein_fake_person::Face::find(face, &find) {
            skein_fake_person::Found::Pending => self.press = Some((find, at)),
            skein_fake_person::Found::Absent => {
                return Some(Next::Failed { step: self.at, why: skein_fake_person::Why::Missing { find } });
            }
            skein_fake_person::Found::Node(node) => {
                if let Err(why) = face.press_at(node, at) {
                    face.fail(why);
                }
                self.at += 1;
            }
        }
        Some(Next::Wait { until: None })
    }
    fn check_focus(&mut self, face: &mut BrowserFace<'_>) -> Option<Next<Box<[u8]>>> {
        if let Some(find) = self.focused.take() {
            match skein_fake_person::Face::find(face, &find) {
                skein_fake_person::Found::Pending => {
                    self.focused = Some(find);
                    return Some(Next::Wait { until: None });
                }
                skein_fake_person::Found::Absent => {
                    return Some(Next::Failed { step: self.at, why: skein_fake_person::Why::Missing { find } });
                }
                skein_fake_person::Found::Node(node) => {
                    if !face.focused(node) {
                        face.fail(format!("node {find:?} is not focused"));
                    }
                    self.at += 1;
                    return Some(Next::Wait { until: None });
                }
            }
        }
        None
    }
}
impl Play for Script {
    #[expect(clippy::too_many_lines, reason = "one match lists the script's steps")]
    fn next(&mut self, face: &mut BrowserFace<'_>, now: Time) -> Next<Box<[u8]>> {
        if self.commanding {
            if face.command_done() {
                self.commanding = false;
                self.at += 1;
            }
            return Next::Wait { until: None };
        }
        if self.keeping {
            if face.kept_done() {
                self.keeping = false;
                self.at += 1;
            }
            return Next::Wait { until: None };
        }
        if self.await_step(face, now) {
            return Next::Wait { until: None };
        }
        if let Some(next) = self.grow(face, now) {
            return next;
        }
        if let Some(next) = self.press_at(face) {
            return next;
        }
        if let Some((find, words, compose)) = self.text.take() {
            match skein_fake_person::Face::find(face, &find) {
                skein_fake_person::Found::Pending => self.text = Some((find, words, compose)),
                skein_fake_person::Found::Absent => {
                    return Next::Failed { step: self.at, why: skein_fake_person::Why::Missing { find } };
                }
                skein_fake_person::Found::Node(node) => {
                    if let Err(why) = face.text(node, words, compose) {
                        face.fail(why);
                    }
                    self.at += 1;
                }
            }
            return Next::Wait { until: None };
        }
        if let Some(next) = self.check_focus(face) {
            return next;
        }
        if let Some(next) = self.check_title(face) {
            return next;
        }
        if let Some(person) = &mut self.person {
            let next = person.next(face, now);
            match next {
                Next::Done => {
                    self.at += person.at();
                    self.person = None;
                }
                Next::Failed { step, why } => return Next::Failed { step: self.at + step, why },
                other @ (Next::Do(_) | Next::Wait { .. }) => return other,
            }
        }
        let Some(step) = self.steps.pop_front() else {
            return Next::Done;
        };
        match step {
            Step::Key(key) | Step::KeyWithShift(key) => {
                face.key(key, matches!(step, Step::KeyWithShift(_)));
                self.at += 1;
            }
            Step::Run(command) => {
                face.command(super::CommandAction::Run(command));
                self.commanding = true;
            }
            Step::Exited { code, within } => {
                face.command(super::CommandAction::Exit { code, within });
                self.commanding = true;
            }
            Step::Keep { selector, attribute, name } => {
                face.keep(selector, attribute, name);
                self.keeping = true;
            }
            Step::Address(path) => {
                face.read_address(&path);
                self.at += 1;
            }
            Step::Taller { find, words } => self.growth = Some(growth::Growth::new(find, words)),
            Step::PressAt { find, at } => self.press = Some((find, at)),
            Step::Focused(find) => self.focused = Some(find),
            Step::Compose { find, words } => self.text = Some((find, words, true)),
            Step::Type { find, words } => self.text = Some((find, words, false)),
            Step::Trouble { words, within } => {
                self.awaited = Some((now.saturating_add(within), Awaited::Trouble(words)));
            }
            Step::Request { method, path, host, headers, body, status, within } => {
                self.request = Some(super::Asked { method, path, host, headers, body });
                self.awaited = Some((now.saturating_add(within), Awaited::Status(status)));
            }
            Step::Visit { origin, path } => {
                face.visit(origin, &path);
                self.at += 1;
            }
            Step::Reopen => {
                face.reopen();
                self.at += 1;
            }
            Step::Back | Step::Forward => {
                face.history(if matches!(step, Step::Forward) {
                    skein_browser::boundary::Go::Forward
                } else {
                    skein_browser::boundary::Go::Back
                });
                self.at += 1;
            }
            Step::Person(step) => {
                let mut steps = vec![step];
                while matches!(self.steps.front(), Some(Step::Person(_))) {
                    let Some(Step::Person(step)) = self.steps.pop_front() else { unreachable!("person step") };
                    steps.push(step);
                }
                self.person = Some(Person::new(steps.into()));
            }
            Step::Warned(expected) => {
                let warned = face.warned();
                if warned != expected {
                    face.fail(format!("last go warned {warned}, expected {expected}"));
                }
                self.at += 1;
            }
            Step::Title(words) => self.title = Some(words),
            Step::StopServer => {
                self.action = Some(ServerAction::Stop);
                self.at += 1;
            }
            Step::StartServer => {
                self.action = Some(ServerAction::Start);
                self.at += 1;
            }
        }
        Next::Wait { until: None }
    }
    fn expected(&self) -> Vec<Box<[u8]>> {
        self.steps
            .iter()
            .filter_map(|step| if let Step::Trouble { words, .. } = step { Some(words.clone()) } else { None })
            .collect()
    }
    fn request(&mut self) -> Option<super::Asked> {
        self.request.take()
    }
    fn answered(&mut self, status: u16) {
        self.status = Some(status);
    }
    fn request_failed(&mut self, failure: super::RequestFailure) {
        self.status = Some(0);
        self.request_failure = Some(failure);
    }
    fn at(&self) -> u32 {
        self.at + self.person.as_ref().map_or(0, Person::at)
    }
    fn check_deadline(&mut self, face: &mut BrowserFace<'_>, now: Time) {
        if self.commanding && face.command_done() {
            self.commanding = false;
            self.at += 1;
        }
        if self.keeping && face.kept_done() {
            self.keeping = false;
            self.at += 1;
        }
        if self.awaited.as_ref().is_some_and(|(until, _)| now >= *until) {
            self.await_step(face, now);
        }
        if let Some(why) = self.growth.as_ref().and_then(|growth| growth.expired(now)) {
            face.fail(why);
            self.growth = None;
            self.at += 1;
        }
    }
    fn server_action(&mut self) -> Option<ServerAction> {
        self.action.take()
    }
}

#[cfg(test)]
mod tests;
