//! Existing and created file opens beneath a root.
use super::{Body, How, Machine, Opened, READ, Refusal, UMASK, WRITE, fail};
use alloc::vec::Vec;
impl Machine {
    /// Opens `path`, resolved beneath the directory `root` has open, as
    /// `how` says.
    pub fn open(&mut self, root: Opened, path: &[u8], how: How) -> Result<Opened, Refusal> {
        let root = self.handle(root).node;
        let node = match how {
            How::PathNoFollow
            | How::Read
            | How::ReadNoFollow
            | How::Write
            | How::WriteNoFollow
            | How::Directory
            | How::DirectoryNoFollow => {
                let path_only = matches!(how, How::PathNoFollow);
                let no_follow =
                    path_only || matches!(how, How::ReadNoFollow | How::WriteNoFollow | How::DirectoryNoFollow);
                let found = self.resolve_path(root, path, false, no_follow, path_only)?;
                let node = found.node.ok_or(Refusal::NotFound)?;
                if (found.slash || matches!(how, How::Directory | How::DirectoryNoFollow)) && !self.is_directory(node) {
                    return Err(Refusal::NotADirectory);
                }
                if !path_only && !self.may(node, READ) {
                    return Err(Refusal::Permission);
                }
                if matches!(how, How::Write | How::WriteNoFollow) {
                    if self.is_directory(node) {
                        return Err(Refusal::IsADirectory);
                    }
                    if !self.may(node, WRITE) {
                        return Err(Refusal::Permission);
                    }
                }
                // Opened without blocking, then refused for what it is.
                if !path_only && let Body::Special(_) = self.node(node).body {
                    return Err(Refusal::NotAFile);
                }
                node
            }
            How::Append { mode } => {
                let found = self.resolve(root, path, false, false)?;
                if found.slash {
                    return Err(Refusal::IsADirectory);
                }
                match found.node {
                    Some(node) => {
                        match &self.node(node).body {
                            Body::Directory { .. } => return Err(Refusal::IsADirectory),
                            Body::Special(_) => return Err(Refusal::NotAFile),
                            Body::File(_) => {}
                            Body::Link(_) => fail("resolution follows links"),
                        }
                        if !self.may(node, WRITE) {
                            return Err(Refusal::Permission);
                        }
                        node
                    }
                    None => {
                        let name = found.name.ok_or(Refusal::NotFound)?;
                        if !self.may(found.parent, WRITE) {
                            return Err(Refusal::Permission);
                        }
                        self.make(found.parent, name, Body::File(Vec::new()), mode & !UMASK)
                    }
                }
            }
            How::Create { mode } | How::CreateNoFollow { mode } => {
                let found = self.resolve(root, path, true, matches!(how, How::CreateNoFollow { .. }))?;
                // A path that ends at a directory of no name of its own
                // (`.`, `a/..`) is taken: O_EXCL answers before O_CREAT.
                let Some(name) = found.name else {
                    return Err(Refusal::Exists);
                };
                if found.node.is_some() {
                    return Err(Refusal::Exists);
                }
                if !self.may(found.parent, WRITE) {
                    return Err(Refusal::Permission);
                }
                self.make(found.parent, name, Body::File(Vec::new()), mode & !UMASK)
            }
        };
        Ok(self.issue(node, how))
    }
}
