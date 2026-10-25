//! Installations and their tokens (fake-forge.md, 10.1).
use alloc::boxed::Box;
use skein_lib::{Duration, List, Queue, Rng, Secret, Wall, Writer, bytes};

/// GitHub's longest login, in bytes.
pub const LOGIN_BYTES: usize = 39;
/// The default token life: an hour on the forge's clock.
pub const TOKEN_LIFE: Duration = Duration::from_secs(3600);
const TOKEN_BYTES: usize = 40;
const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    User,
    Organization,
}
#[derive(Debug)]
pub enum Reach {
    All,
    Listed(List<u32>),
}
#[derive(Debug)]
pub struct Installation {
    pub id: u64,
    pub account: u64,
    pub login: Box<[u8]>,
    pub kind: Kind,
    pub reach: Reach,
    pub suspended: bool,
    pub created: Wall,
}
/// Everything its installation may do, or contents alone (forge-github.md, 8.2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    Full,
    Contents,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Grant {
    pub installation: u64,
    pub scope: Scope,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub installations: u32,
    pub tokens: u32,
    pub listed: u32,
}
/// Admission refused for capacity (including listed repositories or ids), or login length.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Install {
    Full,
    Login,
}
/// A mint refused for its installation, capacity, or an unrepresentable expiry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mint {
    Missing,
    Suspended,
    Full,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Denied {
    Unknown,
    Expired,
}

#[derive(Debug)]
struct Issued {
    secret: Secret,
    grant: Grant,
    expires: Wall,
    mint: u64,
}

/// Bounded stores. Rows stay packed and in id order; removal shifts owned
/// rows in place, without allocating a second table.
#[derive(Debug)]
pub struct Installations {
    events: Queue<Changed>,
    now: Wall,
    rows: List<Option<Installation>>,
    tokens: List<Option<Issued>>,
    rng: Rng,
    next: u64,
    listed: u32,
    life: Duration,
    minted: u64,
}
impl Installations {
    #[must_use]
    pub fn new(limits: &Limits, seed: u64) -> Self {
        Self {
            events: Queue::with_capacity(event_capacity(limits).expect("admitted event capacity")),
            now: Wall::EPOCH,
            rows: List::with_capacity(limits.installations),
            tokens: List::with_capacity(limits.tokens),
            rng: Rng::new(seed),
            next: 1,
            listed: limits.listed,
            life: TOKEN_LIFE,
            minted: 0,
        }
    }

    pub(crate) fn with_life(limits: &Limits, seed: u64, life: Duration) -> Self {
        Self { life, ..Self::new(limits, seed) }
    }

    /// Conversion replaces the fake's one App. Its installations and tokens
    /// belong to the previous App; keep the id and random streams advancing.
    pub(crate) fn replace_app(&mut self) {
        self.rows.clear();
        self.tokens.clear();
        while self.events.pop().is_some() {}
    }

    /// Number of successful mints, also the latest token's mint number.
    #[must_use]
    pub const fn minted(&self) -> u64 {
        self.minted
    }

    pub(crate) fn identity(&self, bearer: &[u8]) -> Option<crate::Minted> {
        for token in self.tokens.iter().flatten() {
            if token.secret.expose() == bearer {
                return Some(crate::Minted { installation: token.grant.installation, mint: token.mint });
            }
        }
        None
    }

    /// Ids are sequential from 1 and are never reused (fake-forge.md, 10.4).
    /// A listed reach is capped per installation, and retained at its length.
    pub fn install(&mut self, account: u64, login: &[u8], kind: Kind, reach: Reach, now: Wall) -> Result<u64, Install> {
        if login.is_empty() || login.len() > LOGIN_BYTES {
            return Err(Install::Login);
        }
        if self.rows.room() == 0 || self.events.room() == 0 {
            return Err(Install::Full);
        }
        let next = self.next.checked_add(1).ok_or(Install::Full)?;
        let reach = match reach {
            Reach::All => Reach::All,
            Reach::Listed(repositories) => {
                if repositories.len() > self.listed {
                    return Err(Install::Full);
                }
                let mut kept = List::with_capacity(repositories.len());
                for repository in &repositories {
                    kept.push(*repository).expect("admitted repository count");
                }
                Reach::Listed(kept)
            }
        };
        let id = self.next;
        let row =
            Installation { id, account, login: bytes::copy_of(login), kind, reach, suspended: false, created: now };
        self.events.push(changed(&row, skein_forge::github::delivery::Change::Created, now));
        self.rows.push(Some(row)).expect("installation room checked");
        self.now = now;
        self.next = next;
        Ok(id)
    }

