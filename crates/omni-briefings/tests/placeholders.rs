//! Prompt placeholder expansion. "Now" is the wall time `2026-02-06T14:30:00` in
//! America/Vancouver, the production `TZ`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_briefings::format::local_ms;
use omni_briefings::placeholders::{
    resolve_all_placeholders, resolve_date_placeholder, resolve_time_placeholder,
};
use omni_testkit::{TEST_EPOCH_MS, TestStore, test_clock};

fn tz() -> jiff::tz::TimeZone {
    jiff::tz::TimeZone::get("America/Vancouver").unwrap()
}

fn now() -> i64 {
    local_ms(&tz(), jiff::civil::date(2026, 2, 6).at(14, 30, 0, 0)).unwrap()
}

#[test]
fn replaces_date_with_formatted_date() {
    assert_eq!(
        resolve_date_placeholder("Today is {{date}}.", now(), &tz()),
        "Today is Friday, February 6, 2026."
    );
}

#[test]
fn replaces_multiple_date_placeholders() {
    let result = resolve_date_placeholder("{{date}} and {{date}}", now(), &tz());
    assert!(result.contains("February 6, 2026"));
    assert!(!result.contains("{{date}}"));
}

#[test]
fn leaves_prompts_without_date_unchanged() {
    let prompt = "No placeholders here.";
    assert_eq!(resolve_date_placeholder(prompt, now(), &tz()), prompt);
}

#[test]
fn replaces_time_with_formatted_time() {
    let result = resolve_time_placeholder("It is {{time}}.", now(), &tz());
    assert!(!result.starts_with(char::is_whitespace));
    assert!(!result.contains("{{time}}"));
    assert_eq!(result, "It is 2:30 PM PST.");
}

#[test]
fn replaces_multiple_time_placeholders() {
    let result = resolve_time_placeholder("{{time}} and {{time}}", now(), &tz());
    assert!(!result.contains("{{time}}"));
}

#[test]
fn leaves_prompts_without_time_unchanged() {
    let prompt = "No placeholders here.";
    assert_eq!(resolve_time_placeholder(prompt, now(), &tz()), prompt);
}

#[tokio::test]
async fn resolves_date_time_and_history_together() {
    let s = TestStore::new(test_clock(TEST_EPOCH_MS)).await;
    let prompt = "Date: {{date}}\nTime: {{time}}\nHistory:\n{{history:5}}";
    let result = resolve_all_placeholders(&s.store, prompt, "TestBriefing", now(), &tz())
        .await
        .unwrap();
    assert!(!result.contains("{{date}}"));
    assert!(!result.contains("{{time}}"));
    assert!(!result.contains("{{history"));
    assert!(result.contains("February 6, 2026"));
    assert!(result.contains("No previous notifications"));
}

#[tokio::test]
async fn works_with_no_placeholders() {
    let s = TestStore::new(test_clock(TEST_EPOCH_MS)).await;
    let prompt = "Just a plain prompt.";
    assert_eq!(
        resolve_all_placeholders(&s.store, prompt, "Test", now(), &tz())
            .await
            .unwrap(),
        prompt
    );
}
