//! What to do with a voice-evidence decision.

use crate::types::{DestinyPresence, PresenceState};
use crate::voice_evidence::VoiceEvidenceDecision;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceMatchAction {
    Ignore,
    RecordPossible,
    RetainConfirmed,
    Verify,
}

/// A current confirmation is never downgraded and never paid for twice.
pub fn decide_voice_match_action(
    evidence: VoiceEvidenceDecision,
    presence: Option<&DestinyPresence>,
) -> VoiceMatchAction {
    if evidence == VoiceEvidenceDecision::None {
        return VoiceMatchAction::Ignore;
    }
    if presence.is_some_and(|p| p.state == PresenceState::Confirmed) {
        return VoiceMatchAction::RetainConfirmed;
    }
    if evidence == VoiceEvidenceDecision::Possible {
        VoiceMatchAction::RecordPossible
    } else {
        VoiceMatchAction::Verify
    }
}
