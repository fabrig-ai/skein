use super::{Store, absent, empty, encoded, full, refusal};
use crate::{Config, Limits, response::Reply};
use alloc::boxed::Box;
use skein_forge::forgejo::{
    document::{Org, TeamSpec},
    request::Page,
    server::Answer,
};
use skein_lib::List;
/// A private organization and the person who owns it.
#[derive(Debug)]
pub struct Organization {
    pub id: u64,
    pub name: Box<[u8]>,
    pub owner: Box<[u8]>,
}
/// A team and its bounded membership.
#[derive(Debug)]
pub struct Team {
    pub id: u64,
    pub org: Box<[u8]>,
    pub spec: TeamSpec,
    pub members: List<u64>,
}
impl Store {
    /// Adds a scenario organization.
    pub fn org(&mut self, name: &[u8], owner: &[u8]) -> u64 {
        assert!(name.len() <= self.string_bytes && owner.len() <= self.string_bytes, "scenario org bounds");
        let id = self.next_org;
        self.next_org = id.checked_add(1).expect("org ids");
        self.orgs.push(Organization { id, name: name.into(), owner: owner.into() }).expect("org capacity");
        id
    }
    #[must_use]
    pub fn organization(&self, name: &[u8]) -> Option<&Organization> {
        let mut found = None;
        for org in &self.orgs {
            if org.name.as_ref() == name {
                found = Some(org);
            }
        }
        found
    }
    #[must_use]
    pub fn team(&self, org: &[u8], name: &[u8]) -> Option<&Team> {
        let mut found = None;
        for team in &self.teams {
            if team.org.as_ref() == org && team.spec.name.as_ref() == name {
                found = Some(team);
            }
        }
        found
    }
    /// Adds a scenario team without members.
    pub fn add_team(&mut self, org: &[u8], spec: TeamSpec) -> u64 {
        assert!(org.len() <= self.string_bytes && spec.name.len() <= self.string_bytes, "scenario team bounds");
        let id = self.next_team;
        self.next_team = id.checked_add(1).expect("team ids");
        self.teams
            .push(Team { id, org: org.into(), spec, members: List::with_capacity(self.accounts.capacity()) })
            .expect("team capacity");
        id
    }
    pub(super) fn read_org(&self, name: &[u8], limits: &Limits) -> Reply {
        if let Some(org) = self.organization(name) {
            encoded(200, Answer::Org(Org { id: org.id, name: org.name.clone() }), limits)
        } else {
            absent(limits)
        }
    }
    pub(super) fn create_org(&mut self, person: &[u8], name: &[u8], limits: &Limits) -> Reply {
        if self.find(person).is_none() {
            return absent(limits);
        }
        if self.organization(name).is_some() {
            return refusal(422, b"organization already exists", limits);
        }
        if self.orgs.room() == 0 {
            return full(limits);
        }
        let id = self.org(name, person);
        encoded(201, Answer::Org(Org { id, name: name.into() }), limits)
    }
    pub(super) fn list_teams(&self, org: &[u8], page: Page, config: &Config, limits: &Limits) -> Reply {
        if self.organization(org).is_none() {
            return absent(limits);
        }
        let (page, cap) = crate::routes::page(config, page);
        let mut rows = List::with_capacity(cap);
        let start = page.saturating_sub(1).saturating_mul(cap);
        let mut index = 0_u32;
        for team in &self.teams {
            if team.org.as_ref() == org {
                if index >= start && rows.room() > 0 {
                    rows.push(document(team)).expect("team page");
                }
                index = index.saturating_add(1);
            }
        }
        encoded(200, Answer::Teams(rows.into_boxed()), limits)
    }
    pub(super) fn create_team(&mut self, org: &[u8], spec: &TeamSpec, limits: &Limits) -> Reply {
        if self.organization(org).is_none() {
            return absent(limits);
        }
        if self.team(org, &spec.name).is_some() {
            return refusal(422, b"team already exists", limits);
        }
        if self.teams.room() == 0 {
            return full(limits);
        }
        self.add_team(org, spec.clone());
        encoded(201, Answer::Team(document(self.teams.last().expect("team added"))), limits)
    }
    pub(super) fn patch_team(&mut self, id: u64, spec: &TeamSpec, limits: &Limits) -> Reply {
        for index in 0..self.teams.len() {
            let team = self.teams.get_mut(index).expect("table index");
            if team.id == id {
                team.spec = spec.clone();
                return encoded(200, Answer::Team(document(team)), limits);
            }
        }
        absent(limits)
    }
    pub(super) fn members(&self, id: u64, page: Page, config: &Config, limits: &Limits) -> Reply {
        let mut found = None;
        for team in &self.teams {
            if team.id == id {
                found = Some(team);
            }
        }
        let Some(team) = found else {
            return absent(limits);
        };
        let (page, cap) = crate::routes::page(config, page);
        let mut rows = List::with_capacity(cap);
        let start = page.saturating_sub(1).saturating_mul(cap);
        let mut index = 0_u32;
        for account in &self.accounts {
            if team.members.as_slice().contains(&account.id) {
                if index >= start && rows.room() > 0 {
                    rows.push(self.user(account.id).expect("account found")).expect("member page");
                }
                index = index.saturating_add(1);
            }
        }
        encoded(200, Answer::Members(rows.into_boxed()), limits)
    }
    pub(super) fn add_member(&mut self, id: u64, login: &[u8], limits: &Limits) -> Reply {
        let Some(account) = self.find(login) else {
            return absent(limits);
        };
        let user = account.id;
        for index in 0..self.teams.len() {
            let team = self.teams.get_mut(index).expect("table index");
            if team.id == id {
                if !team.members.as_slice().contains(&user) && team.members.push(user).is_err() {
                    return full(limits);
                }
                return empty();
            }
        }
        absent(limits)
    }
}
fn document(team: &Team) -> skein_forge::forgejo::document::Team {
    skein_forge::forgejo::document::Team { id: team.id, spec: team.spec.clone() }
}
