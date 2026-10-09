//! Port of `src/alerts/throttle.spec.ts`. The `throttleLogHook` case is
//! `chains_the_hook_and_drops_immediate_repeats` in `alert_layer.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_alerts::throttle::{
    AdmittedAlert, AlertThrottle, AlertThrottleOptions, ThrottledAlert, alert_key,
};

const MINUTE: i64 = 60_000;

fn alert(key: &str) -> ThrottledAlert {
    ThrottledAlert {
        key: key.to_owned(),
        title: "Boom".to_owned(),
        body: "details".to_owned(),
    }
}

fn delivered() -> Option<AdmittedAlert> {
    Some(AdmittedAlert {
        title: "Boom".to_owned(),
        body: "details".to_owned(),
    })
}

#[test]
fn alert_key_normalizes_case_and_whitespace() {
    assert_eq!(
        alert_key("A", "  Boom   happened "),
        alert_key("A", "Boom happened")
    );
    assert_eq!(
        alert_key("A", "Boom\u{feff}happened"),
        alert_key("A", "boom happened")
    );
}

#[test]
fn alert_key_keeps_messages_that_differ_by_an_embedded_identifier_apart() {
    assert_ne!(
        alert_key("A", "Failed to submit \"1Z999AA10123456784\""),
        alert_key("A", "Failed to submit \"1Z999AA10123456785\"")
    );
}

#[test]
fn alert_key_scopes_keys_by_logger_name() {
    assert_ne!(alert_key("A", "boom"), alert_key("B", "boom"));
}

#[test]
fn delivers_the_first_occurrence_immediately() {
    let mut throttle = AlertThrottle::default();
    assert_eq!(throttle.admit(alert("k"), 0), delivered());
}

#[test]
fn suppresses_repeats_inside_the_cooldown() {
    let mut throttle = AlertThrottle::default();
    throttle.admit(alert("k"), 0);
    assert_eq!(throttle.admit(alert("k"), MINUTE), None);
    assert_eq!(throttle.admit(alert("k"), 5 * MINUTE), None);
}

#[test]
fn re_delivers_after_the_cooldown_with_the_repeat_count() {
    let mut throttle = AlertThrottle::default();
    throttle.admit(alert("k"), 0);
    throttle.admit(alert("k"), MINUTE);
    throttle.admit(alert("k"), 2 * MINUTE);
    let admitted = throttle.admit(alert("k"), 16 * MINUTE).unwrap();
    assert_eq!(
        admitted.body,
        "details\n\nRepeated 3 times in the last 16m."
    );
}

#[test]
fn leaves_the_body_alone_when_nothing_was_suppressed_in_between() {
    let mut throttle = AlertThrottle::default();
    throttle.admit(alert("k"), 0);
    assert_eq!(throttle.admit(alert("k"), 20 * MINUTE), delivered());
}

#[test]
fn backs_the_cooldown_off_as_an_alert_keeps_repeating() {
    let mut throttle = AlertThrottle::default();
    throttle.admit(alert("k"), 0);
    assert!(throttle.admit(alert("k"), 16 * MINUTE).is_some());
    // Second cooldown is 30min, so 16min later is still suppressed.
    assert!(throttle.admit(alert("k"), 32 * MINUTE).is_none());
    assert!(throttle.admit(alert("k"), 47 * MINUTE).is_some());
}

#[test]
fn treats_an_alert_as_fresh_again_after_a_long_silence() {
    let mut throttle = AlertThrottle::default();
    throttle.admit(alert("k"), 0);
    assert_eq!(throttle.admit(alert("k"), 7 * 60 * MINUTE), delivered());
}

#[test]
fn tracks_distinct_keys_independently() {
    let mut throttle = AlertThrottle::default();
    throttle.admit(alert("k"), 0);
    assert!(throttle.admit(alert("other"), MINUTE).is_some());
}

fn two_keys() -> AlertThrottle {
    AlertThrottle::new(AlertThrottleOptions {
        max_keys: 2,
        ..AlertThrottleOptions::default()
    })
}

#[test]
fn evicts_the_least_recently_seen_keys_past_the_ceiling() {
    let mut throttle = two_keys();
    throttle.admit(alert("a"), 0);
    throttle.admit(alert("b"), 1);
    throttle.admit(alert("c"), 2);
    // "a" was evicted, so its next occurrence reads as new rather than suppressed.
    assert!(throttle.admit(alert("a"), 3).is_some());
    assert!(throttle.admit(alert("c"), 4).is_none());
}

#[test]
fn keeps_an_actively_repeating_key_over_a_dormant_one_when_evicting() {
    let mut throttle = two_keys();
    throttle.admit(alert("hot"), 0);
    throttle.admit(alert("dormant"), 1);
    throttle.admit(alert("hot"), 2);
    throttle.admit(alert("new"), 3);
    // "dormant" was the eviction victim, so "hot" is still throttled.
    assert!(throttle.admit(alert("hot"), 4).is_none());
    assert!(throttle.admit(alert("dormant"), 5).is_some());
}
