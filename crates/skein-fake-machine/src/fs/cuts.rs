//! File lengths become durable with the file's sync (simulator.md, 3.3).
use super::{Body, LARGEST_FILE, Machine, Opened, Refusal, fail};

impl Machine {
    /// Sets the open file's length, zeros filling any extension.
    pub fn truncate(&mut self, file: Opened, length: u64) -> Result<(), Refusal> {
        let length = usize::try_from(length).map_err(|_| Refusal::NoSpace)?;
        if length > LARGEST_FILE {
            return Err(Refusal::NoSpace);
        }
        let node = self.handle(file).node;
        let Body::File(contents) = &mut self.node_mut(node).body else {
            fail("only a file is opened to cut");
        };
        contents.resize(length, 0);
        Ok(())
    }
}
