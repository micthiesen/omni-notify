//! Display formatting (`frontend/src/utils/format.ts`).

use super::js::{
    date_locale_date_string, date_locale_string, js_round, local_date_ms, now_ms, number_string,
    to_fixed,
};

pub fn pad2(n: i64) -> String {
    format!("{n:02}")
}

/// Cents as "0.12¢" (sub-cent), "$0.0034" or "$1.23"; `None` for missing or
/// non-finite values so callers can render conditionally.
pub fn format_cents(cents: Option<f64>) -> Option<String> {
    let cents = cents.filter(|c| c.is_finite())?;
    if cents < 1.0 {
        return Some(format!("{}¢", to_fixed(cents, 2)));
    }
    let dollars = cents / 100.0;
    Some(if dollars < 1.0 {
        format!("${}", to_fixed(dollars, 4))
    } else {
        format!("${}", to_fixed(dollars, 2))
    })
}

pub fn format_clock_time(hour: i64, minute: i64) -> String {
    let ampm = if hour >= 12 { "PM" } else { "AM" };
    let h12 = if hour % 12 == 0 { 12 } else { hour % 12 };
    format!("{h12}:{} {ampm}", pad2(minute))
}

/// "in 5m" / "3h 2m ago" relative to `now`.
pub fn format_relative_at(epoch_ms: f64, now: f64) -> String {
    let diff = epoch_ms - now;
    let future = diff >= 0.0;
    let abs_ms = diff.abs();
    if abs_ms < 5_000.0 {
        return if future { "in a moment" } else { "just now" }.to_owned();
    }
    let total_sec = js_round(abs_ms / 1000.0);
    let text = if total_sec < 60.0 {
        format!("{}s", number_string(total_sec))
    } else if total_sec < 3600.0 {
        format!("{}m", number_string(js_round(total_sec / 60.0)))
    } else if total_sec < 86_400.0 {
        let hours = (total_sec / 3600.0).floor();
        let mins = js_round((total_sec % 3600.0) / 60.0);
        if mins > 0.0 {
            format!("{}h {}m", number_string(hours), number_string(mins))
        } else {
            format!("{}h", number_string(hours))
        }
    } else {
        format!("{}d", number_string(js_round(total_sec / 86_400.0)))
    };
    if future {
        format!("in {text}")
    } else {
        format!("{text} ago")
    }
}

/// [`format_relative_at`] against the current time.
pub fn format_relative(epoch_ms: f64) -> String {
    format_relative_at(epoch_ms, now_ms())
}

pub fn format_duration(ms: f64) -> String {
    if ms < 1000.0 {
        return format!("{}ms", number_string(js_round(ms).max(0.0)));
    }
    if ms < 60_000.0 {
        return format!("{}s", to_fixed(ms / 1000.0, 1));
    }
    let total_sec = js_round(ms / 1000.0) as i64;
    let total_min = total_sec / 60;
    if total_min < 60 {
        return format!("{total_min}m {}s", total_sec % 60);
    }
    let total_hours = total_min / 60;
    let mins = total_min % 60;
    if total_hours < 24 {
        return if mins > 0 {
            format!("{total_hours}h {mins}m")
        } else {
            format!("{total_hours}h")
        };
    }
    let days = total_hours / 24;
    let hours = total_hours % 24;
    if hours > 0 {
        format!("{days}d {hours}h")
    } else {
        format!("{days}d")
    }
}

/// Ticking countdown: "2h 05m", "3m 12s", "42s", "now".
pub fn format_countdown(ms: f64) -> String {
    if ms <= 0.0 {
        return "now".to_owned();
    }
    let total_sec = (ms / 1000.0).floor() as i64;
    let h = total_sec / 3600;
    let m = (total_sec % 3600) / 60;
    let s = total_sec % 60;
    if h > 0 {
        format!("{h}h {}m", pad2(m))
    } else if m > 0 {
        format!("{m}m {}s", pad2(s))
    } else {
        format!("{s}s")
    }
}

/// Coarse elapsed time for stream uptime: "1h 23m", "23m", "45s".
pub fn format_uptime(ms: f64) -> String {
    let total_sec = (ms / 1000.0).floor().max(0.0) as i64;
    let h = total_sec / 3600;
    let m = (total_sec % 3600) / 60;
    if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{total_sec}s")
    }
}

pub fn format_compact_number(n: f64) -> String {
    if n >= 1_000_000.0 {
        format!("{}M", to_fixed(n / 1_000_000.0, 1))
    } else if n >= 10_000.0 {
        format!("{}k", number_string(js_round(n / 1000.0)))
    } else if n >= 1_000.0 {
        format!("{}k", to_fixed(n / 1000.0, 1))
    } else {
        number_string(n)
    }
}

/// "Oct 9, 3:04 PM".
pub fn format_absolute(epoch_ms: f64) -> String {
    date_locale_string(
        epoch_ms,
        &[
            ("month", "short"),
            ("day", "numeric"),
            ("hour", "numeric"),
            ("minute", "2-digit"),
        ],
    )
}

/// "Oct 9, 2026, 3:04 PM".
pub fn format_absolute_with_year(epoch_ms: f64) -> String {
    date_locale_string(
        epoch_ms,
        &[
            ("month", "short"),
            ("day", "numeric"),
            ("year", "numeric"),
            ("hour", "numeric"),
            ("minute", "2-digit"),
        ],
    )
}

