//! Voice-sampling target selection (`voiceTargets.ts`).

use std::cmp::Ordering;

use omni_core::js::locale_compare;

use crate::observation::Streamer;

/// Anything that carries a streamer.
pub trait HasStreamer {
    fn streamer(&self) -> &Streamer;
}

impl HasStreamer for Streamer {
    fn streamer(&self) -> &Streamer {
        self
    }
}

impl HasStreamer for crate::observation::LiveObservation {
    fn streamer(&self) -> &Streamer {
        &self.streamer
    }
}

fn dgg_viewers(streamer: &Streamer) -> f64 {
    streamer.dgg.and_then(|d| d.viewers).unwrap_or(0.0)
}

fn dgg_hosted(streamer: &Streamer) -> bool {
    streamer.dgg.is_some_and(|d| d.hosted)
}

/// The DGG streams most worth sampling after the whole tick finished: most DGG
/// viewers first, then hosted, then id. Selection happens after every platform
/// poll so fast DGG results cannot occupy the slots by completion order.
pub fn select_voice_targets<T: HasStreamer>(mut observations: Vec<T>, limit: usize) -> Vec<T> {
    observations.sort_by(|left, right| {
        let (l, r) = (left.streamer(), right.streamer());
        dgg_viewers(r)
            .partial_cmp(&dgg_viewers(l))
            .unwrap_or(Ordering::Equal)
            .then_with(|| dgg_hosted(r).cmp(&dgg_hosted(l)))
            .then_with(|| locale_compare(&l.id, &r.id))
    });
    observations.truncate(limit);
    observations
}
