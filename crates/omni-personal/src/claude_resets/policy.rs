//! Claude Code alert selection (`src/claude-resets/policy.ts`). Event dates,
//! never the catalog update date or revisions, bound replay. Reset Radar has no
//! structured banked/regular distinction, so its wording is kept as a report.

use std::collections::HashSet;

use jiff::tz::TimeZone;
use omni_http::Url;

use super::source::ClaudeResetSource;
use crate::js::parse_date;
use crate::reset_alerts::ResetAlert;
use crate::reset_alerts::presentation::{
    ALERT_LOOKBACK_MS, CLOCK_SKEW_MS, compact_summary, pacific_time,
};

fn post_identity(value: &str) -> String {
    let Ok(url) = Url::parse(value) else {
        return value.to_owned();
    };
    let host = url.host_str().unwrap_or_default();
    if ["x.com", "twitter.com", "www.x.com", "www.twitter.com"].contains(&host)
        && let Some(id) = status_id(url.path())
    {
        return format!("x:{id}");
    }
    url.as_str().to_owned()
}

/// `/^\/(?:[^/]+\/status|i\/web\/status)\/(\d+)(?:\/|$)/`.
fn status_id(path: &str) -> Option<&str> {
    let rest = path.strip_prefix('/')?;
    let after = if let Some(rest) = rest.strip_prefix("i/web/status/") {
        Some(rest)
    } else {
        let (first, tail) = rest.split_once('/')?;
        (!first.is_empty())
            .then_some(tail)
            .and_then(|tail| tail.strip_prefix("status/"))
    };
    let after = after?;
    let digits = after
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after.len());
    if digits == 0 {
        return None;
    }
    let (id, remainder) = after.split_at(digits);
    (remainder.is_empty() || remainder.starts_with('/')).then_some(id)
}

/// Selects confirmed historical Claude Code counter resets from the last 48 hours.
pub fn select_claude_reset_alerts(
    source: &ClaudeResetSource,
    now: i64,
    tz: &TimeZone,
) -> Vec<ResetAlert> {
    let mut candidates: Vec<ResetAlert> = Vec::new();
    for event in &source.events {
        let Some(occurred_at) = parse_date(&event.date, tz) else {
            continue;
        };
        if event.kind != "counter-reset"
            || event.status != "historic"
            || event.confidence != "confirmed"
            || !event.surfaces.iter().any(|s| s == "claude-code")
            || now - occurred_at > ALERT_LOOKBACK_MS
            || occurred_at > now + CLOCK_SKEW_MS
        {
            continue;
        }
        let primary = event.sources.first().map(|s| s.url.as_str());
        let lines = [
            compact_summary(&event.title),
            compact_summary(&event.summary),
            "Check Settings → Usage. Banked resets refill usage only when redeemed.".to_owned(),
            "Your account has not been verified.".to_owned(),
            format!("Reported {}.", pacific_time(occurred_at)),
            "Source: Reset Radar".to_owned(),
        ];
        candidates.push(ResetAlert {
            key: format!("{}:reported", event.id),
            // Additional sources can be background references shared by unrelated events.
            aliases: primary
                .map(|url| vec![format!("post:{}:reported", post_identity(url))])
                .unwrap_or_default(),
            title: "Claude Code reset reported".to_owned(),
            message: lines
                .into_iter()
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("\n"),
            url: primary.map_or_else(
                || {
                    format!(
                        "https://resetradar.com/#{}",
                        omni_core::js::encode_uri_component(&event.id)
                    )
                },
                str::to_owned,
            ),
            occurred_at,
        });
    }
    candidates.sort_by_key(|alert| std::cmp::Reverse(alert.occurred_at));
    let mut seen: HashSet<String> = HashSet::new();
    let mut kept: Vec<ResetAlert> = Vec::new();
    for alert in candidates {
        let identities: Vec<String> = std::iter::once(alert.key.clone())
            .chain(alert.aliases.iter().cloned())
            .collect();
        let duplicate = identities.iter().any(|identity| seen.contains(identity));
        seen.extend(identities);
        if !duplicate {
            kept.push(alert);
        }
    }
    kept.reverse();
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x_status_identities() {
        assert_eq!(
            post_identity("https://x.com/ClaudeDevs/status/123"),
            "x:123"
        );
        assert_eq!(
            post_identity("https://www.twitter.com/newhandle/status/123/photo/1?s=20#content"),
            "x:123"
        );
        assert_eq!(post_identity("https://x.com/i/web/status/9"), "x:9");
        assert_eq!(
            post_identity("https://x.com/a/status/12x"),
            "https://x.com/a/status/12x"
        );
        assert_eq!(
            post_identity("https://example.com/?id=1#post"),
            "https://example.com/?id=1#post"
        );
    }
}