/// "Task runs" → "Task Runs", "CastroInboxCleanup" → "Castro Inbox Cleanup".
pub fn to_title_case(value: &str) -> String {
    // .replace(/[_-]+/g, " ")
    let mut step1 = String::with_capacity(value.len());
    let mut in_run = false;
    for ch in value.chars() {
        if ch == '_' || ch == '-' {
            if !in_run {
                step1.push(' ');
            }
            in_run = true;
        } else {
            in_run = false;
            step1.push(ch);
        }
    }
    // .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    let chars: Vec<char> = step1.chars().collect();
    let mut step2 = String::with_capacity(step1.len() + 8);
    for (index, ch) in chars.iter().enumerate() {
        if index > 0 {
            let prev = chars[index - 1];
            if (prev.is_ascii_lowercase() || prev.is_ascii_digit()) && ch.is_ascii_uppercase() {
                step2.push(' ');
            }
        }
        step2.push(*ch);
    }
    // .replace(/([A-Z]+)([A-Z][a-z])/g, "$1 $2")
    let chars: Vec<char> = step2.chars().collect();
    let mut step3 = String::with_capacity(step2.len() + 8);
    for (index, ch) in chars.iter().enumerate() {
        if index > 0
            && ch.is_ascii_uppercase()
            && chars[index - 1].is_ascii_uppercase()
            && chars
                .get(index + 1)
                .is_some_and(|next| next.is_ascii_lowercase())
        {
            step3.push(' ');
        }
        step3.push(*ch);
    }
    // .trim().replace(/\s+/g, " ").split(" ") + capitalize words starting with [a-z]
    step3
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) if first.is_ascii_lowercase() => {
                    format!("{}{}", first.to_ascii_uppercase(), chars.as_str())
                }
                _ => word.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A task's admin-set display name, falling back to a title-cased name.
pub fn task_label(name: &str, display_name: Option<&str>) -> String {
    match display_name.map(str::trim) {
        Some(label) if !label.is_empty() => label.to_owned(),
        _ => to_title_case(name),
    }
}

/// Label of `task_name` from a task list; title-cases unknown names.
pub fn task_label_from_name(task_name: &str, tasks: &[omni_api::tasks::TaskInfo]) -> String {
    match tasks.iter().find(|t| t.name == task_name) {
        Some(task) => task_label(&task.name, task.display_name.as_deref()),
        None => to_title_case(task_name),
    }
}

/// "Oct 9, 2026".
pub fn format_date_only(epoch_ms: f64) -> String {
    date_locale_date_string(
        epoch_ms,
        &[("month", "short"), ("day", "numeric"), ("year", "numeric")],
    )
}

/// Formats `YYYY-MM-DD` without letting a timezone shift the day.
pub fn format_calendar_date(date: &str, include_year: bool) -> String {
    let parts: Vec<f64> = date
        .split('-')
        .map(|part| part.trim().parse::<f64>().unwrap_or(f64::NAN))
        .collect();
    let valid = |v: Option<&f64>| v.is_some_and(|v| *v != 0.0 && !v.is_nan());
    if !(valid(parts.first()) && valid(parts.get(1)) && valid(parts.get(2))) {
        return date.to_owned();
    }
    let ms = local_date_ms(parts[0] as i32, parts[1] as i32, parts[2] as i32);
    if include_year {
        date_locale_date_string(
            ms,
            &[("month", "short"), ("day", "numeric"), ("year", "numeric")],
        )
    } else {
        date_locale_date_string(ms, &[("month", "short"), ("day", "numeric")])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_case_matches_ts() {
        assert_eq!(to_title_case("Task runs"), "Task Runs");
        assert_eq!(to_title_case("CastroInboxCleanup"), "Castro Inbox Cleanup");
        assert_eq!(to_title_case("HTMLParser"), "HTML Parser");
        assert_eq!(to_title_case("email-activity_log"), "Email Activity Log");
        assert_eq!(to_title_case("LiveCheckTask"), "Live Check Task");
        assert_eq!(task_label("PodcastRecs", Some("  ")), "Podcast Recs");
        assert_eq!(task_label("PodcastRecs", Some("Picks")), "Picks");
    }

    #[test]
    fn durations_and_relative() {
        assert_eq!(format_duration(999.0), "999ms");
        assert_eq!(format_duration(1500.0), "1.5s");
        assert_eq!(format_duration(61_000.0), "1m 1s");
        assert_eq!(format_duration(3_600_000.0), "1h");
        assert_eq!(format_duration(90_000_000.0), "1d 1h");
        assert_eq!(format_countdown(3_723_000.0), "1h 02m");
        assert_eq!(format_countdown(0.0), "now");
        assert_eq!(format_uptime(125_000.0), "2m");
        assert_eq!(format_relative_at(0.0, 1000.0), "just now");
        assert_eq!(format_relative_at(0.0, 90_000.0), "2m ago");
        assert_eq!(format_relative_at(3_900_000.0, 0.0), "in 1h 5m");
        assert_eq!(format_compact_number(12_345.0), "12k");
        assert_eq!(format_compact_number(1_234.0), "1.2k");
        assert_eq!(format_compact_number(2_500_000.0), "2.5M");
        assert_eq!(format_cents(Some(0.5)).as_deref(), Some("0.50¢"));
        assert_eq!(format_cents(Some(50.0)).as_deref(), Some("$0.5000"));
        assert_eq!(format_cents(Some(250.0)).as_deref(), Some("$2.50"));
        assert_eq!(format_cents(None), None);
        assert_eq!(format_clock_time(0, 5), "12:05 AM");
        assert_eq!(format_calendar_date("bad", true), "bad");
    }
}
