//! The page holds outgoing messages until the worker's handler is installed.
use std::collections::VecDeque;

#[derive(Default)]
pub(super) struct Hold {
    ready: bool,
    pending: VecDeque<Box<[u8]>>,
}

impl Hold {
    pub fn push(&mut self, bytes: Box<[u8]>) {
        self.pending.push_back(bytes);
    }

    /// The first incoming message releases the hold, once.
    pub fn incoming(&mut self) -> bool {
        !std::mem::replace(&mut self.ready, true)
    }

    pub fn outgoing(&mut self) -> Option<Box<[u8]>> {
        if self.ready { self.pending.pop_front() } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::Hold;

    const MESSAGES: [&[u8]; 3] = [b"start", b"pace", b"exchange"];

    #[test]
    fn nothing_is_posted_before_the_workers_first_message() {
        let mut hold = Hold::default();
        assert_eq!(hold.outgoing(), None);
        for message in MESSAGES {
            hold.push(message.into());
            assert_eq!(hold.outgoing(), None);
        }
    }

    #[test]
    fn the_first_incoming_releases_every_message_in_order() {
        let mut hold = Hold::default();
        for message in MESSAGES {
            hold.push(message.into());
        }
        assert!(hold.incoming());
        for message in MESSAGES {
            assert_eq!(hold.outgoing().as_deref(), Some(message));
        }
        assert_eq!(hold.outgoing(), None);
    }

    #[test]
    fn later_messages_need_no_further_handshake() {
        let mut hold = Hold::default();
        assert!(hold.incoming());
        assert!(!hold.incoming());
        for message in MESSAGES {
            hold.push(message.into());
            assert_eq!(hold.outgoing().as_deref(), Some(message));
            assert!(!hold.incoming());
        }
        assert_eq!(hold.outgoing(), None);
    }
}
