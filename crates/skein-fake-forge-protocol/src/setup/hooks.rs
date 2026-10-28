use super::{Store, absent, empty, encoded, full};
use crate::{Config, Limits, response::Reply};
use alloc::boxed::Box;
use skein_forge::forgejo::{
    document::Hook,
    request::{HookSpec, Page},
    server::Answer,
};
use skein_lib::{List, Secret};
/// A hook's private subscription, including the secret never sent in a reply.
#[derive(Debug)]
pub struct Subscription {
    pub id: u64,
    pub org: Box<[u8]>,
    pub url: Box<[u8]>,
    pub secret: Secret,
    pub events: Box<[Box<[u8]>]>,
    pub active: bool,
}
impl Store {
    #[must_use]
    pub fn subscriptions<'a>(&'a self, org: &'a [u8]) -> Subscriptions<'a> {
        Subscriptions { rows: self.hooks.iter(), org }
    }
    /// Adds a scenario hook with no events, initially inactive.
    pub fn hook(&mut self, org: &[u8], url: &[u8], secret: Secret) -> u64 {
        self.insert_hook(org, &HookSpec { url: url.into(), secret, events: Box::new([]), active: false })
    }
    fn insert_hook(&mut self, org: &[u8], hook: &HookSpec) -> u64 {
        assert!(
            org.len() <= self.string_bytes
                && hook.url.len() <= self.string_bytes
                && hook.secret.len() <= self.secret_bytes,
            "scenario hook bounds"
        );
        assert!(hook.events.len() <= usize::try_from(self.hooks.capacity()).expect("u32 fits"), "scenario event count");
        for event in &hook.events {
            assert!(event.len() <= self.string_bytes, "scenario event bound");
        }
        let id = self.next_hook;
        self.next_hook = id.checked_add(1).expect("hook ids");
        self.hooks
            .push(Subscription {
                id,
                org: org.into(),
                url: hook.url.clone(),
                secret: hook.secret.clone(),
                events: hook.events.clone(),
                active: hook.active,
            })
            .expect("hook capacity");
        id
    }
    pub(super) fn list_hooks(&self, org: &[u8], page: Page, config: &Config, limits: &Limits) -> Reply {
        if self.organization(org).is_none() {
            return absent(limits);
        }
        let (page, cap) = crate::routes::page(config, page);
        let mut rows = List::with_capacity(cap);
        let start = usize::try_from(page.saturating_sub(1).saturating_mul(cap)).expect("u32 fits");
        for hook in self.subscriptions(org).skip(start).take(usize::try_from(cap).expect("u32 fits")) {
            rows.push(document(hook)).expect("hook page");
        }
        encoded(200, Answer::Hooks(rows.into_boxed()), limits)
    }
    pub(super) fn create_hook(&mut self, org: &[u8], hook: &HookSpec, limits: &Limits) -> Reply {
        if self.organization(org).is_none() {
            return absent(limits);
        }
        if self.hooks.room() == 0 {
            return full(limits);
        }
        self.insert_hook(org, hook);
        encoded(201, Answer::Hook(document(self.hooks.last().expect("hook inserted"))), limits)
    }
    pub(super) fn edit_hook(&mut self, org: &[u8], id: u64, spec: &HookSpec, limits: &Limits) -> Reply {
        for index in 0..self.hooks.len() {
            let hook = self.hooks.get_mut(index).expect("table index");
            if hook.org.as_ref() == org && hook.id == id {
                hook.url.clone_from(&spec.url);
                hook.secret = spec.secret.clone();
                hook.events.clone_from(&spec.events);
                hook.active = spec.active;
                return encoded(200, Answer::Hook(document(hook)), limits);
            }
        }
        absent(limits)
    }
    pub(super) fn remove_hook(&mut self, org: &[u8], id: u64, limits: &Limits) -> Reply {
        let old = core::mem::replace(&mut self.hooks, List::with_capacity(limits.documents.inner));
        let mut found = false;
        for hook in old.into_boxed() {
            if hook.org.as_ref() == org && hook.id == id {
                found = true;
            } else {
                self.hooks.push(hook).expect("same hook capacity");
            }
        }
        if found { empty() } else { absent(limits) }
    }
}
fn document(hook: &Subscription) -> Hook {
    Hook { id: hook.id, url: hook.url.clone(), events: hook.events.clone(), active: hook.active }
}

/// The subscriptions belonging to one organization, in id order.
#[derive(Debug)]
pub struct Subscriptions<'a> {
    rows: core::slice::Iter<'a, Subscription>,
    org: &'a [u8],
}
impl<'a> Iterator for Subscriptions<'a> {
    type Item = &'a Subscription;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let hook = self.rows.next()?;
            if hook.org.as_ref() == self.org {
                return Some(hook);
            }
        }
    }
}
