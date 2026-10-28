//! Account admission and removal (oauth.md, sections 6.2, 6.3 and 6.6).

use super::{Component, Lifecycle, Name, State, Unusable, failed, held, set_held};
use crate::accounts::Source;
use crate::keeper::{Operation, Private, Store};
use crate::{Account, Asked, Event, Failure, Keeper, Limits, Lower, Refusal, Transport};
use skein_lib::{Env, List, Queue};
use skein_oauth as oauth;

pub(super) fn admit(account: u32, configuration: Account, limits: &Limits) -> Result<(Source, State, Store), Unusable> {
    Ok(match configuration {
        Account::HandedIn => (Source::HandedIn, State::Empty, Store::Owner),
        Account::SignIn { registration, endpoint, keeper, shared, origins } => {
            match endpoint.transport {
                Transport::Plaintext if !endpoint.address.ip().is_loopback() => {
                    return Err(Unusable::Plaintext { account });
                }
                Transport::Plaintext | Transport::Tls { .. } => {}
            }
            if registration.client_secret.is_none()
                && !crate::redirect::varying(&registration)
                && oauth::redirect_address(&registration.redirect_uri).is_err()
            {
                return Err(Unusable::Redirect { account });
            }
            match oauth::validate_registration(&registration, &limits.client, true) {
                Ok(()) => {}
                Err(failure) => return Err(Unusable::Registration { account, failure }),
            }
            if origins.len() > crate::limits::MAX_ORIGINS || !terms_origin(&registration, &origins) {
                return Err(Unusable::Origin { account });
            }
            let mut bounded = List::with_capacity(crate::limits::MAX_ORIGINS);
            for origin in origins.into_boxed() {
                if origin.uri.len() > usize::try_from(limits.client.uri_bytes).expect("u32 fits")
                    || uri_origin(&origin.uri) != Some(origin.uri.as_ref())
                {
                    return Err(Unusable::Origin { account });
                }
                if let Transport::Plaintext = origin.endpoint.transport
                    && !origin.endpoint.address.ip().is_loopback()
                {
                    return Err(Unusable::Plaintext { account });
                }
                bounded.push(origin).expect("admitted origins");
            }
            let (state, store) = match keeper {
                Keeper::Owner { kept } => (
                    match kept {
                        Some(record) => {
                            match oauth::encode_record(&record, &limits.client.document) {
                                Ok(encoded) => drop(encoded),
                                Err(why) => return Err(Unusable::Record { account, why }),
                            }
                            if super::names::missing_id(&registration, &record) {
                                State::Empty
                            } else {
                                State::Loaded { record }
                            }
                        }
                        None => State::Empty,
                    },
                    Store::Owner,
                ),
                Keeper::Private { root, directory, file } => {
                    if directory.len() >= 4096 || file.len() >= 4096 {
                        return Err(Unusable::Path { account });
                    }
                    (State::Empty, Store::Private(Private::new(root, directory, file)))
                }
            };
            (Source::SignIn { registration, endpoint, shared, origins: bounded }, state, store)
        }
    })
}

impl Component {
    pub(super) fn want(&mut self, account: u32, wanted: bool, up: &mut Queue<Event>) {
        if !self.refused(account, Asked::Want, up) {
            match self.names.get_mut(account).expect("live name") {
                Name::Live { wanted: flag, .. } => *flag = wanted,
                Name::Removing { .. } | Name::Free => unreachable!("live name"),
            }
            if !wanted {
                match self.accounts.get_mut(account).expect("account state") {
                    State::Record { held: false, due, .. } => *due = false,
                    State::Record { .. } | State::Empty | State::Loaded { .. } => {}
                }
            }
        }
    }

    pub(super) fn release(&mut self, account: u32) {
        let wanted = self.wanted(account);
        match self.accounts.get_mut(account).expect("account state") {
            State::Record { held, due, .. } => {
                *held = false;
                if !wanted {
                    *due = false;
                }
            }
            State::Empty | State::Loaded { .. } => {}
        }
    }

    pub(super) fn wanted(&self, account: u32) -> bool {
        match self.names.get(account).expect("account name") {
            Name::Live { wanted, .. } => *wanted,
            Name::Removing { .. } | Name::Free => false,
        }
    }

    pub(super) fn refresh_ready(&self, account: u32, now: skein_lib::Time) -> bool {
        match self.accounts.get(account).expect("account state") {
            State::Record { earliest: Some(at), .. } => now >= *at,
            State::Empty | State::Loaded { .. } | State::Record { earliest: None, .. } => true,
        }
    }

