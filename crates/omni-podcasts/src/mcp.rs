//! Podcast MCP tools: bounded adapters over the
//! account client, recommendation rows and taste data. Metadata and schemas
//! come from the golden tool list.

use std::sync::Arc;

use omni_api::podcasts::{js_number, js_number_opt};
use omni_core::clock::SharedClock;
use omni_mcp_kit::{McpTool, Page, ToolError, ToolMetaError, paginate, typed_tool};
use omni_store::Store;
use serde::{Deserialize, Serialize};

use crate::account::{
    AccountProvider, EnqueueEpisodeRequest, FetchResult, PodcastAccount, PodcastQueuePosition,
    PodcastWriteResult, SubscribeToShowRequest,
};
use crate::persistence::{
    PodcastFeedback, PodcastQueueResult, PodcastRecommendationData, PodcastRecommendationStatus,
    get_all_podcast_recommendations, get_podcast_recommendation,
    set_podcast_recommendation_feedback,
};
use crate::reflection::store::{get_all_podcast_taste_evidence, get_latest_podcast_taste_profile};
use crate::reflection::types::{
    PodcastTasteClaim, PodcastTasteEvidenceKind, PodcastTasteProfileData,
};

const DAY_MS: i64 = 86_400_000;

/// What the tools read and write.
#[derive(Clone)]
pub struct McpState {
    pub store: Store,
    pub clock: SharedClock,
    pub accounts: Arc<dyn AccountProvider>,
}

fn require_account(state: &McpState) -> Result<Arc<dyn PodcastAccount>, ToolError> {
    state
        .accounts
        .resolve()
        .ok_or_else(|| ToolError::execute("Podcast account is not configured"))
}

fn require_available<T>(result: FetchResult<T>) -> Result<T, ToolError> {
    result.map_err(|e| ToolError::execute(e.reason))
}

fn store_error(e: omni_store::StoreError) -> ToolError {
    ToolError::execute_from(&e)
}

/// `matchesQuery`: case-insensitive substring on title/showTitle/episodeTitle.
fn matches_query(texts: &[Option<&str>], query: Option<&str>) -> bool {
    let Some(query) = query.filter(|q| !q.is_empty()) else {
        return true;
    };
    let needle = query.to_lowercase();
    texts
        .iter()
        .flatten()
        .any(|text| text.to_lowercase().contains(&needle))
}

fn default_cursor() -> usize {
    0
}

fn default_limit() -> usize {
    25
}

fn default_search_limit() -> usize {
    20
}

