use super::{
    Domain, Echo, Entry, Env, Keep, Limits, List, Live, Position, Queue, Repository, Request, Resource, Stored, Time,
    What, api,
};

pub(super) fn is_echo(echoes: &[Echo], wanted: Echo) -> bool {
    for echo in echoes {
        if *echo == wanted {
            return true;
        }
    }
    false
}
pub(super) fn key_echo(d: &Domain, repo: Repository, author: u64, key: Option<&[u8]>) -> bool {
    if !crate::identity::writer(d, repo, author) {
        return false;
    }
    match key {
        Some(key) => crate::outbox::echo(d, repo, key) || crate::identity::historical(d, key),
        None => false,
    }
}
pub(super) fn remove_echo(row: &mut Live, removed: Echo) {
    if !is_echo(&row.record.echoes, removed) {
        return;
    }
    let mut remaining = List::with_capacity(u32::try_from(row.record.echoes.len()).expect("bounded echoes"));
    for echo in &row.record.echoes {
        if *echo != removed {
            remaining.push(*echo).expect("bounded echoes");
        }
    }
    row.record.echoes = remaining.into_boxed();
}
pub(super) fn prune_comments(row: &mut Live, last: u64) {
    let mut remaining = List::with_capacity(u32::try_from(row.record.echoes.len()).expect("bounded echoes"));
    for echo in &row.record.echoes {
        match echo {
            Echo::Comment(id) if *id <= last => {}
            Echo::Comment(_) | Echo::Review(_) => {
                remaining.push(*echo).expect("bounded echoes");
            }
        }
    }
    row.record.echoes = remaining.into_boxed();
}
fn entry_resource(entry: &Entry) -> Option<Resource> {
    let what = match entry.effect.write {
        api::Write::Post { number, .. } => What::Issue(number),
        api::Write::Review { number, .. } => What::Pull(number),
        api::Write::CreateIssue { .. }
        | api::Write::OpenPull { .. }
        | api::Write::Edit { .. }
        | api::Write::SetReviewers { .. }
        | api::Write::Close { .. }
        | api::Write::Reopen { .. }
        | api::Write::Merge { .. }
        | api::Write::Update { .. }
        | api::Write::Status { .. }
        | api::Write::CreateBranch { .. }
        | api::Write::Dispatch { .. }
        | api::Write::Rerun { .. }
        | api::Write::DeleteBranch { .. } => return None,
    };
    Some(Resource { repository: entry.repository, what })
}
fn live_for_entry(k: &Keep, entry: &Entry) -> Option<Resource> {
    let mut resource = entry_resource(entry)?;
    if k.live.get(&resource).is_some() {
        return Some(resource);
    }
    match resource.what {
        What::Issue(number) => resource.what = What::Pull(number),
        What::Pull(_) | What::Branch(_) | What::Repository => return None,
    }
    if k.live.get(&resource).is_some() { Some(resource) } else { None }
}
pub(crate) fn position(k: &Keep, entry: &Entry) -> Position {
    match live_for_entry(k, entry) {
        Some(key) => k.live.get(&key).expect("resource exists").record.position,
        None => Position { at: Time::ZERO, comment: 0, review: 0, review_page: 1 },
    }
}
pub(crate) fn echo_room(k: &Keep, entry: &Entry) -> bool {
    match live_for_entry(k, entry) {
        Some(key) => {
            let row = k.live.get(&key).expect("resource exists");
            !row.active
                || !row.record.watch.participating
                || row.record.echoes.len() < usize::try_from(row.echo_capacity).expect("u32 fits usize")
        }
        None => true,
    }
}
pub(crate) fn made(d: &mut Domain, env: &Env<Limits>, entry: &Entry, made: crate::Made, out: &mut Queue<Request>) {
    let Some(key) = live_for_entry(&d.keep, entry) else {
        return;
    };
    let row = d.keep.live.get_mut(&key).expect("resource exists");
    if !row.active || !row.record.watch.participating {
        return;
    }
    let echo = match made {
        crate::Made::Commented(id) => Echo::Comment(id),
        crate::Made::Reviewed(id) => Echo::Review(id),
        crate::Made::Created(_)
        | crate::Made::Merged(_)
        | crate::Made::Updated(_)
        | crate::Made::Branch(_)
        | crate::Made::Set => return,
    };
    if !is_echo(&row.record.echoes, echo) {
        let capacity = u32::try_from(row.record.echoes.len())
            .expect("bounded echoes")
            .checked_add(1)
            .expect("admitted echo capacity");
        let mut echoes = List::with_capacity(capacity);
        for value in &row.record.echoes {
            echoes.push(*value).expect("bounded echoes");
        }
        echoes.push(echo).expect("admitted echo capacity");
        row.record.echoes = echoes.into_boxed();
        out.push(Request::Save { record: Stored::Live(row.record.clone()) });
    }
    super::poll::nudge_resource(&mut d.alarms, env, &key, row);
}
