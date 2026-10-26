use super::Machine;

impl Machine {
    /// A digest of every field of the fake filesystem (testing.md, 5).
    #[must_use]
    pub fn state_digest(&self) -> [u8; 32] {
        skein_lib::state_digest(self)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{How, Item, Machine};

    #[test]
    fn a_pending_move_changes_the_machine_s_digest() {
        let mut machine = Machine::new();
        let root = machine.lay(&[Item::directory(b"from"), Item::directory(b"to"), Item::file(b"from/file", b"bytes")]);
        let from = machine.open(root, b"from", How::Directory).expect("source");
        let to = machine.open(root, b"to", How::Directory).expect("destination");
        machine.rename(from, b"file", to, b"file", false).expect("move");
        assert_eq!(machine.pending_moves.len(), 1);
        let pending = machine.state_digest();
        machine.pending_moves.clear();
        assert_ne!(machine.state_digest(), pending);
    }
}
