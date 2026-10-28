//! Resource limits for one browser machine.

use skein_lib::Duration;

/// Limits chosen by the test owner and fixed for the machine's lifetime.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Limits {
    pub persons: u32,
    pub pages: u32,
    pub ops: u32,
    pub commands: u32,
    /// Downloads in flight across the browser.
    pub downloads: u32,
    pub message: u32,
    pub command: u32,
    pub matches: u32,
    pub text: u32,
    pub snapshot: u32,
    pub screenshot: u32,
    /// A look's list: a tree's entries, a style's faces.
    pub entries: u32,
    /// The properties a style names.
    pub properties: u32,
    pub stderr: u32,
    pub poll: Duration,
    pub answer: Duration,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            persons: 4,
            pages: 16,
            ops: 64,
            commands: 128,
            downloads: 4,
            message: 4_194_304,
            command: 65_536,
            matches: 128,
            text: 4096,
            snapshot: 262_144,
            screenshot: 2_097_152,
            entries: 4096,
            properties: 32,
            stderr: 32_768,
            poll: Duration::from_millis(50),
            answer: Duration::from_secs(10),
        }
    }
}

/// The largest scan requested from the replies pipe, including NUL.
#[must_use]
pub fn largest_read(limits: &Limits) -> Option<u32> {
    limits.message.checked_add(1)
}

/// The largest room requested on the commands pipe, including NUL.
#[must_use]
pub fn largest_room(limits: &Limits) -> Option<u32> {
    limits.command.checked_add(1)
}

/// Whether these limits are representable: every edge bound they size is finite
/// and every capacity they derive fits (programming-model.md, 6.3).
/// The wire limits are positive, framing includes its NUL byte, and deadlines
/// advance time.
#[must_use]
pub fn valid(limits: &Limits) -> bool {
    limits.commands > 0
        && limits.command > 0
        && limits.message > 0
        && largest_read(limits).is_some()
        && largest_room(limits).is_some()
        && limits.answer.as_nanos() > 0
        && limits.poll.as_nanos() > 0
}
