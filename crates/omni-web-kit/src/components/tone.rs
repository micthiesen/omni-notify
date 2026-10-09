//! Tones and status kinds: the only way components pick a semantic color.

/// A semantic hue modifier (`tag warn`, `meter signal`, …).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tone {
    #[default]
    Neutral,
    Live,
    Ok,
    Warn,
    Fault,
    Info,
    Signal,
}

impl Tone {
    /// The CSS modifier class; empty for neutral.
    pub fn class(self) -> &'static str {
        match self {
            Tone::Neutral => "",
            Tone::Live => "live",
            Tone::Ok => "ok",
            Tone::Warn => "warn",
            Tone::Fault => "fault",
            Tone::Info => "info",
            Tone::Signal => "signal",
        }
    }
}

/// What a [`Status`](super::Status) shows: each kind has a shape and a word.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum StatusKind {
    /// Quiet dot, no word needed.
    #[default]
    Ok,
    Running,
    Warn,
    Fault,
    Idle,
    Stale,
    Info,
    Live,
}

impl StatusKind {
    pub fn class(self) -> &'static str {
        match self {
            StatusKind::Ok => "ok",
            StatusKind::Running => "running",
            StatusKind::Warn => "warn",
            StatusKind::Fault => "fault",
            StatusKind::Idle => "idle",
            StatusKind::Stale => "stale",
            StatusKind::Info => "info",
            StatusKind::Live => "live",
        }
    }

    /// Default word for screen readers and labels.
    pub fn word(self) -> &'static str {
        match self {
            StatusKind::Ok => "Healthy",
            StatusKind::Running => "Running",
            StatusKind::Warn => "Warning",
            StatusKind::Fault => "Failed",
            StatusKind::Idle => "Idle",
            StatusKind::Stale => "Stale",
            StatusKind::Info => "Queued",
            StatusKind::Live => "Live",
        }
    }

    pub fn tone(self) -> Tone {
        match self {
            StatusKind::Ok => Tone::Ok,
            StatusKind::Running => Tone::Signal,
            StatusKind::Warn | StatusKind::Stale => Tone::Warn,
            StatusKind::Fault => Tone::Fault,
            StatusKind::Idle => Tone::Neutral,
            StatusKind::Info => Tone::Info,
            StatusKind::Live => Tone::Live,
        }
    }
}

/// Stable 0..6 hue bucket for monogram and poster fallbacks.
pub fn hue_index(seed: &str) -> u32 {
    let mut hash: u32 = 2_166_136_261;
    for byte in seed.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash % 6
}

/// `hue-N` class for [`hue_index`] (`""` for bucket 0).
pub fn hue_class(seed: &str) -> String {
    match hue_index(seed) {
        0 => String::new(),
        n => format!("hue-{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hue_is_stable_and_bounded() {
        assert_eq!(hue_index("hutch"), hue_index("hutch"));
        assert!((0..100).all(|i| hue_index(&i.to_string()) < 6));
        assert_eq!(Tone::Neutral.class(), "");
        assert_eq!(StatusKind::Stale.tone(), Tone::Warn);
    }
}