    /// Removes an installation and every token it minted; false when absent.
    pub fn remove(&mut self, id: u64) -> bool {
        if self.events.room() == 0 {
            return false;
        }
        for at in 0..self.rows.len() {
            let row = self.rows.get(at).expect("stored row").as_ref().expect("packed installation");
            if row.id == id {
                self.events.push(changed(row, skein_forge::github::delivery::Change::Deleted, self.now));
                let last = self.rows.len().checked_sub(1).expect("installation found");
                for index in at..last {
                    let next = index.checked_add(1).expect("before last");
                    let moved = self.rows.get_mut(next).expect("next row").take();
                    *self.rows.get_mut(index).expect("prior row") = moved;
                }
                self.rows.truncate(last);
                for index in (0..self.tokens.len()).rev() {
                    let token = self.tokens.get(index).expect("stored row").as_ref().expect("packed token");
                    if token.grant.installation == id {
                        self.remove_token(index);
                    }
                }
                return true;
            }
        }
        false
    }

    pub fn suspend(&mut self, id: u64, suspended: bool) -> bool {
        if self.events.room() == 0 {
            return false;
        }
        for index in 0..self.rows.len() {
            let row = self.rows.get_mut(index).expect("stored row").as_mut().expect("packed installation");
            if row.id == id {
                if row.suspended == suspended {
                    return true;
                }
                row.suspended = suspended;
                self.events.push(changed(
                    row,
                    if suspended {
                        skein_forge::github::delivery::Change::Suspended
                    } else {
                        skein_forge::github::delivery::Change::Unsuspended
                    },
                    self.now,
                ));
                return true;
            }
        }
        false
    }

    /// Replaces a reach within the listed bound; false for an absent id or an oversized list.
    /// Retains only the list's length, not its caller's spare capacity.
    pub fn set_reach(&mut self, id: u64, reach: Reach) -> bool {
        if self.events.room() == 0 {
            return false;
        }
        let reach = match reach {
            Reach::All => Reach::All,
            Reach::Listed(repositories) => {
                if repositories.len() > self.listed {
                    return false;
                }
                let mut kept = List::with_capacity(repositories.len());
                for repository in &repositories {
                    kept.push(*repository).expect("admitted repository count");
                }
                Reach::Listed(kept)
            }
        };
        for index in 0..self.rows.len() {
            let row = self.rows.get_mut(index).expect("stored row").as_mut().expect("packed installation");
            if row.id == id {
                row.reach = reach;
                self.events.push(changed(row, skein_forge::github::delivery::Change::Repositories, self.now));
                return true;
            }
        }
        false
    }

    /// Sets the forge clock used by scenario mutations without a timestamp argument.
    pub fn clock(&mut self, now: Wall) {
        self.now = now;
    }
    pub(crate) fn event_capacity(&self) -> u32 {
        self.events.capacity()
    }
    pub(crate) fn take_event(&mut self) -> Option<Changed> {
        self.events.pop()
    }
    /// Renames one installation's account, retaining the new login in its event.
    pub fn rename(&mut self, id: u64, login: &[u8]) -> bool {
        if login.is_empty() || login.len() > LOGIN_BYTES || self.events.room() == 0 {
            return false;
        }
        for index in 0..self.rows.len() {
            let row = self.rows.get_mut(index).expect("row").as_mut().expect("packed installation");
            if row.id == id {
                if row.login.as_ref() == login {
                    return true;
                }
                row.login = bytes::copy_of(login);
                self.events.push(changed(
                    row,
                    skein_forge::github::delivery::Change::Renamed { login: bytes::copy_of(login) },
                    self.now,
                ));
                return true;
            }
        }
        false
    }
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&Installation> {
        let mut found = None;
        for row in self.iter() {
            if row.id == id {
                found = Some(row);
            }
        }
        found
    }

    /// In id order.
    pub fn iter(&self) -> impl Iterator<Item = &Installation> {
        self.rows.iter().flatten()
    }

    /// Mints `ghs_` and 36 seeded alphanumeric bytes, expiring after the configured life.
    /// A full store drops its expired tokens first. A clock too near the end
    /// of `Wall` to hold the full life is refused as `Full`.
    pub fn mint(&mut self, id: u64, scope: Scope, now: Wall) -> Result<(Secret, Wall), Mint> {
        let row = self.get(id).ok_or(Mint::Missing)?;
        if row.suspended {
            return Err(Mint::Suspended);
        }
        let expires = Wall::from_nanos(now.as_nanos().checked_add(self.life.as_nanos()).ok_or(Mint::Full)?);
        if self.tokens.room() == 0 {
            for index in (0..self.tokens.len()).rev() {
                let token = self.tokens.get(index).expect("stored row").as_ref().expect("packed token");
                if now >= token.expires {
                    self.remove_token(index);
                }
            }
        }
        if self.tokens.room() == 0 {
            return Err(Mint::Full);
        }
        let mint = self.minted.checked_add(1).ok_or(Mint::Full)?;
        let mut text = Writer::new(TOKEN_BYTES);
        text.put(b"ghs_").expect("token prefix");
        for _byte in 0_u32..36 {
            let digit = usize::try_from(self.rng.below(62)).expect("alphanumeric index");
            text.put(&[*ALPHABET.get(digit).expect("alphanumeric index")]).expect("token length");
        }
        let secret = Secret::new(text.finish());
        self.tokens
            .push(Some(Issued { secret: secret.clone(), grant: Grant { installation: id, scope }, expires, mint }))
            .expect("token room checked");
        self.minted = mint;
        Ok((secret, expires))
    }

