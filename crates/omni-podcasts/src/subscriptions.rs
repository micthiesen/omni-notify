//! Subscribed shows, read exclusively from the podcast account.
//! A configured account whose read fails aborts the
//! run (three-state rule); no account means empty with a warning.

use std::collections::HashSet;

use crate::account::{FetchResult, PodcastAccount, PodcastSubscription, Unavailable};
use crate::titles::normalize_title;
use crate::types::{CanonicalShowId, make_show_id};

const LOG: &str = "PodcastRecsTask";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubscriptionSource {
    Account,
    None,
}

impl SubscriptionSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::None => "none",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubscriptionState {
    pub subscriptions: Vec<PodcastSubscription>,
    pub show_ids: HashSet<CanonicalShowId>,
    pub normalized_titles: HashSet<String>,
    pub source: SubscriptionSource,
}

pub async fn resolve_subscriptions(
    account: Option<&dyn PodcastAccount>,
) -> FetchResult<SubscriptionState> {
    let Some(account) = account else {
        tracing::warn!(
            target: LOG,
            "No podcast account configured; subscribed-show exclusion falls back to the taste profile only"
        );
        return Ok(build_state(Vec::new(), SubscriptionSource::None));
    };
    match account.fetch_subscriptions().await {
        Ok(subscriptions) => Ok(build_state(subscriptions, SubscriptionSource::Account)),
        Err(e) => Err(Unavailable::new(format!(
            "{} subscriptions unavailable: {}",
            account.name(),
            e.reason
        ))),
    }
}

pub fn build_state(
    subscriptions: Vec<PodcastSubscription>,
    source: SubscriptionSource,
) -> SubscriptionState {
    let mut show_ids = HashSet::new();
    let mut normalized_titles = HashSet::new();
    for sub in &subscriptions {
        if let Some(id) = make_show_id(sub.itunes_id, sub.feed_url.as_deref()) {
            show_ids.insert(id);
        }
        normalized_titles.insert(normalize_title(&sub.title));
    }
    SubscriptionState {
        subscriptions,
        show_ids,
        normalized_titles,
        source,
    }
}

/// Compact digest of subscribed shows for model prompts.
pub fn format_subscriptions_digest(state: &SubscriptionState) -> String {
    if state.subscriptions.is_empty() {
        return "Subscribed shows: unknown (no podcast account configured).".to_owned();
    }
    let mut titles: Vec<&str> = state
        .subscriptions
        .iter()
        .map(|s| s.title.as_str())
        .collect();
    // JS default sort compares UTF-16 code units.
    titles.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    let list: Vec<String> = titles.iter().map(|t| format!("- {t}")).collect();
    format!(
        "Shows the user already subscribes to (source: {}) — never recommend these, but they are strong taste evidence:\n{}",
        state.source.as_str(),
        list.join("\n")
    )
}
