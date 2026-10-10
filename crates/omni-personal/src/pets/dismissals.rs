//! Dismissed pet health findings.
//!
//! A `pet-health-dismissal` row per `(petId, kind)` hides that rule's current
//! episode from the attention views (the `/pets` warning and Home's count). It
//! never touches the `pet-health-alert` throttle or Pushover. The dismissal
//! lapses when the episode ends (PetTracker deletes it on the rule's clear
//! signal) and whenever the alert row records a newer episode start or push,
//! so a repeat push for a worsening episode raises the finding again. The
//! clear-signal path works without Pushover, when no alert rows are written.

use omni_api::pets::PetHealthKind;
use omni_store::cbor::Extra;
use omni_store::entity::Entity;
use serde::{Deserialize, Serialize};

use super::health::RuleState;

/// `pet-health-dismissal`, keyed `(petId, kind)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetHealthDismissal {
    pub pet_id: String,
    pub kind: PetHealthKind,
    /// Epoch ms.
    pub dismissed_at: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Entity for PetHealthDismissal {
    const NAME: &'static str = "pet-health-dismissal";
    type Key = (String, String);
    fn key(&self) -> (String, String) {
        (self.pet_id.clone(), self.kind.as_str().to_owned())
    }
}

/// Rules a dismissal can hide: the household data gap clears itself when
/// readings resume and always stays visible.
pub fn dismissable(kind: PetHealthKind) -> bool {
    kind != PetHealthKind::DataGap
}

/// A dismissal made at `dismissed_at` still applies unless the rule's alert
/// row started a new episode or sent a push after it. Without a row (Pushover
/// unconfigured, or nothing tripped yet) it applies until the episode clears.
pub fn still_dismissed(dismissed_at: i64, alert: Option<&RuleState>) -> bool {
    alert.is_none_or(|state| {
        state.episode_started_at.is_none_or(|at| at <= dismissed_at)
            && state.last_notified_at.is_none_or(|at| at <= dismissed_at)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: i64 = 1_791_000_000_000;

    fn state(episode_started_at: i64, last_notified_at: Option<i64>) -> RuleState {
        RuleState {
            active: true,
            notified: last_notified_at.is_some(),
            episode_started_at: Some(episode_started_at),
            last_notified_at,
            last_value: Some(6.0),
            last_message: None,
            recovered_at: None,
        }
    }

    #[test]
    fn a_dismissal_holds_until_a_newer_episode_or_push() {
        // No alert row: only the episode's end lifts it.
        assert!(still_dismissed(AT, None));
        // The dismissed episode, pushed before the dismissal.
        assert!(still_dismissed(AT, Some(&state(AT - 10, Some(AT - 5)))));
        // Dismissed before the first push of the episode (inside the weekly cap).
        assert!(still_dismissed(AT, Some(&state(AT - 10, None))));
        // A repeat push for the worsening episode re-raises it.
        assert!(!still_dismissed(AT, Some(&state(AT - 10, Some(AT + 1)))));
        // So does a new episode, notified or not.
        assert!(!still_dismissed(AT, Some(&state(AT + 1, None))));
    }

    #[test]
    fn only_the_data_gap_cannot_be_dismissed() {
        for kind in PetHealthKind::ALL {
            assert_eq!(dismissable(kind), kind != PetHealthKind::DataGap);
        }
    }
}
