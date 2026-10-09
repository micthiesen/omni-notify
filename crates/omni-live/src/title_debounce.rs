//! Eager title-change debounce (`titleDebounce.ts`).
//!
//! The first change notifies immediately; later changes within the cooldown
//! are held (last one wins) and the held title fires once the cooldown lapses,
//! restarting it. State is in memory only: after a restart the next change
//! notifies immediately, an acceptable cold start.

use std::collections::HashMap;

/// Ten minutes.
pub const TITLE_CHANGE_COOLDOWN_MS: i64 = 10 * 60_000;

#[derive(Clone, Debug)]
struct DebounceState {
    last_notified_at: i64,
    last_notified_title: String,
    pending_title: Option<String>,
}

/// What to do on this observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DebounceAction {
    Notify { title: String },
    None,
}

/// Per-streamer debounce state.
#[derive(Clone, Debug, Default)]
pub struct TitleChangeDebouncer {
    states: HashMap<String, DebounceState>,
}

impl TitleChangeDebouncer {
    /// On went-live: the go-live notification already carried the title, so a
    /// quick post-live fix is held for the rest of the cooldown.
    pub fn seed(&mut self, streamer_id: &str, title: &str, now: i64) {
        self.states.insert(
            streamer_id.to_owned(),
            DebounceState {
                last_notified_at: now,
                last_notified_title: title.to_owned(),
                pending_title: None,
            },
        );
    }

    /// Called on every still-live tick so a held title can fire later.
    pub fn observe(
        &mut self,
        streamer_id: &str,
        current_title: &str,
        title_changed: bool,
        now: i64,
    ) -> DebounceAction {
        let Some(state) = self.states.get_mut(streamer_id) else {
            if !title_changed {
                return DebounceAction::None;
            }
            self.seed(streamer_id, current_title, now);
            return DebounceAction::Notify {
                title: current_title.to_owned(),
            };
        };

        let cooldown_elapsed = now - state.last_notified_at >= TITLE_CHANGE_COOLDOWN_MS;
        if title_changed {
            if cooldown_elapsed {
                state.last_notified_at = now;
                state.last_notified_title = current_title.to_owned();
                state.pending_title = None;
                return DebounceAction::Notify {
                    title: current_title.to_owned(),
                };
            }
            state.pending_title = Some(current_title.to_owned());
            return DebounceAction::None;
        }

        if cooldown_elapsed && let Some(pending) = state.pending_title.take() {
            // A -> B -> A round trip: nothing new to announce.
            if pending == state.last_notified_title {
                return DebounceAction::None;
            }
            state.last_notified_at = now;
            state.last_notified_title = pending.clone();
            return DebounceAction::Notify { title: pending };
        }
        DebounceAction::None
    }

    /// On went-offline and on a primary switch: a title held from the old
    /// primary must not fire under the new one.
    pub fn clear(&mut self, streamer_id: &str) {
        self.states.remove(streamer_id);
    }
}