fn default_since_days() -> i64 {
    30
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AccountResource {
    Subscriptions,
    Queue,
    Inbox,
    ListenHistory,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountListInput {
    resource: AccountResource,
    #[serde(default = "default_since_days")]
    since_days: i64,
    #[serde(default)]
    query: Option<String>,
    #[serde(default = "default_cursor")]
    cursor: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountPage<T> {
    account: String,
    resource: AccountResource,
    items: Vec<T>,
    next_cursor: Option<usize>,
    total: usize,
}

fn account_page<T: Serialize>(
    account: &str,
    resource: AccountResource,
    page: Page<T>,
) -> serde_json::Value {
    serde_json::to_value(AccountPage {
        account: account.to_owned(),
        resource,
        items: page.items,
        next_cursor: page.next_cursor,
        total: page.total,
    })
    .unwrap_or(serde_json::Value::Null)
}

async fn account_list(
    state: McpState,
    input: AccountListInput,
) -> Result<serde_json::Value, ToolError> {
    let account = require_account(&state)?;
    let query = input.query.as_deref().map(str::trim);
    let name = account.name().to_owned();
    let value = match input.resource {
        AccountResource::Subscriptions => {
            let items: Vec<_> = require_available(account.fetch_subscriptions().await)?
                .into_iter()
                .filter(|s| matches_query(&[Some(&s.title)], query))
                .collect();
            account_page(
                &name,
                input.resource,
                paginate(items, input.cursor, input.limit),
            )
        }
        AccountResource::Queue => {
            let items: Vec<_> = require_available(account.fetch_queue().await)?
                .into_iter()
                .filter(|q| matches_query(&[Some(&q.show_title), Some(&q.episode_title)], query))
                .collect();
            account_page(
                &name,
                input.resource,
                paginate(items, input.cursor, input.limit),
            )
        }
        AccountResource::Inbox => {
            let items: Vec<_> = require_available(account.fetch_inbox().await)?
                .into_iter()
                .filter(|q| matches_query(&[Some(&q.show_title), Some(&q.episode_title)], query))
                .collect();
            account_page(
                &name,
                input.resource,
                paginate(items, input.cursor, input.limit),
            )
        }
        AccountResource::ListenHistory => {
            let since = state.clock.now_ms() - input.since_days * DAY_MS;
            let items: Vec<_> = require_available(account.fetch_listen_history(Some(since)).await)?
                .into_iter()
                .filter(|q| matches_query(&[Some(&q.show_title), Some(&q.episode_title)], query))
                .collect();
            account_page(
                &name,
                input.resource,
                paginate(items, input.cursor, input.limit),
            )
        }
    };
    Ok(value)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SearchResource {
    Shows,
    Episodes,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountSearchInput {
    resource: SearchResource,
    query: String,
    #[serde(default = "default_cursor")]
    cursor: usize,
    #[serde(default = "default_search_limit")]
    limit: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchPage<T> {
    account: String,
    resource: SearchResource,
    items: Vec<T>,
    next_cursor: Option<usize>,
    total: usize,
}

async fn account_search(
    state: McpState,
    input: AccountSearchInput,
) -> Result<serde_json::Value, ToolError> {
    let query = input.query.trim();
    if query.is_empty() {
        return Err(ToolError::input("query must not be empty"));
    }
    let account = require_account(&state)?;
    let name = account.name().to_owned();
    let to_value = |v: Result<serde_json::Value, serde_json::Error>| {
        v.map_err(|e| ToolError::output(e.to_string()))
    };
    match input.resource {
        SearchResource::Shows => {
            let page = paginate(
                require_available(account.search_podcasts(query).await)?,
                input.cursor,
                input.limit,
            );
            to_value(serde_json::to_value(SearchPage {
                account: name,
                resource: input.resource,
                items: page.items,
                next_cursor: page.next_cursor,
                total: page.total,
            }))
        }
        SearchResource::Episodes => {
            let page = paginate(
                require_available(account.search_episodes(query).await)?,
                input.cursor,
                input.limit,
            );
            to_value(serde_json::to_value(SearchPage {
                account: name,
                resource: input.resource,
                items: page.items,
                next_cursor: page.next_cursor,
                total: page.total,
            }))
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum AccountUpdateInput {
    #[serde(rename_all = "camelCase")]
    Enqueue {
        feed_url: String,
        #[serde(default)]
        itunes_id: Option<i64>,
        episode_guid: String,
        #[serde(default)]
        media_url: Option<String>,
        show_title: String,
        episode_title: String,
        #[serde(default)]
        position: Option<PodcastQueuePosition>,
    },
    #[serde(rename_all = "camelCase")]
    Dequeue { episode_guid: String },
    #[serde(rename_all = "camelCase")]
    ClearInbox { client_episode_id: String },
    #[serde(rename_all = "camelCase")]
    Subscribe {
        title: String,
        feed_url: String,
        #[serde(default)]
        itunes_id: Option<i64>,
    },
}

#[derive(Serialize)]
struct AccountUpdateOutput {
    account: String,
    action: &'static str,
    result: PodcastWriteResult,
}

fn trimmed_non_empty(value: &str, field: &str) -> Result<String, ToolError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ToolError::input(format!("{field} must not be empty")));
    }
    Ok(trimmed.to_owned())
}

async fn account_update(
    state: McpState,
    input: AccountUpdateInput,
) -> Result<AccountUpdateOutput, ToolError> {
    let account = require_account(&state)?;
    let (action, result) = match input {
        AccountUpdateInput::Enqueue {
            feed_url,
            itunes_id,
            episode_guid,
            media_url,
            show_title,
            episode_title,
            position,
        } => {
            let request = EnqueueEpisodeRequest {
                feed_url,
                itunes_id,
                episode_guid,
                media_url,
                show_title: trimmed_non_empty(&show_title, "showTitle")?,
                episode_title: trimmed_non_empty(&episode_title, "episodeTitle")?,
                position: Some(position.unwrap_or_default()),
            };
            ("enqueue", account.enqueue_episode(request).await)
        }
        AccountUpdateInput::Dequeue { episode_guid } => {
            ("dequeue", account.dequeue_episode(&episode_guid).await)
        }
        AccountUpdateInput::ClearInbox { client_episode_id } => (
            "clear_inbox",
            account.clear_inbox_episode(&client_episode_id).await,
        ),
        AccountUpdateInput::Subscribe {
            title,
            feed_url,
            itunes_id,
        } => {
            let request = SubscribeToShowRequest {
                title: trimmed_non_empty(&title, "title")?,
                feed_url,
                itunes_id,
            };
            ("subscribe", account.subscribe_to_show(request).await)
        }
    };
    Ok(AccountUpdateOutput {
        account: account.name().to_owned(),
        action,
        result,
    })
}

/// The MCP recommendation shape (`serializePodcastRecommendation` in podcasts.ts).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct McpRecommendation {
    recommendation_id: String,
    episode_id: String,
    show_id: String,
    show_title: String,
    episode_title: String,
    feed_url: String,
    itunes_id: Option<i64>,
    episode_guid: String,
    media_url: Option<String>,
    episode_url: Option<String>,
    published_at: i64,
    duration_minutes: Option<i64>,
    status: PodcastRecommendationStatus,
    why_for_user: Option<String>,
    caveats: Vec<String>,
    #[serde(serialize_with = "js_number_opt")]
    confidence: Option<f64>,
    discovered_via: Option<String>,
    matched_voices: Vec<String>,
    recommended_at: i64,
    notified_at: Option<i64>,
    resolved_at: Option<i64>,
    queue_result: Option<PodcastQueueResult>,
    feedback: Option<PodcastFeedback>,
    feedback_at: Option<i64>,
    feedback_note: Option<String>,
}

fn serialize_mcp_recommendation(rec: &PodcastRecommendationData) -> McpRecommendation {
    McpRecommendation {
        recommendation_id: rec.recommendation_id.clone(),
        episode_id: rec.episode_id.clone(),
        show_id: rec.show_id.clone(),
        show_title: rec.show_title.clone(),
        episode_title: rec.episode_title.clone(),
        feed_url: rec.feed_url.clone(),
        itunes_id: rec.itunes_id,
        episode_guid: rec.episode_guid.clone(),
        media_url: rec.media_url.clone(),
        episode_url: rec.episode_url.clone(),
        published_at: rec.published_at,
        duration_minutes: rec.duration_minutes,
        status: rec.status,
        why_for_user: rec.why_for_user.clone(),
        caveats: rec.caveats.clone().unwrap_or_default(),
        confidence: rec.confidence,
        discovered_via: rec.discovered_via.clone(),
        matched_voices: rec.matched_voices.clone().unwrap_or_default(),
        recommended_at: rec.recommended_at,
        notified_at: rec.notified_at,
        resolved_at: rec.resolved_at,
        queue_result: rec.queue_result,
        feedback: rec.feedback,
        feedback_at: rec.feedback_at,
        feedback_note: rec.feedback_note.clone(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FeedbackFilter {
    GoodPick,
    NotForMe,
    None,
}

#[derive(Debug, Deserialize)]
struct RecommendationsListInput {
    #[serde(default)]
    status: Option<PodcastRecommendationStatus>,
    #[serde(default)]
    feedback: Option<FeedbackFilter>,
    #[serde(default = "default_cursor")]
    cursor: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

async fn recommendations_list(
    state: McpState,
    input: RecommendationsListInput,
) -> Result<Page<McpRecommendation>, ToolError> {
    let values: Vec<McpRecommendation> = get_all_podcast_recommendations(&state.store)
        .await
        .map_err(store_error)?
        .iter()
        .filter(|r| input.status.is_none_or(|s| r.status == s))
        .filter(|r| match input.feedback {
            None => true,
            Some(FeedbackFilter::None) => r.feedback.is_none(),
            Some(FeedbackFilter::GoodPick) => r.feedback == Some(PodcastFeedback::GoodPick),
            Some(FeedbackFilter::NotForMe) => r.feedback == Some(PodcastFeedback::NotForMe),
        })
        .map(serialize_mcp_recommendation)
        .collect();
    Ok(paginate(values, input.cursor, input.limit))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecommendationGetInput {
    recommendation_id: String,
}

#[derive(Serialize)]
struct RecommendationOutput {
    recommendation: McpRecommendation,
}

async fn recommendation_get(
    state: McpState,
    input: RecommendationGetInput,
) -> Result<RecommendationOutput, ToolError> {
    match get_podcast_recommendation(&state.store, &input.recommendation_id)
        .await
        .map_err(store_error)?
    {
        Some(rec) => Ok(RecommendationOutput {
            recommendation: serialize_mcp_recommendation(&rec),
        }),
        None => Err(ToolError::execute("Podcast recommendation not found")),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecommendationFeedbackInput {
    recommendation_id: String,
    #[serde(default)]
    feedback: Option<PodcastFeedback>,
    #[serde(default)]
    note: Option<String>,
}

async fn recommendation_feedback(
    state: McpState,
    input: RecommendationFeedbackInput,
) -> Result<RecommendationOutput, ToolError> {
    if input.feedback.is_none() && input.note.is_none() {
        return Err(ToolError::input("feedback or note is required"));
    }
    let note = input.note.as_deref().map(|n| n.trim().to_owned());
    match set_podcast_recommendation_feedback(
        &state.store,
        &input.recommendation_id,
        input.feedback,
        note,
        state.clock.now_ms(),
    )
    .await
    .map_err(store_error)?
    {
        Some(rec) => Ok(RecommendationOutput {
            recommendation: serialize_mcp_recommendation(&rec),
        }),
        None => Err(ToolError::execute("Podcast recommendation not found")),
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "resource", rename_all = "snake_case")]
enum TasteReadInput {
    Profile {},
    Evidence {
        #[serde(default = "default_cursor")]
        cursor: usize,
        #[serde(default = "default_limit")]
        limit: usize,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct McpClaim {
    claim: String,
    #[serde(serialize_with = "js_number")]
    confidence: f64,
    evidence_ids: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct McpProfile {
    profile_id: String,
    version: i64,
    generated_at: i64,
    evidence_fingerprint: String,
    evidence_count: u64,
    model_id: String,
    prompt_version: String,
    summary: String,
    stable_preferences: Vec<McpClaim>,
    conditional_preferences: Vec<McpClaim>,
    aversions: Vec<McpClaim>,
    current_saturation: Vec<McpClaim>,
    exploration_targets: Vec<McpClaim>,
    uncertainties: Vec<McpClaim>,
    stats: crate::reflection::types::PodcastBehavioralStats,
}

fn mcp_claims(claims: &[PodcastTasteClaim]) -> Vec<McpClaim> {
    claims
        .iter()
        .map(|c| McpClaim {
            claim: c.claim.clone(),
            confidence: c.confidence,
            evidence_ids: c.evidence_ids.clone(),
        })
        .collect()
}

fn mcp_profile(profile: PodcastTasteProfileData) -> McpProfile {
    McpProfile {
        stable_preferences: mcp_claims(&profile.stable_preferences),
        conditional_preferences: mcp_claims(&profile.conditional_preferences),
        aversions: mcp_claims(&profile.aversions),
        current_saturation: mcp_claims(&profile.current_saturation),
        exploration_targets: mcp_claims(&profile.exploration_targets),
        uncertainties: mcp_claims(&profile.uncertainties),
        profile_id: profile.profile_id,
        version: profile.version,
        generated_at: profile.generated_at,
        evidence_fingerprint: profile.evidence_fingerprint,
        evidence_count: profile.evidence_count,
        model_id: profile.model_id,
        prompt_version: profile.prompt_version,
        summary: profile.summary,
        stats: profile.stats,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct McpEvidence {
    evidence_id: String,
    kind: PodcastTasteEvidenceKind,
    show_key: String,
    show_title: String,
    episode_title: Option<String>,
    observed_at: i64,
    #[serde(serialize_with = "js_number_opt")]
    completion: Option<f64>,
    recommendation_id: Option<String>,
    feedback: Option<PodcastFeedback>,
    note: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "resource", rename_all = "snake_case")]
enum TasteReadOutput {
    Profile {
        profile: Option<Box<McpProfile>>,
    },
    Evidence {
        items: Vec<McpEvidence>,
        #[serde(rename = "nextCursor")]
        next_cursor: Option<usize>,
        total: usize,
    },
}

async fn taste_read(state: McpState, input: TasteReadInput) -> Result<TasteReadOutput, ToolError> {
    match input {
        TasteReadInput::Profile {} => Ok(TasteReadOutput::Profile {
            profile: get_latest_podcast_taste_profile(&state.store)
                .await
                .map_err(store_error)?
                .map(|p| Box::new(mcp_profile(p))),
        }),
        TasteReadInput::Evidence { cursor, limit } => {
            let values: Vec<McpEvidence> = get_all_podcast_taste_evidence(&state.store)
                .await
                .map_err(store_error)?
                .into_iter()
                .map(|e| McpEvidence {
                    evidence_id: e.evidence_id,
                    kind: e.kind,
                    show_key: e.show_key,
                    show_title: e.show_title,
                    episode_title: e.episode_title,
                    observed_at: e.observed_at,
                    completion: e.completion,
                    recommendation_id: e.recommendation_id,
                    feedback: e.feedback,
                    note: e.note,
                })
                .collect();
            let page = paginate(values, cursor, limit);
            Ok(TasteReadOutput::Evidence {
                items: page.items,
                next_cursor: page.next_cursor,
                total: page.total,
            })
        }
    }
}

/// The seven podcast tools in `src/mcp/tools/podcasts.ts` order.
pub fn tools(state: McpState) -> Result<Vec<McpTool>, ToolMetaError> {
    let s = state;
    Ok(vec![
        typed_tool("podcast_account_list", {
            let s = s.clone();
            move |input: AccountListInput, _cx| account_list(s.clone(), input)
        })?,
        typed_tool("podcast_account_search", {
            let s = s.clone();
            move |input: AccountSearchInput, _cx| account_search(s.clone(), input)
        })?,
        typed_tool("podcast_account_update", {
            let s = s.clone();
            move |input: AccountUpdateInput, _cx| account_update(s.clone(), input)
        })?,
        typed_tool("podcast_recommendations_list", {
            let s = s.clone();
            move |input: RecommendationsListInput, _cx| recommendations_list(s.clone(), input)
        })?,
        typed_tool("podcast_recommendation_get", {
            let s = s.clone();
            move |input: RecommendationGetInput, _cx| recommendation_get(s.clone(), input)
        })?,
        typed_tool("podcast_recommendation_feedback", {
            let s = s.clone();
            move |input: RecommendationFeedbackInput, _cx| recommendation_feedback(s.clone(), input)
        })?,
        typed_tool("podcast_taste_read", move |input: TasteReadInput, _cx| {
            taste_read(s.clone(), input)
        })?,
    ])
}
