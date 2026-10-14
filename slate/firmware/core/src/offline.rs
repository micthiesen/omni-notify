//! Backoff when the Thread network or the server is unreachable.
//!
//! The last frame stays on the panel; the server renders an "updated at" time into every
//! frame so a stale image is visible without the device drawing anything.

/// First retry delay after a failed check.
pub const FIRST_RETRY_S: u32 = 60;
/// Backoff multiplier per consecutive failure.
pub const BACKOFF_FACTOR: u32 = 2;
/// Longest retry delay while still in light sleep with Thread state retained.
pub const MAX_LIGHT_SLEEP_RETRY_S: u32 = 30 * 60;
/// After this long offline, drop Thread and deep sleep between reattach attempts.
pub const DEEP_SLEEP_AFTER_S: u32 = 6 * 60 * 60;
/// Reattach attempt interval while in the deep-sleep offline mode.
pub const DEEP_SLEEP_RETRY_S: u32 = 60 * 60;
