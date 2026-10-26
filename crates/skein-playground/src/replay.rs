//! A log's inputs at their stamps, checked against the replay (playground.md, 6).

use skein_ui_web::bridge::{
    Duty, Said, Stamp,
    log::{End, Fold, Input, Log},
};
use std::collections::VecDeque;

/// A log's inputs fed to a replayed world at their stamps, and its digests
/// checked. The application's loop moves its world and supplies its outputs.
pub struct Feed {
    build: Box<[u8]>,
    inputs: VecDeque<Input>,
    input: usize,
    end: End,
    fold: Fold,
    differed: Option<([u8; 32], [u8; 32])>,
}

impl Feed {
    #[must_use]
    pub fn new(log: Log) -> Self {
        Self {
            build: log.build,
            inputs: log.inputs.into(),
            input: 0,
            end: log.end,
            fold: Fold::default(),
            differed: None,
        }
    }

    /// The next input enters before this step, at this instant.
    #[must_use]
    pub fn next(&self) -> Option<Stamp> {
        self.inputs.front().map(|input| input.stamp)
    }

    /// Check what was said since the previous input, then give this input.
    /// Panics if `at` is not `next()`, the replay loop's contract.
    pub fn take(&mut self, at: Stamp) -> Result<Duty, Differs> {
        assert_eq!(Some(at), self.next(), "the replay takes the next input at its stamp");
        let logged = self.inputs.front().expect("next names an input").digest;
        if self.differed.is_none() {
            let replayed = self.fold.take();
            if logged != replayed {
                self.differed = Some((logged, replayed));
            }
        }
        if let Some((logged, replayed)) = self.differed {
            return Err(self.differs(logged, replayed));
        }
        let input = self.inputs.pop_front().expect("next names an input");
        self.input += 1;
        Ok(input.duty)
    }

    /// What the replayed world said, in order.
    pub fn said(&mut self, said: &Said) {
        self.fold.said(said);
    }

    /// After the last input, check the end's digest; a cut has no end digest.
    /// Panics if the replay loop has left inputs untaken.
    pub fn finish(mut self) -> Result<(), Differs> {
        if let Some((logged, replayed)) = self.differed {
            return Err(self.differs(logged, replayed));
        }
        assert!(self.inputs.is_empty(), "the replay takes every input before finishing");
        match self.end {
            End::Said(logged) => {
                let replayed = self.fold.take();
                if logged != replayed {
                    return Err(self.differs(logged, replayed));
                }
            }
            End::Cut => {}
        }
        Ok(())
    }

    fn differs(&self, logged: [u8; 32], replayed: [u8; 32]) -> Differs {
        Differs { input: self.input, logged, replayed, build: self.build.clone() }
    }
}

/// The replay's first difference, at an input (from 0) or at the end (the
/// inputs' count), with the build the log came from for the failure's words.
#[derive(Debug, PartialEq, Eq)]
pub struct Differs {
    pub input: usize,
    pub logged: [u8; 32],
    pub replayed: [u8; 32],
    pub build: Box<[u8]>,
}
