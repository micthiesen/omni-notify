//! `presspods.job_finished` publications for the queue worker.
//!
//! A published episode is keyed by its episode ID and is published before the
//! job completes, so a crash in between replays the same key from the
//! recovered job. A permanent failure is keyed by job and attempt count.
//! Callbacks carry the article's hostname only, never the full URL.

use omni_api::events::{
    PRESSPODS_JOB_FINISHED, PresspodsJobFinished, PresspodsOutcome, bounded_title,
};
use omni_runtime::ports::{EventPublication, Ports};
use serde_json::Value;

use crate::model::{PressPodsEpisode, PressPodsJob};

const LOG: &str = "PressPods";

fn host(url: &str) -> Option<String> {
    ::url::Url::parse(url)
        .ok()?
        .host_str()
        .map(|host| host.chars().take(253).collect())
}

fn publication(
    dedup_key: String,
    occurred_at_ms: i64,
    payload: PresspodsJobFinished,
) -> Option<EventPublication> {
    let Ok(Value::Object(data)) = serde_json::to_value(payload) else {
        return None;
    };
    Some(EventPublication {
        name: PRESSPODS_JOB_FINISHED,
        dedup_key,
        occurred_at_ms,
        data,
    })
}

/// `published` for the episode a job produced.
pub fn published(episode: &PressPodsEpisode, job: &PressPodsJob) -> Option<EventPublication> {
    publication(
        format!("episode:{}", episode.episode_id),
        episode.created_at,
        PresspodsJobFinished {
            outcome: PresspodsOutcome::Published,
            episode_id: Some(episode.episode_id.clone()),
            job_id: Some(job.job_id.clone()),
            title: bounded_title(&episode.title),
            article_url_host: host(&episode.article_url),
            duration_seconds: episode.duration_seconds,
            attempts: None,
        },
    )
}

/// `failed` for a job that will not be retried (its row after the failure).
pub fn failed(job: &PressPodsJob) -> Option<EventPublication> {
    publication(
        format!("failed:{}:{}", job.job_id, job.attempts),
        job.updated_at,
        PresspodsJobFinished {
            outcome: PresspodsOutcome::Failed,
            episode_id: None,
            job_id: Some(job.job_id.clone()),
            title: None,
            article_url_host: host(&job.url),
            duration_seconds: None,
            attempts: Some(job.attempts),
        },
    )
}

/// Publishes when MCP Events are enabled. A failure is logged and never
/// changes the job's outcome.
pub async fn publish(ports: &Ports, event: Option<EventPublication>) {
    let Some(event) = event else {
        return;
    };
    if let Err(error) = ports.publish_event(&event).await {
        tracing::warn!(target: LOG, error = %error, "PressPods event not published");
    }
}
