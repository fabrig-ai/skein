use super::{encode, encoded_len};
use alloc::boxed::Box;
use skein_fake_forge_domain::{Domain, Limits, Observation, promises::Recorded};
use skein_lib::{List, Overflow, Queue, bytes::copy_of};

/// Drains observations in order into the caller's buffer; holds no io.
/// Moved `forward` asks `Domain::is_ancestor`: `to` descends from `from`.
/// A remake uses that branch's last deleted `at`; a first creation has no answer.
/// Repository slots and per-repository name-keyed deletion queues allocate at construction.
/// Each queue holds `Limits::branches`, consuming remakes and evicting the oldest past it.
/// Non-UTF-8 names have no line and add to the domain's cumulative lost count.
/// A drain that popped anything or saw loss grow ends with one checkpoint; an idle drain writes nothing.
/// `stop` drains and ends with a checkpoint always; the caller owns the file.
#[derive(Debug)]
pub struct Writer {
    repositories: List<Repository>,
    observations: u32,
    pending: Option<Recorded>,
    omitted: u64,
    checkpoint: u64,
    due: bool,
}
#[derive(Debug)]
struct Repository {
    name: Option<Box<[u8]>>,
    deleted: Queue<Deletion>,
}
#[derive(Debug)]
struct Deletion {
    branch: Box<[u8]>,
    at: u64,
}

impl Writer {
    /// Allocates `repositories` slots, each with a `branches`-entry deletion queue.
    #[must_use]
    pub fn new(limits: &Limits) -> Writer {
        let mut repositories = List::with_capacity(limits.repositories);
        for _ in 0..limits.repositories {
            repositories
                .push(Repository { name: None, deleted: Queue::with_capacity(limits.branches) })
                .expect("one slot per repository");
        }
        Writer { repositories, observations: limits.observations, pending: None, omitted: 0, checkpoint: 0, due: false }
    }

    /// Appends a drain and its checkpoint, or nothing when no observation or loss arrived.
    /// On insufficient room, keeps the next record for a later call; complete lines already written stand.
    pub fn drain(&mut self, domain: &mut Domain, out: &mut List<u8>) -> Result<(), Overflow> {
        self.flush(domain, out, false)
    }

    /// Drains and writes a final checkpoint, including when nothing arrived.
    pub fn stop(&mut self, domain: &mut Domain, out: &mut List<u8>) -> Result<(), Overflow> {
        self.flush(domain, out, true)
    }

    fn flush(&mut self, domain: &mut Domain, out: &mut List<u8>, stop: bool) -> Result<(), Overflow> {
        self.due |= stop;
        // There is at most one saved observation and one bounded domain queue.
        for _ in 0..=self.observations {
            let record = match self.pending.take() {
                Some(record) => record,
                None => {
                    let Some(observation) = domain.pop_observation() else {
                        break;
                    };
                    self.due = true;
                    let forward = self.forward(domain, &observation);
                    Recorded::Observed { observation, forward }
                }
            };
            match encoded_len(&record) {
                Some(size) => {
                    if out.room() < size {
                        self.pending = Some(record);
                        return Err(Overflow);
                    }
                    encode(&record, out).expect("measured line admitted");
                }
                None => self.omitted = self.omitted.saturating_add(1),
            }
        }
        let lost = domain.observations_lost().saturating_add(self.omitted);
        self.due |= lost > self.checkpoint;
        if self.due {
            encode(&Recorded::Lost(lost), out).ok_or(Overflow)?;
            self.checkpoint = lost;
            self.due = false;
        }
        Ok(())
    }

    pub(crate) fn forward(&mut self, domain: &Domain, observation: &Observation) -> Option<bool> {
        match observation {
            Observation::Moved { repository, branch, from, to, .. } => {
                let history = self.repository(repository);
                let deleted = history.take(branch);
                let at = from.or(deleted)?;
                Some(domain.is_ancestor(at, *to))
            }
            Observation::Deleted { repository, branch, at, .. } => {
                self.repository(repository).remember(branch, *at);
                None
            }
            Observation::Opened { .. }
            | Observation::Closed { .. }
            | Observation::Reopened { .. }
            | Observation::Labelled { .. }
            | Observation::Revised { .. }
            | Observation::Depends { .. }
            | Observation::Requested { .. }
            | Observation::Defined { .. }
            | Observation::Commented { .. }
            | Observation::Edited { .. }
            | Observation::Removed { .. }
            | Observation::Reviewed { .. }
            | Observation::Reported { .. }
            | Observation::Rebased { .. }
            | Observation::Merged { .. }
            | Observation::Enqueued { .. }
            | Observation::Group { .. }
            | Observation::Dequeued { .. }
            | Observation::Refused { .. }
            | Observation::Rejected { .. }
            | Observation::Wiki { .. } => None,
        }
    }

    fn repository(&mut self, name: &[u8]) -> &mut Repository {
        let mut empty = None;
        let mut found = None;
        for (index, repository) in self.repositories.iter().enumerate() {
            let index = u32::try_from(index).expect("repository count is u32");
            match &repository.name {
                Some(kept) if kept.as_ref() == name => {
                    found = Some(index);
                    break;
                }
                Some(_) => {}
                None => {
                    empty = Some(index);
                }
            }
        }
        let index = found.or(empty).expect("writer and domain have the same repository bound");
        let repository = self.repositories.get_mut(index).expect("repository slot");
        if repository.name.is_none() {
            repository.name = Some(copy_of(name));
        }
        repository
    }
}

impl Repository {
    fn take(&mut self, branch: &[u8]) -> Option<u64> {
        let mut found = None;
        for _ in 0..self.deleted.len() {
            let entry = self.deleted.pop().expect("one pass over the held deletions");
            if entry.branch.as_ref() == branch {
                found = Some(entry.at);
            } else {
                self.deleted.push(entry);
            }
        }
        found
    }
    fn remember(&mut self, branch: &[u8], at: u64) {
        let _previous = self.take(branch);
        if self.deleted.capacity() == 0 {
            return;
        }
        if self.deleted.room() == 0 {
            drop(self.deleted.pop());
        }
        self.deleted.push(Deletion { branch: copy_of(branch), at });
    }
}
