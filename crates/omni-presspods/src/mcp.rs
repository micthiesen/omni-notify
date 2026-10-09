//! PressPods MCP tools (`src/mcp/tools/press-pods.ts`). Metadata (names,
//! schemas, annotations, policy) comes from the golden tool list.

use omni_mcp_kit::{McpTool, ToolError, ToolMetaError, paginate, truncate_utf16, typed_tool};
use serde::{Deserialize, Serialize};

use crate::error::PressPodsError;
use crate::model::{JobStatus, PressPodsEpisode, PressPodsJob};
use crate::persistence::job_normalized_url;
use crate::routes::{cost_cents, job_dto, js_numbers};
use crate::service::PressPods;
use crate::storage::checkpoint_work_id;
use crate::submit::first_line_public_url;

/// `serializeEpisode` for MCP (no audio file name or lead image).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpEpisode {
    pub episode_id: String,
    pub title: String,
    pub author: Option<String>,
    pub publication: Option<String>,
    pub domain: Option<String>,
    pub article_url: String,
    pub excerpt: Option<String>,
    pub voice_name: Option<String>,
    pub voice_provider: Option<String>,
    pub duration_seconds: Option<f64>,
    pub file_bytes: i64,
    pub retriever_name: Option<String>,
    pub cost_cents: Option<f64>,
    pub created_at: i64,
    pub published_at: Option<i64>,
    pub run_id: Option<String>,
    pub chapter_count: usize,
}