    /// Suspension is checked by the route; it does not revoke a grant here.
    pub fn token(&self, bearer: &[u8], now: Wall) -> Result<&Grant, Denied> {
        match self.token_slot(bearer, now) {
            Ok((_, grant)) => Ok(grant),
            Err(denied) => Err(denied),
        }
    }
    pub(crate) fn token_slot(&self, bearer: &[u8], now: Wall) -> Result<(u32, &Grant), Denied> {
        for slot in 0..self.tokens.len() {
            let token = self.tokens.get(slot).expect("stored row").as_ref().expect("packed token");
            if token.secret.expose() == bearer {
                return if now >= token.expires { Err(Denied::Expired) } else { Ok((slot, &token.grant)) };
            }
        }
        Err(Denied::Unknown)
    }
    pub(crate) fn token_at(&self, slot: u32) -> Option<&Grant> {
        Some(&self.tokens.get(slot)?.as_ref()?.grant)
    }
    pub(crate) fn revoke_slot(&mut self, slot: u32) -> bool {
        if self.token_at(slot).is_none() {
            return false;
        }
        self.remove_token(slot);
        true
    }
    /// `DELETE /installation/token`'s store: false when unknown.
    pub fn revoke(&mut self, bearer: &[u8]) -> bool {
        for slot in 0..self.tokens.len() {
            let token = self.tokens.get(slot).expect("stored row").as_ref().expect("packed token");
            if token.secret.expose() == bearer {
                return self.revoke_slot(slot);
            }
        }
        false
    }

    /// Whether the grant's installation reaches the repository.
    #[must_use]
    pub fn reaches(&self, grant: &Grant, repository: u32) -> bool {
        let Some(row) = self.get(grant.installation) else {
            return false;
        };
        match &row.reach {
            Reach::All => true,
            Reach::Listed(repositories) => repositories.as_slice().contains(&repository),
        }
    }

    fn remove_token(&mut self, at: u32) {
        let last = self.tokens.len().checked_sub(1).expect("token found");
        for index in at..last {
            let next = index.checked_add(1).expect("before last");
            let moved = self.tokens.get_mut(next).expect("next token").take();
            *self.tokens.get_mut(index).expect("prior token") = moved;
        }
        self.tokens.truncate(last);
    }

    /// Heap held by both tables when full (fake-forge.md, 8), including
    /// maximum logins, per-installation repository lists and token text.
    #[must_use]
    pub fn worst_case(limits: &Limits) -> Option<u64> {
        let rows = List::<Option<Installation>>::worst_case(limits.installations)?;
        let per_row = u64::try_from(LOGIN_BYTES).ok()?.checked_add(List::<u32>::worst_case(limits.listed)?)?;
        let tokens = List::<Option<Issued>>::worst_case(limits.tokens)?;
        rows.checked_add(per_row.checked_mul(u64::from(limits.installations))?)?
            .checked_add(Queue::<Changed>::worst_case(event_capacity(limits)?)?)?
            .checked_add(
                u64::from(event_capacity(limits)?).checked_mul(u64::try_from(LOGIN_BYTES).ok()?.checked_mul(2)?)?,
            )?
            .checked_add(tokens)?
            .checked_add(u64::try_from(TOKEN_BYTES).ok()?.checked_mul(u64::from(limits.tokens))?)
    }
}

#[cfg(test)]
#[path = "tests/memory.rs"]
mod tests;

#[derive(Debug)]
pub(crate) struct Changed {
    pub payload: skein_forge::github::delivery::Delivery,
    pub at: Wall,
}
fn changed(row: &Installation, change: skein_forge::github::delivery::Change, at: Wall) -> Changed {
    Changed {
        payload: skein_forge::github::delivery::Delivery::Installation {
            installation: row.id,
            account: skein_forge::github::delivery::Account { id: row.account, login: bytes::copy_of(&row.login) },
            change,
        },
        at,
    }
}
fn event_capacity(limits: &Limits) -> Option<u32> {
    limits.installations.checked_mul(8)
}
