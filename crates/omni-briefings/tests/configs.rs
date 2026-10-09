//! Briefing config loading from `BRIEFINGS_PATH` Markdown files.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_briefings::load_briefing_configs;
use omni_testkit::capture_logs;

fn tz() -> jiff::tz::TimeZone {
    jiff::tz::TimeZone::get("America/Vancouver").unwrap()
}

fn write(dir: &tempfile::TempDir, name: &str, content: &str) {
    std::fs::write(dir.path().join(name), content).unwrap();
}

fn logged(capture: &omni_testkit::LogCapture, level: tracing::Level, needle: &str) -> bool {
    capture
        .events()
        .iter()
        .any(|e| e.level == level && e.message.contains(needle))
}

#[test]
fn returns_empty_when_briefings_path_is_unset() {
    let capture = capture_logs();
    let result = load_briefing_configs(None, &tz()).unwrap();
    assert!(result.is_empty());
    assert!(logged(&capture, tracing::Level::INFO, "No BRIEFINGS_PATH"));
}

#[test]
fn returns_empty_and_warns_when_folder_does_not_exist() {
    let capture = capture_logs();
    let result = load_briefing_configs(Some("/nonexistent/path"), &tz()).unwrap();
    assert!(result.is_empty());
    assert!(logged(
        &capture,
        tracing::Level::WARN,
        "Briefings folder not found"
    ));
}

#[test]
fn loads_a_valid_md_file() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir,
        "TestBriefing.md",
        "---\nschedule: \"0 0 8 * * *\"\n---\nYou are a test assistant.",
    );
    let result = load_briefing_configs(dir.path().to_str(), &tz()).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "TestBriefing");
    assert_eq!(result[0].schedule.as_str(), "0 0 8 * * *");
    assert_eq!(result[0].prompt, "You are a test assistant.");
}

#[test]
fn skips_files_with_missing_schedule() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir,
        "NoSchedule.md",
        "---\ntitle: \"oops\"\n---\nSome prompt.",
    );
    let capture = capture_logs();
    let result = load_briefing_configs(dir.path().to_str(), &tz()).unwrap();
    assert!(result.is_empty());
    assert!(logged(
        &capture,
        tracing::Level::WARN,
        "Skipping NoSchedule.md"
    ));
}

#[test]
fn skips_files_with_invalid_cron_expression() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir,
        "BadCron.md",
        "---\nschedule: \"not a cron\"\n---\nSome prompt.",
    );
    let capture = capture_logs();
    let result = load_briefing_configs(dir.path().to_str(), &tz()).unwrap();
    assert!(result.is_empty());
    assert!(logged(
        &capture,
        tracing::Level::WARN,
        "invalid cron expression"
    ));
}

#[test]
fn skips_files_with_empty_body() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir,
        "EmptyBody.md",
        "---\nschedule: \"0 0 8 * * *\"\n---\n",
    );
    let capture = capture_logs();
    let result = load_briefing_configs(dir.path().to_str(), &tz()).unwrap();
    assert!(result.is_empty());
    assert!(logged(&capture, tracing::Level::WARN, "empty body"));
}

#[test]
fn ignores_non_md_files() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir,
        "readme.txt",
        "---\nschedule: \"0 0 8 * * *\"\n---\nPrompt.",
    );
    write(
        &dir,
        "Valid.md",
        "---\nschedule: \"0 0 8 * * *\"\n---\nActual prompt.",
    );
    let result = load_briefing_configs(dir.path().to_str(), &tz()).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "Valid");
}

#[test]
fn loads_multiple_valid_files() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir,
        "Alpha.md",
        "---\nschedule: \"0 0 8 * * *\"\n---\nPrompt A.",
    );
    write(
        &dir,
        "Beta.md",
        "---\nschedule: \"0 0 12 * * *\"\n---\nPrompt B.",
    );
    let result = load_briefing_configs(dir.path().to_str(), &tz()).unwrap();
    assert_eq!(result.len(), 2);
    let names: Vec<&str> = result.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"Alpha"));
    assert!(names.contains(&"Beta"));
}
