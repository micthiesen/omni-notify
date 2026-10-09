//! Reset Beacon alert feed and history (`src/codex-resets/source.ts`).

use jiff::tz::TimeZone;
use omni_http::public::PublicHttpClient;
use serde::Deserialize;
use serde_json::Value;

use crate::reset_alerts::source::{
    Check, ResetSourceError, decode, optional_non_null, read_json, required_nullable,
};

pub const ALERTS_URL: &str = "https://resetbeacon.com/api/alerts";
pub const HISTORY_URL: &str = "https://resetbeacon.com/api/history";
const MAX_FEED_ITEMS: usize = 1_000;
const MAX_HISTORY_ITEMS: usize = 5_000;

/// `AlertFeedSchema`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertFeed {
    pub generated_at: String,
    pub items: Vec<FeedItem>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedItem {
    pub id: String,
    pub event_id: String,
    #[serde(deserialize_with = "required_nullable")]
    pub post_id: Option<String>,
    pub topic: String,
    pub state: String,
    pub title: String,
    pub summary: String,
    pub source_url: String,
    #[serde(deserialize_with = "required_nullable")]
    pub evidence_id: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub target_at: Option<String>,
    pub published_at: String,
    /// Optional and nullable.
    #[serde(default)]
    pub source_published_at: Option<String>,
    pub withdrawn: bool,
}

/// `HistorySchema`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetHistory {
    pub items: Vec<HistoryEvent>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEvent {
    pub id: String,
    pub kind: String,
    pub scope: String,
    pub event_kind: String,
    pub evidence_class: String,
    pub status: String,
    #[serde(deserialize_with = "required_nullable")]
    pub fulfilled_by: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub superseded_by: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub announced_at: Option<String>,
    #[serde(default, deserialize_with = "optional_non_null")]
    pub summary: Option<String>,
    pub sources: Vec<HistorySource>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistorySource {
    pub announcement_id: String,
    pub url: String,
}

fn check_feed(feed: &AlertFeed, tz: &TimeZone) -> Result<(), String> {
    let c = Check { tz };
    c.timestamp("generatedAt", &feed.generated_at)?;
    c.max_items("items", feed.items.len(), MAX_FEED_ITEMS)?;
    for (i, item) in feed.items.iter().enumerate() {
        let p = |field: &str| format!("items[{i}].{field}");
        c.text(&p("id"), &item.id)?;
        c.text(&p("eventId"), &item.event_id)?;
        if let Some(post) = &item.post_id {
            c.text(&p("postId"), post)?;
        }
        c.text(&p("topic"), &item.topic)?;
        c.text(&p("state"), &item.state)?;
        c.text(&p("title"), &item.title)?;
        c.text(&p("summary"), &item.summary)?;
        c.url(&p("sourceUrl"), &item.source_url)?;
        if let Some(evidence) = &item.evidence_id {
            c.identifier(&p("evidenceId"), evidence)?;
        }
        if let Some(target) = &item.target_at {
            c.timestamp(&p("targetAt"), target)?;
        }
        c.timestamp(&p("publishedAt"), &item.published_at)?;
        if let Some(source) = &item.source_published_at {
            c.timestamp(&p("sourcePublishedAt"), source)?;
        }
    }
    Ok(())
}

fn check_history(history: &ResetHistory, tz: &TimeZone) -> Result<(), String> {
    let c = Check { tz };
    c.max_items("items", history.items.len(), MAX_HISTORY_ITEMS)?;
    for (i, event) in history.items.iter().enumerate() {
        let p = |field: &str| format!("items[{i}].{field}");
        c.text(&p("id"), &event.id)?;
        c.text(&p("kind"), &event.kind)?;
        c.identifier(&p("scope"), &event.scope)?;
        c.text(&p("eventKind"), &event.event_kind)?;
        c.text(&p("evidenceClass"), &event.evidence_class)?;
        c.text(&p("status"), &event.status)?;
        for value in [&event.fulfilled_by, &event.superseded_by, &event.summary]
            .into_iter()
            .flatten()
        {
            c.text(&p("text"), value)?;
        }
        if let Some(announced) = &event.announced_at {
            c.timestamp(&p("announcedAt"), announced)?;
        }
        for (j, source) in event.sources.iter().enumerate() {
            c.text(
                &p(&format!("sources[{j}].announcementId")),
                &source.announcement_id,
            )?;
            c.url(&p(&format!("sources[{j}].url")), &source.url)?;
        }
    }
    Ok(())
}

/// Decodes and validates an alert feed document.
pub fn decode_alert_feed(value: Value, tz: &TimeZone) -> Result<AlertFeed, String> {
    decode(value, |feed| check_feed(feed, tz))
}

/// Decodes and validates a history document.
pub fn decode_history(value: Value, tz: &TimeZone) -> Result<ResetHistory, String> {
    decode(value, |history| check_history(history, tz))
}

/// Reads the alert feed and history concurrently.
pub async fn read_reset_sources(
    http: &PublicHttpClient,
    tz: &TimeZone,
) -> Result<(AlertFeed, ResetHistory), ResetSourceError> {
    let feed = async {
        let raw = read_json(http, ALERTS_URL).await?;
        decode_alert_feed(raw, tz)
            .map_err(|e| ResetSourceError::new(format!("read {ALERTS_URL}"), e))
    };
    let history = async {
        let raw = read_json(http, HISTORY_URL).await?;
        decode_history(raw, tz).map_err(|e| ResetSourceError::new(format!("read {HISTORY_URL}"), e))
    };
    tokio::try_join!(feed, history)
}
