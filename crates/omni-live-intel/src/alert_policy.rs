//! Alert confidence floors and per-session dedup (`alertPolicy.ts`).

use crate::types::{LivestreamAlertType, LivestreamIntelligenceData};

/// The minimum confidence an alert of `alert_type` needs.
///
/// Destiny presence already passed dedicated speaker, repeated-evidence and
/// live-conversation gates, so the generic semantic floor is not reapplied to
/// it; repeated negative feedback raises only its dedicated floor.
pub fn livestream_alert_confidence_floor(
    alert_type: LivestreamAlertType,
    feedback_digest: &str,
    destiny_speaker_threshold: f64,
) -> f64 {
    let prefix = format!("{}:", alert_type.as_str());
    let relevant: Vec<&str> = feedback_digest
        .split('\n')
        .filter(|line| line.starts_with(&prefix))
        .collect();
    let is_destiny = alert_type == LivestreamAlertType::DestinyGuest;
    let base = if is_destiny {
        destiny_speaker_threshold
    } else {
        0.75
    };
    if relevant.len() < 2 {
        return base;
    }
    let negative = relevant
        .iter()
        .filter(|line| line.contains("not_useful") || line.contains("false_positive"))
        .count();
    #[allow(clippy::cast_precision_loss)]
    let ratio = negative as f64 / relevant.len() as f64;
    if ratio < 0.6 {
        return base;
    }
    if is_destiny { base.max(0.75) } else { 0.9 }
}

/// Whether an alert of `alert_type` was already delivered in the state's session.
pub fn alert_sent_in_session(
    state: &LivestreamIntelligenceData,
    alert_type: LivestreamAlertType,
) -> bool {
    let recorded_at = state
        .alerted_at_by_type
        .as_ref()
        .and_then(|map| map.get(alert_type.as_str()))
        .copied()
        .unwrap_or(0);
    if recorded_at >= state.session_started_at {
        return true;
    }
    state.latest_alert.as_ref().is_some_and(|alert| {
        alert.alert_type == alert_type && alert.created_at >= state.session_started_at
    })
}
