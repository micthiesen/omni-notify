//! Prompt placeholders (`src/briefing-agent/placeholders.ts`): `{{history:N}}`,
//! `{{date}}` and `{{time}}`, resolved in that order.

use jiff::tz::TimeZone;
use omni_store::{Store, StoreError};

use crate::format::{format_long_date, format_time_with_zone};
use crate::persistence::resolve_history_placeholders;

/// Replaces every `{{date}}` with `Friday, February 6, 2026`.
pub fn resolve_date_placeholder(prompt: &str, now_ms: i64, tz: &TimeZone) -> String {
    if !prompt.contains("{{date}}") {
        return prompt.to_owned();
    }
    prompt.replace("{{date}}", &format_long_date(now_ms, tz))
}

/// Replaces every `{{time}}` with `2:30 PM PST`.
pub fn resolve_time_placeholder(prompt: &str, now_ms: i64, tz: &TimeZone) -> String {
    if !prompt.contains("{{time}}") {
        return prompt.to_owned();
    }
    prompt.replace("{{time}}", &format_time_with_zone(now_ms, tz))
}

/// History first, then date and time at `now_ms`.
pub async fn resolve_all_placeholders(
    store: &Store,
    prompt: &str,
    briefing_name: &str,
    now_ms: i64,
    tz: &TimeZone,
) -> Result<String, StoreError> {
    let resolved = resolve_history_placeholders(store, prompt, briefing_name, tz).await?;
    let resolved = resolve_date_placeholder(&resolved, now_ms, tz);
    Ok(resolve_time_placeholder(&resolved, now_ms, tz))
}
