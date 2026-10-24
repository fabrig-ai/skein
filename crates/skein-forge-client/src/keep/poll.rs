use super::{
    Alarm, Change, Commit, Domain, Env, Fact, Fresh, Limits, Live, Op, Owner, Phase, Priority, Progress, Queue, Read,
    Repository, Request, Resource, Scan, Sent, State, Stored, What, api, calls,
};

pub(crate) fn pump(d: &mut Domain, _env: &Env<Limits>, out: &mut Queue<Request>) -> bool {
    if !d.keep.is_ready() || d.calls.is_full() {
        return false;
    }
    for key in super::repos(&d.keep) {
        let row = d.keep.repos.get_mut(&key).expect("bounded key remains");
        let key = &key;
        match row.state {
            Scan::Due(progress) if row.active => {
                row.state = Scan::Busy(progress);
                calls::queue(
                    &mut d.calls,
                    Owner::Repository(*key),
                    *key,
                    Op::Read(Read::Items { since: progress.since, page: progress.page, kind: None }),
                    if row.fresh == Fresh::Pending { Priority::Fresh } else { Priority::Keep },
                );
                return true;
            }
            Scan::Due(_) | Scan::Idle | Scan::Busy(_) | Scan::Waiting(_) => {}
        }
    }
    for key in super::keys(&d.keep) {
        let row = d.keep.live.get_mut(&key).expect("bounded key remains");
        let key = &key;
        match row.state {
            State::Due(phase, priority) if row.active => {
                // Repository watches use only the repository listing.
                match key.what {
                    What::Repository => {
                        row.state = State::Idle;
                        continue;
                    }
                    What::Pull(_) | What::Issue(_) | What::Branch(_) => {}
                }
                prepare_inbox(row, phase, priority, out);
                let read = operation(row, key, phase);
                row.state = State::Busy(phase, priority);
                calls::queue(&mut d.calls, Owner::Resource(key.clone()), key.repository, Op::Read(read), priority);
                return true;
            }
            State::Due(_, _) | State::Idle | State::Busy(_, _) | State::Waiting(_, _) => {}
        }
    }
    false
}
fn prepare_inbox(row: &mut Live, phase: Phase, priority: Priority, out: &mut Queue<Request>) {
    match phase {
        Phase::Object if row.record.watch.participating => {
            if !row.record.comment_scanning {
                if priority == Priority::Slow || row.inbox.needs_repair {
                    row.record.comment_after = 0;
                    row.record.comment_repair = true;
                }
                row.record.comment_scanning = true;
                row.record.comment_page = 1;
                row.inbox.needs_repair = false;
                out.push(Request::Save { record: Stored::Live(row.record.clone()) });
            }
        }
        Phase::Object | Phase::Pull | Phase::Reviews(_) | Phase::Branch | Phase::BranchCompare { .. } => {}
    }
}
fn operation(row: &Live, key: &Resource, phase: Phase) -> Read {
    match phase {
        Phase::BranchCompare { before, after } => Read::Compare { before, after },
        Phase::Object => match key.what {
            What::Pull(number) | What::Issue(number) => Read::Item {
                number,
                after: if row.record.watch.participating { row.record.comment_after } else { u64::MAX },
            },
            What::Repository | What::Branch(_) => unreachable!("item stage"),
        },
        Phase::Pull => match key.what {
            What::Pull(number) => Read::Pull { number },
            What::Repository | What::Issue(_) | What::Branch(_) => unreachable!("pull stage"),
        },
        Phase::Reviews(page) => match key.what {
            What::Pull(number) => Read::Reviews { number, page },
            What::Repository | What::Issue(_) | What::Branch(_) => unreachable!("reviews stage"),
        },
        Phase::Branch => match &key.what {
            What::Branch(branch) => Read::Branch { branch: branch.clone() },
            What::Repository | What::Issue(_) | What::Pull(_) => unreachable!("branch stage"),
        },
    }
}
pub(crate) fn due_resource(d: &mut Domain, env: &Env<Limits>, alarm: u64) {
    let mut found = None;
    for (key, row) in &d.keep.live {
        if row.alarm == alarm {
            found = Some(key.clone());
            break;
        }
    }
    let key = found.expect("alarm owns resource");
    let key = &key;
    let row = d.keep.live.get_mut(key).expect("alarm owns resource");
    row.state = match row.state {
        State::Waiting(phase, priority) => State::Due(phase, priority),
        State::Idle => {
            State::Due(super::first(key), if env.now >= row.inbox.repair_at { Priority::Slow } else { Priority::Keep })
        }
        State::Due(_, _) | State::Busy(_, _) => unreachable!("idle resource alarm"),
    };
}
pub(crate) fn due_repository(d: &mut Domain, key: Repository) {
    let row = d.keep.repos.get_mut(&key).expect("alarm owns repository");
    row.state = match row.state {
        Scan::Waiting(progress) => Scan::Due(progress),
        Scan::Idle => Scan::Due(Progress::first(row.record.mark)),
        Scan::Due(_) | Scan::Busy(_) => unreachable!("idle repository alarm"),
    };
}
pub(crate) fn writer(d: &mut Domain, env: &Env<Limits>, key: &Resource, taken: bool) {
    if !super::valid_resource(key, &env.limits) {
        d.fact(Fact::Refused);
        return;
    }
    if let Some(row) = d.keep.live.get_mut(key) {
        if row.writer && !taken && row.active {
            match row.state {
                State::Idle | State::Waiting(_, _) => {
                    d.alarms.cancel(Alarm::Resource(row.alarm));
                    row.state = State::Due(super::first(key), Priority::Fresh);
                }
                State::Due(_, _) | State::Busy(_, _) => row.hinted = true,
            }
        }
        row.writer = taken;
    }
}
pub(crate) fn pushed(d: &mut Domain, env: &Env<Limits>, key: &Resource, commit: Commit, out: &mut Queue<Request>) {
    if !super::valid_resource(key, &env.limits) {
        d.fact(Fact::Refused);
        return;
    }
    if let Some(row) = d.keep.live.get_mut(key)
        && row.active
    {
        match key.what {
            What::Branch(_) => {}
            What::Repository | What::Pull(_) | What::Issue(_) => return,
        }
        row.presence = super::Presence::Present;
        if let State::Busy(Phase::Branch, _) = row.state {
            row.read = Sent::BeforePush;
        }
        row.record.pushed = Some(commit);
        row.record.cached.tip = Some(commit);
        out.push(Request::Save { record: Stored::Live(row.record.clone()) });
    }
}
pub(crate) fn hint(d: &mut Domain, env: &Env<Limits>, hint: api::Hint) {
    let valid = match &hint.change {
        Change::Branch(name) | Change::BranchMoved { branch: name, .. } => {
            name.len() <= usize::try_from(env.limits.op_bytes).expect("u32 fits usize")
        }
        Change::Item(_) | Change::Commit(_) => true,
    };
    if !valid {
        d.fact(Fact::Refused);
        return;
    }
    match &hint.key {
        Some(key) if key.len() > usize::try_from(env.limits.op_bytes).expect("u32 fits usize") => {
            d.fact(Fact::Refused);
            return;
        }
        // Hint keys carry no authenticated author; a copied marker must
        // still cause a read so the returned row can establish provenance.
        Some(_) | None => {}
    }
    for key in super::keys(&d.keep) {
        let row = d.keep.live.get_mut(&key).expect("bounded key remains");
        let key = &key;
        if key.repository != hint.repository || !row.active {
            continue;
        }
        let affected = affected(row, key, &hint.change);
        if affected {
            if let Change::BranchMoved { head, actor, .. } = &hint.change {
                row.pushed_by = Some((*head, *actor));
            }
            match hint.change {
                Change::Item(_) => row.inbox.needs_repair = true,
                Change::Branch(_) | Change::BranchMoved { .. } | Change::Commit(_) => {}
            }
            nudge_resource(&mut d.alarms, env, key, row);
        }
    }
    if let Some(row) = d.keep.repos.get_mut(&hint.repository)
        && row.active
    {
        match row.state {
            Scan::Idle => {
                row.state = Scan::Waiting(Progress::first(row.record.mark));
                d.alarms
                    .arm(Alarm::Repository(hint.repository), env.now.saturating_add(env.limits.hinted))
                    .expect("one repository alarm");
            }
            Scan::Due(_) | Scan::Busy(_) | Scan::Waiting(_) => row.hinted = true,
        }
    }
    crate::outbox::queue::hinted(d, env, &hint);
}
fn affected(row: &Live, key: &Resource, change: &Change) -> bool {
    match change {
        Change::Item(number) => match key.what {
            What::Pull(n) | What::Issue(n) => n == *number,
            What::Repository => true,
            What::Branch(_) => false,
        },
        Change::Branch(name) | Change::BranchMoved { branch: name, .. } => match &key.what {
            What::Branch(branch) => branch == name,
            What::Pull(_) => match &row.record.cached.pull {
                Some(pull) => pull.head == *name || pull.base == *name,
                None => true,
            },
            What::Repository => true,
            What::Issue(_) => false,
        },
        Change::Commit(commit) => match key.what {
            What::Pull(_) => match &row.record.cached.pull {
                Some(pull) => pull.commit == *commit || pull.base_commit == Some(*commit),
                None => true,
            },
            What::Branch(_) => row.record.cached.tip == Some(*commit),
            What::Repository => true,
            What::Issue(_) => false,
        },
    }
}
pub(super) fn nudge_resource(
    alarms: &mut skein_lib::Deadlines<Alarm>,
    env: &Env<Limits>,
    key: &Resource,
    row: &mut Live,
) {
    row.interval = env.limits.poll;
    match row.state {
        State::Idle => {
            row.state = State::Waiting(super::first(key), Priority::Keep);
            alarms
                .arm(Alarm::Resource(row.alarm), env.now.saturating_add(env.limits.hinted))
                .expect("one resource alarm");
        }
        State::Due(_, _) | State::Busy(_, _) | State::Waiting(_, _) => row.hinted = true,
    }
}
