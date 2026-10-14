//! Injected monotonic time. The core never reads a clock; the app passes one in.

/// Milliseconds since boot from the app's monotonic timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Millis(pub u64);
