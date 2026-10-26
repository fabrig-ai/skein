//! Exclusive locks belong to open handles (simulator.md, 3.1).
use super::{How, Machine, Opened, Refusal};

impl Machine {
    /// Takes the lock, or keeps it through the same handle, without waiting.
    pub fn lock(&mut self, file: Opened) -> Result<(), Refusal> {
        let handle = self.handle(file);
        assert_ne!(handle.how, How::PathNoFollow, "only Stat and Close may use a path-only handle");
        let node = handle.node;
        if self.handles.iter().any(|(other, handle)| *other != file.raw() && handle.node == node && handle.locked) {
            return Err(Refusal::Held);
        }
        self.handles.get_mut(&file.raw()).expect("a held handle").locked = true;
        Ok(())
    }
}