    pub(super) fn grant_before_refresh(&mut self, env: &Env<Limits>, account: u32, up: &mut Queue<Event>) {
        let slot = self.accounts.get_mut(account).expect("account state");
        match slot {
            State::Record { expiry, rejected, .. } if env.now < *expiry && !*rejected => {
                super::grant(
                    slot,
                    self.sources.get(account).expect("account source"),
                    env.now,
                    super::names::number(&self.names, account),
                    up,
                );
            }
            State::Record { held, due, .. } => {
                *held = true;
                *due = true;
            }
            State::Empty | State::Loaded { .. } => unreachable!("refreshable record"),
        }
    }

    /// Admits a named account, answering only a refusal into the uncapped owner queue.
    pub fn add(&mut self, env: &Env<Limits>, account: u32, configuration: Account, up: &mut Queue<Event>) {
        let why = if self.lifecycle != Lifecycle::Live {
            Some(Refusal::Closed)
        } else if self.terminal_place(account).is_some() {
            Some(Refusal::Taken)
        } else {
            None
        };
        if let Some(why) = why {
            up.push(Event::Refused { account, asked: Asked::Add, why });
            return;
        }
        let mut free = None;
        for place in 0..self.names.len() {
            match self.names.get(place).expect("bounded name") {
                Name::Free => {
                    free = Some(place);
                    break;
                }
                Name::Live { .. } | Name::Removing { .. } => {}
            }
        }
        let Some(place) = free else {
            up.push(Event::Refused { account, asked: Asked::Add, why: Refusal::Full { bound: env.limits.accounts } });
            return;
        };
        let (source, state, store) = match admit(account, configuration, &env.limits) {
            Ok(admitted) => admitted,
            Err(why) => {
                up.push(Event::Refused { account, asked: Asked::Add, why: Refusal::Unusable(why) });
                return;
            }
        };
        *self.names.get_mut(place).expect("bounded name") = Name::Live { number: account, wanted: false };
        if place == self.accounts.len() {
            match self.accounts.push(state) {
                Ok(()) => {}
                Err(State::Empty | State::Loaded { .. } | State::Record { .. }) => unreachable!("free account slot"),
            }
            match self.sources.push(source) {
                Ok(()) => {}
                Err(Source::HandedIn | Source::SignIn { .. }) => unreachable!("free account slot"),
            }
            self.bindings.push(None).expect("free binding slot");
            match self.keepers.push(store) {
                Ok(()) => {}
                Err(Store::Owner | Store::Private(_)) => unreachable!("free keeper slot"),
            }
        } else {
            *self.accounts.get_mut(place).expect("free account slot") = state;
            *self.sources.get_mut(place).expect("free source slot") = source;
            *self.keepers.get_mut(place).expect("free keeper slot") = store;
        }
    }

    pub(super) fn terminal_place(&self, number: u32) -> Option<u32> {
        for place in 0..self.names.len() {
            match self.names.get(place).expect("bounded name") {
                Name::Live { number: held, .. } | Name::Removing { number: held } if number == *held => {
                    return Some(place);
                }
                Name::Live { .. } | Name::Removing { .. } | Name::Free => {}
            }
        }
        None
    }

    pub(super) fn live(&self, place: u32) -> bool {
        match self.names.get(place).expect("bounded name") {
            Name::Live { .. } => true,
            Name::Removing { .. } | Name::Free => false,
        }
    }

    fn is_removing(&self, place: u32) -> bool {
        match self.names.get(place).expect("bounded name") {
            Name::Removing { .. } => true,
            Name::Live { .. } | Name::Free => false,
        }
    }

    pub(super) fn removing(&self) -> bool {
        for place in 0..self.accounts.len() {
            if self.is_removing(place) {
                return true;
            }
        }
        false
    }

    pub(super) fn remove(&mut self, env: &Env<Limits>, place: u32, up: &mut Queue<Event>, io: &mut Queue<Lower>) {
        let number = super::names::number(&self.names, place);
        *self.names.get_mut(place).expect("live name") = Name::Removing { number };
        self.removal_step(env, place, up, io);
    }

    pub(super) fn removal_has_work(&self) -> bool {
        for place in 0..self.accounts.len() {
            if !self.is_removing(place) {
                continue;
            }
            if held(self.accounts.get(place).expect("account state")) {
                return true;
            }
            if let Some(id) = *self.bindings.get(place).expect("account binding") {
                let exchange = self.exchanges.get(id).expect("bound exchange");
                if exchange.pending_grant || unsent(exchange) {
                    return true;
                }
            } else if self.account_has_exchanges(place) {
                if let Store::Private(keeper) = self.keepers.get(place).expect("account keeper")
                    && keeper.waiting_grant
                {
                    return true;
                }
            } else {
                match self.keepers.get(place).expect("account keeper") {
                    Store::Owner => return true,
                    Store::Private(keeper) => {
                        if keeper.waiting_grant || keeper.closed || keeper.pending.is_none() {
                            return true;
                        }
                        match &keeper.pending {
                            Some(pending) if pending.operation != Operation::Close && !pending.cancelled => {
                                return true;
                            }
                            Some(_) | None => {}
                        }
                    }
                }
            }
        }
        false
    }

