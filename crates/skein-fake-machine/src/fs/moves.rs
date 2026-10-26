//! Cross-directory moves stay atomic through power loss (simulator.md, 3.3).
use super::{Body, Machine, Node, NodeId, draw};
use alloc::{boxed::Box, collections::BTreeMap};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(super) struct Move {
    node: NodeId,
    from_dir: NodeId,
    from: Box<[u8]>,
    to_dir: NodeId,
    to: Box<[u8]>,
    durable_from: Option<NodeId>,
    durable_to: Option<NodeId>,
}

fn entries(nodes: &mut BTreeMap<NodeId, Node>, dir: NodeId) -> Option<&mut BTreeMap<Box<[u8]>, NodeId>> {
    let Body::Directory { entries, .. } = &mut nodes.get_mut(&dir)?.body else {
        super::fail("a move names directories");
    };
    Some(entries)
}

fn restore(names: Option<&mut BTreeMap<Box<[u8]>, NodeId>>, name: &[u8], node: Option<NodeId>) {
    let Some(names) = names else { return };
    if let Some(node) = node {
        names.insert(Box::from(name), node);
    } else {
        names.remove(name);
    }
}

fn parent(nodes: &mut BTreeMap<NodeId, Node>, id: NodeId, directory: NodeId) {
    if let Some(node) = nodes.get_mut(&id) {
        match node.body {
            Body::Directory { .. } => node.parent = Some(directory),
            Body::File(_) | Body::Link(_) | Body::Special(_) => {}
        }
    }
}

impl Machine {
    pub(super) fn remember_move(&mut self, node: NodeId, from_dir: NodeId, from: &[u8], to_dir: NodeId, to: &[u8]) {
        let durable_from = entries(&mut self.durable_nodes, from_dir).and_then(|names| names.get(from).copied());
        let durable_to = entries(&mut self.durable_nodes, to_dir).and_then(|names| names.get(to).copied());
        self.pending_moves.push(Move {
            node,
            from_dir,
            from: Box::from(from),
            to_dir,
            to: Box::from(to),
            durable_from,
            durable_to,
        });
    }

    pub(super) fn sync_moves(&mut self, directory: NodeId) {
        for moved in core::mem::take(&mut self.pending_moves) {
            if moved.to_dir == directory {
                parent(&mut self.durable_nodes, moved.node, moved.to_dir);
                restore(entries(&mut self.durable_nodes, moved.from_dir), &moved.from, None);
            } else {
                if moved.from_dir == directory {
                    restore(entries(&mut self.durable_nodes, directory), &moved.from, moved.durable_from);
                }
                self.pending_moves.push(moved);
            }
        }
    }

    pub(super) fn crash_moves(&mut self, surviving: &mut BTreeMap<NodeId, Node>, rng: &mut u64) {
        for moved in core::mem::take(&mut self.pending_moves) {
            *rng = draw(*rng);
            if *rng & 1 != 0 {
                parent(surviving, moved.node, moved.to_dir);
                restore(entries(surviving, moved.to_dir), &moved.to, Some(moved.node));
                restore(entries(surviving, moved.from_dir), &moved.from, None);
            } else {
                parent(surviving, moved.node, moved.from_dir);
                restore(entries(surviving, moved.to_dir), &moved.to, moved.durable_to);
                restore(entries(surviving, moved.from_dir), &moved.from, moved.durable_from);
            }
        }
    }
}
