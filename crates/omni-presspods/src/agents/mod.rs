//! Model agents: metadata extraction and narration cleaning.

use std::sync::Arc;

use jiff::tz::TimeZone;
use omni_ai::Ai;
use omni_config::Config;

pub mod cleaner;
pub mod metadata;
pub mod parsing;

/// The language-model calls PressPods makes.
#[derive(Clone)]
pub struct Agents {
    pub(crate) ai: Ai,
    pub(crate) config: Arc<Config>,
    pub(crate) tz: TimeZone,
}

impl Agents {
    pub fn new(ai: Ai, config: Arc<Config>, tz: TimeZone) -> Self {
        Self { ai, config, tz }
    }
}
