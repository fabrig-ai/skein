//! The bridge between the page and a world in a worker (playground.md, 5).

/// A duty of the transport's, as data: what the page hands the world.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Duty {
    Exchange(crate::fetch::Exchange),
    Abandon {
        exchange: u32,
    },
    Stream(crate::transport::StreamDuty),
    /// The next piece of the stream, after the step that took the last.
    ReadOn,
}
