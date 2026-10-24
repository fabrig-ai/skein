use super::{
    Alarm, Answer, Box, Commit, Delivery, Domain, Echo, Env, Error, Fresh, Limits, List, Phase, Priority, Progress,
    Queue, Repository, Request, Resource, Scan, Sent, State, Stored, Time, What, api,
};

pub(crate) fn answered_repository(
    d: &mut Domain,
    env: &Env<Limits>,
    key: Repository,
    result: Result<Answer, Error>,
    out: &mut Queue<Request>,
) {
    let row = d.keep.repos.get(&key).expect("call owns repository");
    if !row.active {
        d.keep.repos.remove(&key).expect("inactive repository exists");
        return;
    }
    let mut progress = match row.state {
        Scan::Busy(progress) => progress,
        Scan::Idle | Scan::Due(_) | Scan::Waiting(_) => unreachable!("repository terminal while busy"),
    };
    let Ok(answer) = result else {
        d.keep.repos.get_mut(&key).expect("repository remains").state = Scan::Waiting(progress);
        d.alarms.arm(Alarm::Repository(key), env.now.saturating_add(env.limits.backoff)).expect("one repository alarm");
        return;
    };
    match answer {
        Answer::Items { items, more, now } => {
            let row = d.keep.repos.get_mut(&key).expect("repository remains");
            row.record.clock = row.record.clock.max(now);
            let begun = progress.begun.unwrap_or(now);
            progress.begun = Some(begun);
            for item in &items {
                progress.newest = progress.newest.max(item.updated);
                progress.moved |= item.updated >= begun;
                listed(d, env, key, item, now, out);
            }
            let row = d.keep.repos.get_mut(&key).expect("repository remains");
            if more && !progress.bootstrap {
                match items.last() {
                    Some(last) if last.updated > progress.since => {
                        progress.since = last.updated;
                        progress.page = 1;
                    }
                    Some(_) | None => {
                        progress.tied = Some(progress.tied.unwrap_or(progress.since));
                        progress.page = match progress.page.checked_add(1) {
                            Some(page) => page,
                            None => {
                                row.state = Scan::Waiting(Progress::first(row.record.mark));
                                d.alarms
                                    .arm(Alarm::Repository(key), env.now.saturating_add(env.limits.backoff))
                                    .expect("one repository alarm");
                                return;
                            }
                        };
                    }
                }
                row.state = Scan::Due(progress);
            } else {
                row.record.mark = Some(if progress.bootstrap {
                    now
                } else if progress.moved {
                    progress.tied.unwrap_or(progress.newest)
                } else {
                    progress.newest
                });
                row.state = Scan::Idle;
                row.fresh = Fresh::Complete;
                let interval = if row.hinted { env.limits.hinted } else { env.limits.poll };
                row.hinted = false;
                d.alarms.arm(Alarm::Repository(key), env.now.saturating_add(interval)).expect("one repository alarm");
            }
            out.push(Request::Save { record: Stored::Repository(row.record) });
        }
        Answer::Item { .. }
        | Answer::Pull(_)
        | Answer::Reviews { .. }
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::Commit(_)
        | Answer::Branches(_)
        | Answer::Compare { .. }
        | Answer::PullFiles { .. }
        | Answer::Checks(_)
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        // Only a merge is answered Queued; it never reaches a read.
        | Answer::Queued
        | Answer::Branch(_)
        | Answer::Located { .. }
        | Answer::Done => unreachable!("listing answer shape"),
    }
}
fn listed(
    d: &mut Domain,
    env: &Env<Limits>,
    repo: Repository,
    item: &api::Summary,
    now: Time,
    out: &mut Queue<Request>,
) {
    for key in super::keys(&d.keep) {
        let row = d.keep.live.get_mut(&key).expect("bounded key remains");
        let key = &key;
        if key.repository != repo || !row.active {
            continue;
        }
        let number = match key.what {
            What::Pull(number) | What::Issue(number) => number,
            What::Repository | What::Branch(_) => continue,
        };
        // Inclusive pages can repeat a time; reread an equal timestamp too.
        // The hint/cursor and response equality suppress duplicate news.
        if item.number == number
            && (item.updated > row.record.listed
                || row.record.cached.item.as_ref() != Some(item)
                || item.updated >= now)
        {
            row.record.listed = item.updated;
            out.push(Request::Save { record: Stored::Live(row.record.clone()) });
            super::poll::nudge_resource(&mut d.alarms, env, key, row);
        }
    }
}
pub(crate) fn answered_resource(
    d: &mut Domain,
    env: &Env<Limits>,
    key: &Resource,
    result: Result<Answer, Error>,
    out: &mut Queue<Request>,
) {
    let row = d.keep.live.get_mut(key).expect("call owns resource");
    if !row.active {
        d.keep.live.remove(key).expect("inactive resource exists");
        return;
    }
    let phase = match row.state {
        State::Busy(phase, _) => phase,
        State::Idle | State::Due(_, _) | State::Waiting(_, _) => unreachable!("resource terminal while busy"),
    };
    // A head or an absence read before this client's own push cannot judge
    // that push; a fresh read does. Other failures keep their backoff.
    let before_push = core::mem::replace(&mut row.read, Sent::Current) == Sent::BeforePush;
    let stale = match &result {
        Ok(_) => before_push,
        Err(error) => before_push && *error == Error::Missing,
    };
    if stale {
        row.state = State::Due(Phase::Branch, Priority::Fresh);
        return;
    }
    let answer = match result {
        Ok(answer) => answer,
        Err(error) => {
            failed_resource(d, env, key, phase, error, out);
            return;
        }
    };
    match answer {
        Answer::Item { item, comments, more } => item_answered(d, env, key, item, comments, more, out),
        Answer::Pull(pull) => {
            let row = d.keep.live.get_mut(key).expect("resource remains");
            row.presence = super::Presence::Present;
            let changed = row.record.cached.pull.as_ref() != Some(&pull);
            row.record.cached.pull = Some(pull.clone());
            out.push(Request::Save { record: Stored::Live(row.record.clone()) });
            if changed {
                row.interval = env.limits.poll;
                out.push(Request::Changed { resource: key.clone(), result: Ok(Answer::Pull(pull)) });
            }
            if row.record.watch.participating && row.inbox.reviews_needed {
                row.state = State::Due(Phase::Reviews(row.record.review_page), super::priority(row));
            } else {
                rest_resource(d, env, key);
            }
        }
        Answer::Reviews { reviews, more } => reviews_answered(d, env, key, phase, reviews, more, out),
        Answer::Commit(commit) => branch_answered(d, env, key, commit, out),
        Answer::Compare { before, after, contains_before, .. } => {
            branch_compared(d, env, key, before, after, contains_before, out);
        }
        Answer::Items { .. }
        | Answer::Located { .. }
        | Answer::Branches(_)
        | Answer::Statuses { .. }
        | Answer::Remarks { .. }
        | Answer::PullFiles { .. }
        | Answer::Checks(_)
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_)
        | Answer::Created(_)
        | Answer::Commented(_)
        | Answer::Reviewed(_)
        | Answer::Merged(_)
        // Only a merge is answered Queued; it never reaches a read.
        | Answer::Queued
        | Answer::Branch(_)
        | Answer::Done => unreachable!("resource operation answer shape"),
    }
}
fn item_answered(
    d: &mut Domain,
    env: &Env<Limits>,
    key: &Resource,
    item: api::Summary,
    comments: Box<[api::Comment]>,
    more: bool,
    out: &mut Queue<Request>,
) {
    let row = d.keep.live.get(key).expect("resource remains");
    let after = row.record.comment_after;
    let participating = row.record.watch.participating;
    if more && participating && row.record.comment_page >= env.limits.inbox {
        failed_resource(d, env, key, Phase::Object, Error::TooLarge, out);
        return;
    }
    let mut stamps = copied_stamps(&row.record.comments, env.limits.inbox);
    if participating {
        for comment in &comments {
            if !remember(&mut stamps, Delivery { id: comment.id, revision: comment.revision }) {
                failed_resource(d, env, key, Phase::Object, Error::TooLarge, out);
                return;
            }
        }
    }
    let mut last = after;
    let mut delivered = List::with_capacity(env.limits.rows);
    if participating {
        for comment in comments {
            last = last.max(comment.id);
            if unseen(&row.record.comments, Delivery { id: comment.id, revision: comment.revision })
                && (known_id(&row.record.comments, comment.id)
                    || !original(comment.provenance)
                    || (!super::echo::is_echo(&row.record.echoes, Echo::Comment(comment.id))
                        && !super::echo::key_echo(d, key.repository, comment.author, comment.key.as_deref())))
            {
                delivered.push(comment).expect("answer rows checked before retention");
            }
        }
    }
    if more && participating && last <= after {
        failed_resource(d, env, key, Phase::Object, Error::InvalidAnswer, out);
        return;
    }
    let row = d.keep.live.get_mut(key).expect("resource remains");
    if let What::Issue(_) = key.what {
        row.presence = super::Presence::Present;
    }
    let changed = row.record.cached.item.as_ref() != Some(&item);
    row.record.position.comment = row.record.position.comment.max(last);
    row.record.comment_after = if more && participating { last } else { row.record.position.comment };
    row.record.comment_page = if more && participating {
        row.record.comment_page.checked_add(1).expect("scan page admitted below cap")
    } else {
        1
    };
    row.record.comment_scanning = more && participating;
    row.inbox.reviews_needed = changed || row.record.comment_repair || row.record.review_page > 1;
    if !more {
        if row.record.comment_repair || row.record.cached.item.is_none() || super::priority(row) == Priority::Slow {
            row.inbox.repair_at = env.now.saturating_add(env.limits.slow);
        }
        row.record.comment_repair = false;
    }
    row.record.comments = stamps.into_boxed();
    row.record.cached.item = Some(item.clone());
    super::echo::prune_comments(row, last);
    out.push(Request::Save { record: Stored::Live(row.record.clone()) });
    if changed || !delivered.is_empty() {
        out.push(Request::Changed {
            resource: key.clone(),
            result: Ok(Answer::Item { item, comments: delivered.into_boxed(), more }),
        });
    }
    if more && participating {
        row.state = State::Due(Phase::Object, super::priority(row));
        return;
    }
    match key.what {
        What::Pull(_) => row.state = State::Due(Phase::Pull, super::priority(row)),
        What::Issue(_) => rest_resource(d, env, key),
        What::Repository | What::Branch(_) => unreachable!("item resource"),
    }
}
fn reviews_answered(
    d: &mut Domain,
    env: &Env<Limits>,
    key: &Resource,
    phase: Phase,
    reviews: Box<[api::Review]>,
    more: bool,
    out: &mut Queue<Request>,
) {
    let page = match phase {
        Phase::Reviews(page) => page,
        Phase::Object | Phase::Pull | Phase::Branch | Phase::BranchCompare { .. } => unreachable!("reviews stage"),
    };
    if more && page >= env.limits.inbox {
        failed_resource(d, env, key, phase, Error::TooLarge, out);
        return;
    }
    let next = if more {
        match page.checked_add(1) {
            Some(next) => next,
            None => {
                failed_resource(d, env, key, phase, Error::TooLarge, out);
                return;
            }
        }
    } else {
        1
    };
    let row = d.keep.live.get(key).expect("resource remains");
    let mut stamps = copied_stamps(&row.record.reviews, env.limits.inbox);
    for review in &reviews {
        if !remember(&mut stamps, Delivery { id: review.id, revision: review.revision }) {
            failed_resource(d, env, key, phase, Error::TooLarge, out);
            return;
        }
    }
    let mut delivered = List::with_capacity(env.limits.rows);
    for review in reviews {
        let row = d.keep.live.get(key).expect("resource remains");
        let fresh = unseen(&row.record.reviews, Delivery { id: review.id, revision: review.revision });
        let echo = !known_id(&row.record.reviews, review.id)
            && original(review.provenance)
            && (super::echo::is_echo(&row.record.echoes, Echo::Review(review.id))
                || super::echo::key_echo(d, key.repository, review.author, review.key.as_deref()));
        let row = d.keep.live.get_mut(key).expect("resource remains");
        super::echo::remove_echo(row, Echo::Review(review.id));
        row.record.position.review = row.record.position.review.max(review.id);
        if fresh && !echo {
            delivered.push(review).expect("bounded answer rows");
        }
    }
    let row = d.keep.live.get_mut(key).expect("resource remains");
    row.record.reviews = stamps.into_boxed();
    row.record.review_page = next;
    if more {
        row.state = State::Due(Phase::Reviews(next), super::priority(row));
    }
    out.push(Request::Save { record: Stored::Live(row.record.clone()) });
    if !delivered.is_empty() {
        out.push(Request::Changed {
            resource: key.clone(),
            result: Ok(Answer::Reviews { reviews: delivered.into_boxed(), more }),
        });
    }
    if !more {
        row.inbox.reviews_needed = false;
        rest_resource(d, env, key);
    }
}
pub(super) fn valid_deliveries(stamps: &[Delivery], cap: u32) -> bool {
    if stamps.len() > usize::try_from(cap).expect("u32 fits usize") {
        return false;
    }
    for (index, stamp) in stamps.iter().enumerate() {
        for previous in stamps.get(..index).expect("enumerated stamp index") {
            if previous.id == stamp.id {
                return false;
            }
        }
    }
    true
}
fn copied_stamps(stamps: &[Delivery], cap: u32) -> List<Delivery> {
    let mut copied = List::with_capacity(cap);
    for stamp in stamps {
        copied.push(*stamp).expect("restored stamp table bounded");
    }
    copied
}
fn original(provenance: api::Provenance) -> bool {
    match provenance {
        api::Provenance::Original => true,
        api::Provenance::Revised | api::Provenance::Unknown => false,
    }
}
fn known_id(stamps: &[Delivery], wanted: u64) -> bool {
    for stamp in stamps {
        if stamp.id == wanted {
            return true;
        }
    }
    false
}
fn unseen(stamps: &[Delivery], wanted: Delivery) -> bool {
    for stamp in stamps {
        if *stamp == wanted {
            return false;
        }
    }
    true
}
fn remember(stamps: &mut List<Delivery>, wanted: Delivery) -> bool {
    for index in 0..stamps.len() {
        let stamp = stamps.get_mut(index).expect("bounded stamp index");
        if stamp.id == wanted.id {
            stamp.revision = wanted.revision;
            return true;
        }
    }
    stamps.push(wanted).is_ok()
}
fn branch_answered(d: &mut Domain, env: &Env<Limits>, key: &Resource, commit: Commit, out: &mut Queue<Request>) {
    let row = d.keep.live.get(key).expect("resource remains");
    if row.writer
        && row.record.cached.tip != Some(commit)
        && let Some(before) = row.record.cached.tip
    {
        let row = d.keep.live.get_mut(key).expect("resource remains");
        row.state = State::Due(Phase::BranchCompare { before, after: commit }, Priority::Fresh);
        return;
    }
    branch_observed(d, env, key, commit, false, out);
}
fn branch_compared(
    d: &mut Domain,
    env: &Env<Limits>,
    key: &Resource,
    before: Commit,
    after: Commit,
    forward: bool,
    out: &mut Queue<Request>,
) {
    let row = d.keep.live.get(key).expect("resource remains");
    // A run answer can confirm a newer head while this comparison is in flight.
    // The comparison cannot overwrite that confirmation.
    if row.record.cached.tip != Some(before) {
        let row = d.keep.live.get_mut(key).expect("resource remains");
        row.state = State::Due(Phase::Branch, Priority::Fresh);
        return;
    }
    branch_observed(d, env, key, after, !forward, out);
}
fn branch_observed(
    d: &mut Domain,
    env: &Env<Limits>,
    key: &Resource,
    commit: Commit,
    backwards: bool,
    out: &mut Queue<Request>,
) {
    let row = d.keep.live.get(key).expect("resource remains");
    let outside = match row.pushed_by {
        Some((head, actor)) if head == commit => !crate::identity::writer(d, key.repository, actor),
        Some(_) | None => false,
    };
    let row = d.keep.live.get_mut(key).expect("resource remains");
    row.presence = super::Presence::Present;
    let changed = row.record.cached.tip != Some(commit);
    let expected = if row.writer { row.record.cached.tip } else { row.record.pushed };
    if (backwards || outside || !row.writer)
        && changed
        && let Some(expected) = expected
        && expected != commit
    {
        out.push(Request::Drift { resource: key.clone(), expected, observed: Some(commit) });
    }
    row.pushed_by = None;
    row.record.cached.tip = Some(commit);
    out.push(Request::Save { record: Stored::Live(row.record.clone()) });
    if changed && !row.writer {
        out.push(Request::Changed { resource: key.clone(), result: Ok(Answer::Commit(commit)) });
    }
    rest_resource(d, env, key);
}
fn failed_resource(
    d: &mut Domain,
    env: &Env<Limits>,
    key: &Resource,
    phase: Phase,
    error: Error,
    out: &mut Queue<Request>,
) {
    if error == Error::Missing {
        let row = d.keep.live.get_mut(key).expect("resource remains");
        if row.presence != super::Presence::Missing {
            if let What::Branch(_) = key.what
                && let Some(expected) = row.record.cached.tip.take()
            {
                out.push(Request::Drift { resource: key.clone(), expected, observed: None });
                out.push(Request::Save { record: Stored::Live(row.record.clone()) });
            }
            out.push(Request::Changed { resource: key.clone(), result: Err(error) });
        }
        row.presence = super::Presence::Missing;
        rest_resource(d, env, key);
        return;
    }
    out.push(Request::Changed { resource: key.clone(), result: Err(error) });
    let row = d.keep.live.get_mut(key).expect("resource remains");
    row.state = State::Waiting(phase, Priority::Slow);
    d.alarms.arm(Alarm::Resource(row.alarm), env.now.saturating_add(env.limits.backoff)).expect("one resource alarm");
}
fn rest_resource(d: &mut Domain, env: &Env<Limits>, key: &Resource) {
    let row = d.keep.live.get_mut(key).expect("resource remains");
    row.fresh = Fresh::Complete;
    let interval = if row.hinted {
        env.limits.hinted
    } else if row.presence == super::Presence::Missing {
        env.limits.slow
    } else {
        match key.what {
            What::Pull(_) => {
                let interval = row.interval;
                row.interval = interval.saturating_mul(2).min(env.limits.poll_max);
                interval.min(env.limits.slow)
            }
            What::Branch(_) | What::Issue(_) | What::Repository => env.limits.slow,
        }
    };
    row.hinted = false;
    row.state = State::Idle;
    let next = env.now.saturating_add(interval);
    let next = if row.presence != super::Presence::Missing && row.inbox.repair_at > env.now {
        next.min(row.inbox.repair_at)
    } else {
        next
    };
    d.alarms.arm(Alarm::Resource(row.alarm), next).expect("one resource alarm");
}
