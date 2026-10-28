//! Both differences and every login are retained before the first mutation (forge-github.md, 4.2).
use super::{Bounds, Plan, Request, Stage, Step, Tables, api};
use crate::github::{document, request};
use alloc::boxed::Box;
use skein_lib::List;

#[derive(Debug)]
pub(super) struct Reviewers {
    pub(super) add: List<u64>,
    remove: List<u64>,
    additions: List<Box<[u8]>>,
    removals: List<Box<[u8]>>,
    pub(super) added: bool,
    bytes: u64,
}

fn ids_fit(ids: &[u64], bounds: Bounds) -> Result<u32, api::Error> {
    let count = u32::try_from(ids.len()).ok().ok_or(api::Error::TooLarge)?;
    let bytes = u64::from(count).checked_mul(8).ok_or(api::Error::TooLarge)?;
    if count > bounds.rows || bytes > u64::from(bounds.answer_bytes) {
        return Err(api::Error::TooLarge);
    }
    Ok(count)
}

impl Plan {
    pub(super) fn reviewers_given(&self) -> Option<(u64, &[u64])> {
        match &self.write {
            api::Write::SetReviewers { number, reviewers } => Some((*number, reviewers)),
            api::Write::CreateIssue { .. }
            | api::Write::OpenPull { .. }
            | api::Write::Post { .. }
            | api::Write::Review { .. }
            | api::Write::Labels { .. }
            | api::Write::CreateLabel { .. }
            | api::Write::Edit { .. }
            | api::Write::Close { .. }
            | api::Write::Reopen { .. }
            | api::Write::Merge { .. }
            | api::Write::Update { .. }
            | api::Write::Status { .. }
            | api::Write::CreateBranch { .. }
            | api::Write::Dispatch { .. }
            | api::Write::Rerun { .. }
            | api::Write::DeleteBranch { .. } => None,
        }
    }
    pub(super) fn reviewers_check(&self, bounds: Bounds) -> Result<(), api::Error> {
        if let Some((_, reviewers)) = self.reviewers_given() {
            ids_fit(reviewers, bounds)?;
        }
        Ok(())
    }
    pub(super) fn added(&self) -> bool {
        match &self.reviewers {
            Some(reviewers) => reviewers.added,
            None => false,
        }
    }
    pub(super) fn reviewers_pull(
        &mut self,
        pull: document::Pull,
        bounds: Bounds,
        tables: &mut Tables,
    ) -> Result<Option<Step>, api::Error> {
        let (number, desired) = self.reviewers_given().ok_or(api::Error::InvalidAnswer)?;
        if pull.number != number {
            return Err(self.invalid());
        }
        let desired_count = ids_fit(desired, bounds)?;
        let current_count = ids_fit(&pull.reviewers, bounds)?;
        let mut add = List::with_capacity(desired_count);
        let mut remove = List::with_capacity(current_count);
        for id in desired {
            if !pull.reviewers.contains(id) && !add.as_slice().contains(id) {
                add.push(*id).ok().ok_or(api::Error::TooLarge)?;
            }
        }
        for id in &pull.reviewers {
            if !desired.contains(id) && !remove.as_slice().contains(id) {
                remove.push(*id).ok().ok_or(api::Error::TooLarge)?;
            }
        }
        let count = add.len().checked_add(remove.len()).ok_or(api::Error::TooLarge)?;
        let bytes = u64::from(count).checked_mul(8).ok_or(api::Error::TooLarge)?;
        if count > bounds.rows || bytes > u64::from(bounds.answer_bytes) {
            return Err(api::Error::TooLarge);
        }
        if count == 0 {
            return self.terminal(api::Answer::Done, bounds);
        }
        self.reviewers = Some(Box::new(Reviewers {
            additions: List::with_capacity(add.len()),
            removals: List::with_capacity(remove.len()),
            add,
            remove,
            added: false,
            bytes,
        }));
        self.reviewers_next(bounds, tables)
    }
    fn reviewers_next(&mut self, bounds: Bounds, tables: &mut Tables) -> Result<Option<Step>, api::Error> {
        let cap = bounds.rows;
        for _ in 0..=cap {
            let reviewers = self.reviewers.as_ref().ok_or(api::Error::InvalidAnswer)?;
            let needed = reviewers.next();
            match needed {
                Some(id) => match tables.user(self.repository.forge, id) {
                    Some(login) => self.reviewers_login(login, bounds)?,
                    None => {
                        self.stage = Stage::ResolveUser;
                        return Ok(Some(self.send(Request::User { id })));
                    }
                },
                None => {
                    if !request::reviewers_fit(reviewers.additions.as_slice(), bounds.body_bytes)
                        || !request::reviewers_fit(reviewers.removals.as_slice(), bounds.body_bytes)
                    {
                        return Err(api::Error::TooLarge);
                    }
                    let (number, _) = self.reviewers_given().ok_or(api::Error::InvalidAnswer)?;
                    return Ok(Some(self.reviewers_mutation(number)?));
                }
            }
        }
        Err(api::Error::TooLarge)
    }
    fn reviewers_login(&mut self, login: &[u8], bounds: Bounds) -> Result<(), api::Error> {
        let reviewers = self.reviewers.as_mut().ok_or(api::Error::InvalidAnswer)?;
        let width = u64::try_from(crate::measure::STRING_BYTES).ok().ok_or(api::Error::TooLarge)?;
        let length = u64::try_from(login.len()).ok().ok_or(api::Error::TooLarge)?;
        reviewers.bytes = reviewers
            .bytes
            .checked_add(width)
            .ok_or(api::Error::TooLarge)?
            .checked_add(length)
            .ok_or(api::Error::TooLarge)?;
        if reviewers.bytes > u64::from(bounds.answer_bytes) {
            return Err(api::Error::TooLarge);
        }
        let logins = if reviewers.additions.len() < reviewers.add.len() {
            &mut reviewers.additions
        } else {
            &mut reviewers.removals
        };
        logins.push(Box::from(login)).ok().ok_or(api::Error::TooLarge)
    }
    pub(super) fn reviewers_user(
        &mut self,
        user: document::User,
        bounds: Bounds,
        tables: &mut Tables,
    ) -> Result<Option<Step>, api::Error> {
        let reviewers = self.reviewers.as_ref().ok_or(api::Error::InvalidAnswer)?;
        if reviewers.next() != Some(user.id) {
            return Err(self.invalid());
        }
        self.reviewers_login(&user.login, bounds)?;
        tables.keep_user(self.repository.forge, user.id, user.login);
        self.reviewers_next(bounds, tables)
    }
    pub(super) fn reviewers_mutation(&mut self, number: u64) -> Result<Step, api::Error> {
        let row = self.row.as_ref().ok_or(api::Error::InvalidAnswer)?;
        let reviewers = self.reviewers.as_ref().ok_or(api::Error::InvalidAnswer)?;
        let add = !reviewers.added && !reviewers.add.is_empty();
        let logins = if add { reviewers.additions.to_boxed() } else { reviewers.removals.to_boxed() };
        let request = Request::Reviewers { owner: row.owner.clone(), name: row.name.clone(), number, add, logins };
        self.stage = Stage::Mutate;
        Ok(self.send(request))
    }
    pub(super) fn reviewers_done(&mut self, bounds: Bounds) -> Result<Option<Step>, api::Error> {
        let reviewers = self.reviewers.as_mut().ok_or(api::Error::InvalidAnswer)?;
        if !reviewers.added && !reviewers.add.is_empty() {
            reviewers.added = true;
            if !reviewers.remove.is_empty() {
                let (number, _) = self.reviewers_given().ok_or(api::Error::InvalidAnswer)?;
                return Ok(Some(self.reviewers_mutation(number)?));
            }
        }
        self.terminal(api::Answer::Done, bounds)
    }
}
impl Reviewers {
    fn next(&self) -> Option<u64> {
        match self.add.get(self.additions.len()) {
            Some(id) => Some(*id),
            None => self.remove.get(self.removals.len()).copied(),
        }
    }
}
