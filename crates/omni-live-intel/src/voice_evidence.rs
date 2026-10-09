//! Repeated-sample voice evidence. In-memory, reset on restart.

use std::collections::HashMap;

const VOICE_HIT_WINDOW_MS: i64 = 10 * 60_000;
const SPEECH_MISSES_TO_RESET: u32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceEvidenceDecision {
    None,
    Possible,
    Confirmed,
}

impl VoiceEvidenceDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Possible => "possible",
            Self::Confirmed => "confirmed",
        }
    }
}

#[derive(Debug, Default)]
pub struct VoiceEvidenceTracker {
    hits: HashMap<String, Vec<i64>>,
    misses: HashMap<String, u32>,
}

impl VoiceEvidenceTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Matches in two independent samples within ten minutes confirm; silence
    /// (`checked_windows == 0`) is not counter-evidence, and five speech
    /// samples without a match forget earlier hits.
    pub fn observe(
        &mut self,
        streamer_id: &str,
        matched_windows: u32,
        checked_windows: u32,
        now: i64,
    ) -> VoiceEvidenceDecision {
        if checked_windows == 0 {
            return VoiceEvidenceDecision::None;
        }
        if matched_windows < 1 {
            let misses = self.misses.get(streamer_id).copied().unwrap_or(0) + 1;
            self.misses.insert(streamer_id.to_owned(), misses);
            if misses >= SPEECH_MISSES_TO_RESET {
                self.hits.remove(streamer_id);
            }
            return VoiceEvidenceDecision::None;
        }
        self.misses.insert(streamer_id.to_owned(), 0);
        let mut hits = self.hits.remove(streamer_id).unwrap_or_default();
        hits.push(now);
        hits.retain(|at| *at >= now - VOICE_HIT_WINDOW_MS);
        let confirmed = hits.len() >= 2;
        self.hits.insert(streamer_id.to_owned(), hits);
        if confirmed {
            VoiceEvidenceDecision::Confirmed
        } else {
            VoiceEvidenceDecision::Possible
        }
    }

    pub fn clear(&mut self, streamer_id: &str) {
        self.hits.remove(streamer_id);
        self.misses.remove(streamer_id);
    }
}
