//! Independently opened startup descriptors shared by hosted launches and restarts.

use super::{Host, Inherited, Machine, PreparedStartup, Restart, StartupRoot, World};
use crate::StartupAppend;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use skein_io::kernel::Error;
use skein_sim::Pid;

impl<P: Host, M: Machine, H: crate::Meter> World<P, M, H> {
    /// Adds a restartable process with independently opened roots and append files.
    pub fn spawn_restartable_startup(
        &mut self,
        roots: Vec<StartupRoot>,
        appends: Vec<StartupAppend>,
        make: fn(&Inherited) -> P,
    ) -> usize {
        crate::program::check_roots(&roots);
        crate::program::check_appends(&appends);
        let pid = self.sim.spawn_process();
        let inherited = self.restart_inherited(pid, &roots, &appends);
        self.signals.insert(self.next_host, (pid, inherited.signal));
        let at = self.admit(pid, None, || make(&inherited));
        *self.restart.get_mut(at).expect("a slot per process") = Some(Restart { roots, appends, make });
        at
    }

    pub(super) fn restart_inherited(
        &mut self,
        pid: Pid,
        roots: &[StartupRoot],
        appends: &[StartupAppend],
    ) -> Inherited {
        let prepared = self.open_startup(roots, appends).unwrap_or_else(|(what, error)| {
            crate::fail(&format!("a restartable {what} opens: {error:?}"));
        });
        let roots = prepared.roots.into_iter().map(|(name, handle)| (name, self.sim.root(pid, handle))).collect();
        let appends = prepared.appends.into_iter().map(|(name, handle)| (name, self.sim.append(pid, handle))).collect();
        Inherited { roots, dir: None, pipes: Vec::new(), appends, signal: self.sim.open_signal_source(pid) }
    }

    pub(super) fn rollback_startup(&mut self, prepared: PreparedStartup) {
        for (_, handle) in prepared.roots {
            self.machine.close_root(handle);
        }
        for (_, handle) in prepared.appends {
            self.machine.close_append(handle);
        }
        if let Some(handle) = prepared.dir {
            self.machine.close_root(handle);
        }
    }

    pub(super) fn open_startup(
        &mut self,
        declarations: &[StartupRoot],
        append_declarations: &[StartupAppend],
    ) -> Result<PreparedStartup, (String, Error)> {
        let mut prepared = PreparedStartup { dir: None, roots: Vec::new(), appends: Vec::new() };
        let result = (|| {
            for declaration in declarations {
                let handle = self
                    .machine
                    .open_root(&declaration.path)
                    .map_err(|error| (format!("startup root {}", String::from_utf8_lossy(&declaration.name)), error))?;
                prepared.roots.push((declaration.name.clone(), handle));
            }
            for declaration in append_declarations {
                let what = || format!("startup append {}", String::from_utf8_lossy(&declaration.name));
                let root = self.machine.open_root(&declaration.root).map_err(|error| (what(), error))?;
                let opened = self.machine.open_append(root, &declaration.path, declaration.mode);
                self.machine.close_root(root);
                let file = opened.map_err(|error| (what(), error))?;
                prepared.appends.push((declaration.name.clone(), file));
            }
            Ok(())
        })();
        match result {
            Ok(()) => Ok(prepared),
            Err(error) => {
                self.rollback_startup(prepared);
                Err(error)
            }
        }
    }
}