fn mcp_episode(e: &PressPodsEpisode) -> McpEpisode {
    McpEpisode {
        episode_id: e.episode_id.clone(),
        title: e.title.clone(),
        author: e.author.clone(),
        publication: e.publication.clone(),
        domain: e.domain.clone(),
        article_url: e.article_url.clone(),
        excerpt: e.excerpt.clone(),
        voice_name: e.voice_name.clone(),
        voice_provider: e.voice_provider.clone(),
        duration_seconds: e.duration_seconds,
        file_bytes: e.file_bytes,
        retriever_name: e.retriever_name.clone(),
        cost_cents: cost_cents(e.costs.as_ref()),
        created_at: e.created_at,
        published_at: e.published_at,
        run_id: e.run_id.clone(),
        chapter_count: e.chapters.as_ref().map_or(0, Vec::len),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListResource {
    Episodes,
    Jobs,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListInput {
    resource: ListResource,
    status: Option<JobStatus>,
    query: Option<String>,
    #[serde(default)]
    cursor: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    25
}

/// `{resource, items, nextCursor, total}` in TS key order.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListOutput {
    resource: ListResource,
    items: serde_json::Value,
    next_cursor: Option<usize>,
    total: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EpisodeInput {
    episode_id: String,
}

#[derive(Serialize)]
struct EpisodeOutput {
    episode: McpEpisode,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptInput {
    episode_id: String,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_max_chars")]
    max_chars: usize,
}

fn default_max_chars() -> usize {
    4000
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptOutput {
    episode_id: String,
    title: String,
    offset: usize,
    text: String,
    next_offset: Option<usize>,
    total_chars: usize,
    truncated: bool,
}

#[derive(Deserialize)]
struct SubmitInput {
    url: String,
}

#[derive(Serialize)]
struct JobOutput {
    job: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(tag = "resource", rename_all = "lowercase")]
enum ResourceInput {
    Episode {
        #[serde(rename = "episodeId")]
        episode_id: String,
    },
    Job {
        #[serde(rename = "jobId")]
        job_id: String,
    },
}

#[derive(Serialize)]
struct DeleteOutput {
    resource: &'static str,
    deleted: bool,
}

fn execute(error: &PressPodsError) -> ToolError {
    ToolError::execute_from(error)
}

fn job_value(job: &PressPodsJob) -> Result<serde_json::Value, ToolError> {
    serde_json::to_value(job_dto(job))
        .map(js_numbers)
        .map_err(|e| ToolError::output(e.to_string()))
}

fn contains_ci(value: Option<&str>, needle: &str) -> bool {
    value.is_some_and(|v| v.to_lowercase().contains(needle))
}

/// The six PressPods tools.
pub fn tools(service: &PressPods) -> Result<Vec<McpTool>, ToolMetaError> {
    let list = {
        let service = service.clone();
        typed_tool("presspods_list", move |input: ListInput, _cx| {
            let service = service.clone();
            async move {
                let needle = input
                    .query
                    .as_deref()
                    .map(str::trim)
                    .filter(|q| !q.is_empty())
                    .map(str::to_lowercase);
                let persistence = service.persistence();
                let page = match input.resource {
                    ListResource::Episodes => {
                        let mut values = persistence
                            .get_all_episodes()
                            .await
                            .map_err(|e| execute(&e))?;
                        if let Some(needle) = &needle {
                            values.retain(|e| {
                                contains_ci(Some(&e.title), needle)
                                    || contains_ci(e.author.as_deref(), needle)
                                    || contains_ci(e.publication.as_deref(), needle)
                                    || contains_ci(e.domain.as_deref(), needle)
                            });
                        }
                        let items: Vec<McpEpisode> = values.iter().map(mcp_episode).collect();
                        let page = paginate(items, input.cursor, input.limit);
                        (
                            serde_json::to_value(page.items),
                            page.next_cursor,
                            page.total,
                        )
                    }
                    ListResource::Jobs => {
                        let mut values =
                            persistence.get_all_jobs().await.map_err(|e| execute(&e))?;
                        if let Some(status) = input.status {
                            values.retain(|j| j.status == status);
                        }
                        if let Some(needle) = &needle {
                            values.retain(|j| contains_ci(Some(&j.url), needle));
                        }
                        let items: Vec<omni_api::presspods::PressPodsJob> =
                            values.iter().map(job_dto).collect();
                        let page = paginate(items, input.cursor, input.limit);
                        (
                            serde_json::to_value(page.items),
                            page.next_cursor,
                            page.total,
                        )
                    }
                };
                let (items, next_cursor, total) = page;
                Ok(ListOutput {
                    resource: input.resource,
                    items: js_numbers(items.map_err(|e| ToolError::output(e.to_string()))?),
                    next_cursor,
                    total,
                })
            }
        })?
    };

    let episode_get = {
        let service = service.clone();
        typed_tool("presspods_episode_get", move |input: EpisodeInput, _cx| {
            let service = service.clone();
            async move {
                let episode = service
                    .persistence()
                    .get_episode(&input.episode_id)
                    .await
                    .map_err(|e| execute(&e))?
                    .ok_or_else(|| ToolError::execute("PressPods episode not found"))?;
                Ok(EpisodeOutput {
                    episode: mcp_episode(&episode),
                })
            }
        })?
    };

    let transcript = {
        let service = service.clone();
        typed_tool(
            "presspods_transcript_read",
            move |input: TranscriptInput, _cx| {
                let service = service.clone();
                async move {
                    let episode = service
                        .persistence()
                        .get_episode(&input.episode_id)
                        .await
                        .map_err(|e| execute(&e))?
                        .ok_or_else(|| ToolError::execute("PressPods episode not found"))?;
                    let total = omni_core::js::utf16_len(&episode.content);
                    if input.offset > total {
                        return Err(ToolError::execute(
                            "Transcript offset is beyond the end of the episode",
                        ));
                    }
                    let rest = omni_core::js::utf16_slice(&episode.content, input.offset, total);
                    let (text, truncated) = truncate_utf16(&rest, input.max_chars);
                    let next_offset =
                        truncated.then(|| input.offset + omni_core::js::utf16_len(&text));
                    Ok(TranscriptOutput {
                        episode_id: input.episode_id,
                        title: episode.title,
                        offset: input.offset,
                        text,
                        next_offset,
                        total_chars: total,
                        truncated,
                    })
                }
            },
        )?
    };

    let submit = {
        let service = service.clone();
        typed_tool("presspods_submit", move |input: SubmitInput, _cx| {
            let service = service.clone();
            async move {
                let url = first_line_public_url(&input.url).map_err(ToolError::input)?;
                let job = service
                    .submit_episode_url(&url)
                    .await
                    .map_err(|e| execute(&e))?;
                Ok(JobOutput {
                    job: job_value(&job)?,
                })
            }
        })?
    };

    let retry = {
        let service = service.clone();
        typed_tool("presspods_retry", move |input: ResourceInput, _cx| {
            let service = service.clone();
            async move {
                let persistence = service.persistence();
                let job = match input {
                    ResourceInput::Episode { episode_id } => {
                        let episode = persistence
                            .get_episode(&episode_id)
                            .await
                            .map_err(|e| execute(&e))?
                            .ok_or_else(|| ToolError::execute("PressPods episode not found"))?;
                        service
                            .submit_episode_url(&episode.article_url)
                            .await
                            .map_err(|e| execute(&e))?
                    }
                    ResourceInput::Job { job_id } => {
                        let existing = persistence
                            .get_job(&job_id)
                            .await
                            .map_err(|e| execute(&e))?
                            .ok_or_else(|| ToolError::execute("PressPods job not found"))?;
                        if existing.status != JobStatus::Failed {
                            return Err(ToolError::execute(
                                "Only failed PressPods jobs can be retried",
                            ));
                        }
                        let requeued = persistence
                            .requeue_job_now(&job_id)
                            .await
                            .map_err(|e| execute(&e))?
                            .ok_or_else(|| {
                                ToolError::execute("PressPods job could not be retried")
                            })?;
                        service.kick_worker().map_err(|e| execute(&e))?;
                        requeued
                    }
                };
                Ok(JobOutput {
                    job: job_value(&job)?,
                })
            }
        })?
    };

    let delete = {
        let service = service.clone();
        typed_tool("presspods_delete", move |input: ResourceInput, _cx| {
            let service = service.clone();
            async move {
                let persistence = service.persistence();
                let resource = match input {
                    ResourceInput::Episode { episode_id } => {
                        let episode = persistence
                            .delete_episode(&episode_id)
                            .await
                            .map_err(|e| execute(&e))?
                            .ok_or_else(|| ToolError::execute("PressPods episode not found"))?;
                        service
                            .audio()
                            .delete_episode_audio(&episode.audio_file)
                            .await;
                        "episode"
                    }
                    ResourceInput::Job { job_id } => {
                        let job = persistence
                            .get_job(&job_id)
                            .await
                            .map_err(|e| execute(&e))?
                            .ok_or_else(|| ToolError::execute("PressPods job not found"))?;
                        if job.status == JobStatus::Processing {
                            return Err(ToolError::execute(
                                "A processing PressPods job cannot be dismissed",
                            ));
                        }
                        persistence
                            .delete_job(&job.job_id)
                            .await
                            .map_err(|e| execute(&e))?;
                        service
                            .audio()
                            .clear_chunk_checkpoints(&checkpoint_work_id(&job_normalized_url(&job)))
                            .await;
                        "job"
                    }
                };
                Ok(DeleteOutput {
                    resource,
                    deleted: true,
                })
            }
        })?
    };

    Ok(vec![list, episode_get, transcript, submit, retry, delete])
}