    pub(super) fn removal_progress(&mut self, env: &Env<Limits>, up: &mut Queue<Event>, io: &mut Queue<Lower>) -> bool {
        for place in 0..self.accounts.len() {
            if self.is_removing(place) && self.removal_step(env, place, up, io) {
                return true;
            }
        }
        false
    }

    fn removal_step(&mut self, env: &Env<Limits>, place: u32, up: &mut Queue<Event>, io: &mut Queue<Lower>) -> bool {
        let number = super::names::number(&self.names, place);
        if let Some(id) = *self.bindings.get(place).expect("account binding") {
            let exchange = self.exchanges.get_mut(id).expect("bound exchange");
            if unsent(exchange) {
                self.fail_exchange(env, id, Failure::Exchange(oauth::Failure::Cancelled), up, io);
                self.collect(env, id, up, io);
                return true;
            }
            let slot = self.accounts.get_mut(place).expect("account state");
            if exchange.pending_grant || held(slot) {
                exchange.pending_grant = false;
                set_held(slot, false);
                failed(number, Failure::Exchange(oauth::Failure::Cancelled), up);
                return true;
            }
            return false;
        }
        let slot = self.accounts.get_mut(place).expect("account state");
        if held(slot) {
            set_held(slot, false);
            failed(number, Failure::Exchange(oauth::Failure::Cancelled), up);
            return true;
        }
        match self.keepers.get_mut(place).expect("account keeper") {
            Store::Private(keeper) => {
                if keeper.waiting_grant {
                    keeper.waiting_grant = false;
                    failed(number, Failure::Exchange(oauth::Failure::Cancelled), up);
                    return true;
                }
                if !keeper.closed {
                    return false;
                }
            }
            Store::Owner => {}
        }
        if self.account_has_exchanges(place) {
            return false;
        }
        *self.names.get_mut(place).expect("removing name") = Name::Free;
        *self.accounts.get_mut(place).expect("removed account") = State::Empty;
        *self.sources.get_mut(place).expect("removed source") = Source::HandedIn;
        *self.keepers.get_mut(place).expect("removed keeper") = Store::Owner;
        up.push(Event::Removed { account: number });
        self.settled(up);
        true
    }
}

fn unsent(exchange: &crate::exchange::Exchange) -> bool {
    match exchange.stage {
        crate::exchange::Stage::Loading | crate::exchange::Stage::Locking | crate::exchange::Stage::Reading => true,
        crate::exchange::Stage::Running => match &exchange.web {
            Some(web) => !web.sent,
            None => true,
        },
        crate::exchange::Stage::Keeping { .. }
        | crate::exchange::Stage::Forgetting { .. }
        | crate::exchange::Stage::Finished => false,
    }
}

fn uri_origin(uri: &[u8]) -> Option<&[u8]> {
    let start = skein_lib::bytes::find(uri, b"://")?.checked_add(3)?;
    let rest = uri.get(start..)?;
    let end = skein_lib::bytes::find(rest, b"/").unwrap_or(rest.len()).checked_add(start)?;
    uri.get(..end)
}

pub(super) fn endpoint<'a>(
    uri: &[u8],
    registration: &oauth::Registration,
    token: &'a crate::Endpoint,
    origins: &'a List<crate::Origin>,
) -> Option<&'a crate::Endpoint> {
    let origin = uri_origin(uri)?;
    if Some(origin) == uri_origin(&registration.token_endpoint) {
        return Some(token);
    }
    for configured in origins {
        if origin == configured.uri.as_ref() {
            return Some(&configured.endpoint);
        }
    }
    None
}

fn terms_origin(registration: &oauth::Registration, origins: &List<crate::Origin>) -> bool {
    match &registration.terms {
        Some(terms) => {
            if let Some(endpoint) = &terms.revocation_endpoint
                && !given(endpoint, registration, origins)
            {
                return false;
            }
            match &terms.openid {
                Some(openid) => {
                    given(&openid.issuer, registration, origins)
                        && match &openid.key_set {
                            Some(uri) => given(uri, registration, origins),
                            None => true,
                        }
                }
                None => true,
            }
        }
        None => true,
    }
}

fn given(uri: &[u8], registration: &oauth::Registration, origins: &List<crate::Origin>) -> bool {
    let origin = uri_origin(uri);
    if origin == uri_origin(&registration.token_endpoint) {
        return true;
    }
    for configured in origins {
        if Some(configured.uri.as_ref()) == origin {
            return true;
        }
    }
    false
}
