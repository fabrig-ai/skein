use super::{Event, Pid, Sim};

impl Sim {
    /// Records `digest` under a process's pid or `Pid::MACHINE` at the present time.
    pub fn trace_digest(&mut self, pid: Pid, digest: [u8; 32]) {
        assert!(
            pid == Pid::MACHINE || usize::try_from(pid.raw()).expect("pid fits") < self.processes.len(),
            "a process of this simulator or the machine"
        );
        self.record(pid, Event::Digest(digest));
    }
}
