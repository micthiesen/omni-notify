//! Port of `src/live-check/titleDebounce.spec.ts`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_live::title_debounce::{DebounceAction, TITLE_CHANGE_COOLDOWN_MS, TitleChangeDebouncer};

fn notify(title: &str) -> DebounceAction {
    DebounceAction::Notify {
        title: title.into(),
    }
}

const NONE: DebounceAction = DebounceAction::None;

#[test]
fn notifies_immediately_on_the_first_observed_change() {
    let mut d = TitleChangeDebouncer::default();
    assert_eq!(d.observe("s1", "B", true, 1000), notify("B"));
}

#[test]
fn does_nothing_on_an_unrelated_tick() {
    let mut d = TitleChangeDebouncer::default();
    assert_eq!(d.observe("s1", "A", false, 1000), NONE);
}

#[test]
fn holds_a_title_change_immediately_after_seeding_on_go_live() {
    let mut d = TitleChangeDebouncer::default();
    d.seed("s1", "Live title", 0);
    assert_eq!(d.observe("s1", "Fixed title", true, 5000), NONE);
}

#[test]
fn notifies_immediately_once_the_cooldown_from_seeding_has_fully_elapsed() {
    let mut d = TitleChangeDebouncer::default();
    d.seed("s1", "A", 0);
    assert_eq!(
        d.observe("s1", "B", true, TITLE_CHANGE_COOLDOWN_MS),
        notify("B")
    );
}

#[test]
fn holds_multiple_changes_within_the_cooldown_and_fires_the_last_one_on_expiry() {
    let mut d = TitleChangeDebouncer::default();
    d.seed("s1", "T0", 0);
    assert_eq!(d.observe("s1", "T1", true, 1000), NONE);
    assert_eq!(d.observe("s1", "T2", true, 2000), NONE);
    assert_eq!(
        d.observe("s1", "T2", false, TITLE_CHANGE_COOLDOWN_MS + 1),
        notify("T2")
    );
}

#[test]
fn keeps_holding_on_ticks_before_the_cooldown_expires() {
    let mut d = TitleChangeDebouncer::default();
    d.seed("s1", "T0", 0);
    d.observe("s1", "T1", true, 1000);
    assert_eq!(
        d.observe("s1", "T1", false, TITLE_CHANGE_COOLDOWN_MS - 1),
        NONE
    );
}

#[test]
fn holds_a_change_made_right_after_a_trailing_fire_instead_of_notifying_immediately() {
    let mut d = TitleChangeDebouncer::default();
    d.seed("s1", "T0", 0);
    d.observe("s1", "T1", true, 1000);
    assert_eq!(
        d.observe("s1", "T1", false, TITLE_CHANGE_COOLDOWN_MS + 1),
        notify("T1")
    );
    assert_eq!(
        d.observe("s1", "T2", true, TITLE_CHANGE_COOLDOWN_MS + 2),
        NONE
    );
    assert_eq!(
        d.observe("s1", "T2", false, 2 * TITLE_CHANGE_COOLDOWN_MS + 3),
        notify("T2")
    );
}

#[test]
fn does_not_re_notify_when_the_held_title_matches_what_was_last_notified() {
    let mut d = TitleChangeDebouncer::default();
    d.seed("s1", "A", 0);
    d.observe("s1", "B", true, 1000);
    d.observe("s1", "A", true, 2000);
    assert_eq!(
        d.observe("s1", "A", false, TITLE_CHANGE_COOLDOWN_MS + 1),
        NONE
    );
}

#[test]
fn clears_the_pending_title_even_when_the_round_trip_suppresses_the_notify() {
    let mut d = TitleChangeDebouncer::default();
    d.seed("s1", "A", 0);
    d.observe("s1", "B", true, 1000);
    d.observe("s1", "A", true, 2000);
    d.observe("s1", "A", false, TITLE_CHANGE_COOLDOWN_MS + 1);
    assert_eq!(
        d.observe("s1", "A", false, 2 * TITLE_CHANGE_COOLDOWN_MS),
        NONE
    );
}

#[test]
fn drops_a_pending_held_title_so_it_cant_fire_later() {
    let mut d = TitleChangeDebouncer::default();
    d.seed("s1", "A", 0);
    d.observe("s1", "B", true, 1000);
    d.clear("s1");
    assert_eq!(
        d.observe("s1", "B", false, TITLE_CHANGE_COOLDOWN_MS + 1),
        NONE
    );
}

#[test]
fn resets_to_cold_start_behavior_the_next_change_notifies_immediately() {
    let mut d = TitleChangeDebouncer::default();
    d.seed("s1", "A", 0);
    d.clear("s1");
    assert_eq!(d.observe("s1", "B", true, 1000), notify("B"));
}

#[test]
fn tracks_state_independently_per_streamer() {
    let mut d = TitleChangeDebouncer::default();
    d.seed("s1", "A", 0);
    d.seed("s2", "X", 0);
    assert_eq!(d.observe("s1", "B", true, 1000), NONE);
    assert_eq!(
        d.observe("s2", "Y", true, TITLE_CHANGE_COOLDOWN_MS),
        notify("Y")
    );
}
