//! Episode rows and the durable job queue (`src/press-pods/persistence.ts`).
//!
//! Submissions become durable `press-pods-job` rows that the PressPods task
//! drains. A crash mid-processing leaves a stale `processing` row that the
//! next sweep reclaims; transient failures requeue with exponential backoff;
//! permanent failures stay visible for a manual retry. Persistence stays free
//! of filesystem I/O: deletions return the removed rows so the caller can
//! remove their audio files.

use omni_store::entity::{EntityOps, EntityWrite, ModifyOpts, UpsertOpts};
use omni_store::{Store, StoreError};

use crate::error::PressPodsError;
use crate::model::{JobStatus, PressPodsEpisode, PressPodsJob};
use crate::url::normalize_url;

/// Attempts before a retryable failure becomes permanent.
pub const MAX_JOB_ATTEMPTS: i64 = 6;
/// First retry delay; doubles per attempt.
pub const BASE_RETRY_DELAY_MS: i64 = 60_000;
/// A `processing` claim older than this is presumed crashed and reclaimed.
pub const STALE_CLAIM_MS: i64 = 30 * 60_000;

/// CSPRNG ids: episode ids double as publicly served audio file names whose
/// only protection is unguessability.
pub fn secure_id() -> String {
    omni_core::ids::secure_id_b64url(16)
}

/// Canonical identity of a job, computed on read so rows written before
/// `normalizedUrl` existed still dedup.
pub fn job_normalized_url(job: &PressPodsJob) -> String {
    job.normalized_url
        .clone()
        .unwrap_or_else(|| normalize_url(&job.url))
}

/// Canonical identity of an episode (see [`job_normalized_url`]).
pub fn episode_normalized_url(episode: &PressPodsEpisode) -> String {
    episode
        .normalized_url
        .clone()
        .unwrap_or_else(|| normalize_url(&episode.article_url))
}

/// `60 s * 2^(attempts - 1)`.
pub fn retry_delay_ms(attempts: i64) -> i64 {
    let exponent = u32::try_from((attempts - 1).max(0))
        .unwrap_or(u32::MAX)
        .min(40);
    BASE_RETRY_DELAY_MS.saturating_mul(1_i64 << exponent)
}

/// Jobs runnable now: due queued rows plus stale processing claims, oldest
/// submission first.
pub fn select_due_jobs(rows: Vec<PressPodsJob>, now: i64) -> Vec<PressPodsJob> {
    let mut due: Vec<PressPodsJob> = rows
        .into_iter()
        .filter(|job| match job.status {
            JobStatus::Queued => job.next_attempt_at <= now,
            JobStatus::Processing => job.claimed_at.unwrap_or(0) <= now - STALE_CLAIM_MS,
            JobStatus::Failed => false,
        })
        .collect();
    due.sort_by_key(|job| job.created_at);
    due
}

/// The outcome of a failed attempt: requeue with backoff, or fail once the
/// error is permanent or attempts are exhausted. Pure; `base` is the live row.
pub fn failed_job(base: PressPodsJob, error: &str, retryable: bool, now: i64) -> PressPodsJob {
    let attempts = base.attempts + 1;
    let (status, next_attempt_at) = if retryable && attempts < MAX_JOB_ATTEMPTS {
        (JobStatus::Queued, now + retry_delay_ms(attempts))
    } else {
        (JobStatus::Failed, 0)
    };
    PressPodsJob {
        attempts,
        last_error: Some(error.to_owned()),
        updated_at: now,
        claimed_at: None,
        status,
        next_attempt_at,
        ..base
    }
}

fn sorted_newest_first<T>(mut rows: Vec<T>, created_at: impl Fn(&T) -> i64) -> Vec<T> {
    rows.sort_by_key(|row| std::cmp::Reverse(created_at(row)));
    rows
}

fn decode_error(operation: &'static str, error: StoreError) -> PressPodsError {
    match error {
        StoreError::Decode { pk, source } => {
            PressPodsError::invalid(operation, format!("{pk}: {source}"))
        }
        other => PressPodsError::store(operation, other),
    }
}

/// The PressPods docstore API (TS `PressPodsPersistence`).
#[derive(Clone)]
pub struct Persistence {
    store: Store,
}

impl Persistence {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Every episode, newest first.
    pub async fn get_all_episodes(&self) -> Result<Vec<PressPodsEpisode>, PressPodsError> {
        let rows = self
            .store
            .read(|docs| docs.get_all::<PressPodsEpisode>())
            .await
            .map_err(|e| decode_error("decode PressPods episode", e))?;
        Ok(sorted_newest_first(rows, |e| e.created_at))
    }

    /// One episode; a row the typed model rejects is `InvalidData`.
    pub async fn get_episode(
        &self,
        episode_id: &str,
    ) -> Result<Option<PressPodsEpisode>, PressPodsError> {
        let key = episode_id.to_owned();
        self.store
            .read(move |docs| docs.get::<PressPodsEpisode>(&key))
            .await
            .map_err(|e| decode_error("decode PressPods episode", e))
    }

    /// Crash-recovery idempotency probe: an episode for this job's URL created
    /// after the job was submitted means the pipeline completed but the
    /// process died before the job row was deleted.
    pub async fn find_episode_for_job(
        &self,
        job: &PressPodsJob,
    ) -> Result<Option<PressPodsEpisode>, PressPodsError> {
        let normalized = job_normalized_url(job);
        let created_at = job.created_at;
        let rows = self
            .store
            .read(|docs| docs.get_all::<PressPodsEpisode>())
            .await
            .map_err(|e| decode_error("decode PressPods episode", e))?;
        Ok(rows.into_iter().find(|episode| {
            episode_normalized_url(episode) == normalized && episode.created_at >= created_at
        }))
    }

    pub async fn upsert_episode(&self, episode: PressPodsEpisode) -> Result<(), PressPodsError> {
        self.store
            .write(move |tx| tx.upsert(&episode, UpsertOpts::default()))
            .await
            .map_err(|e| PressPodsError::store("persist PressPods episode", e))
    }

    /// Enqueues a fresh job for `url`.
    pub async fn enqueue_episode_job(&self, url: &str) -> Result<PressPodsJob, PressPodsError> {
        let url = url.to_owned();
        self.store
            .write(move |tx| {
                let now = omni_store::DocOps::now_ms(tx);
                let job = PressPodsJob {
                    job_id: secure_id(),
                    normalized_url: Some(normalize_url(&url)),
                    url,
                    status: JobStatus::Queued,
                    attempts: 0,
                    next_attempt_at: 0,
                    last_error: None,
                    created_at: now,
                    updated_at: now,
                    claimed_at: None,
                    last_run_id: None,
                    extra: Default::default(),
                };
                tx.upsert(&job, UpsertOpts::default())?;
                Ok::<_, StoreError>(job)
            })
            .await
            .map_err(|e| PressPodsError::store("enqueue PressPods job", e))
    }

    /// An in-flight (queued or processing) job for this URL, newest first.
    pub async fn find_active_job_by_normalized_url(
        &self,
        normalized_url: &str,
    ) -> Result<Option<PressPodsJob>, PressPodsError> {
        Ok(self.get_all_jobs().await?.into_iter().find(|job| {
            matches!(job.status, JobStatus::Queued | JobStatus::Processing)
                && job_normalized_url(job) == normalized_url
        }))
    }

    /// A failed job for this URL; a resubmit requeues it rather than stacking.
    pub async fn find_failed_job_by_normalized_url(
        &self,
        normalized_url: &str,
    ) -> Result<Option<PressPodsJob>, PressPodsError> {
        Ok(self.get_all_jobs().await?.into_iter().find(|job| {
            job.status == JobStatus::Failed && job_normalized_url(job) == normalized_url
        }))
    }

    /// Marks a job `processing` for this run.
    pub async fn claim_job(
        &self,
        job_id: &str,
        run_id: Option<String>,
    ) -> Result<(), PressPodsError> {
        let key = job_id.to_owned();
        self.store
            .write(move |tx| {
                let now = omni_store::DocOps::now_ms(tx);
                tx.update::<PressPodsJob>(
                    &key,
                    |job| PressPodsJob {
                        status: JobStatus::Processing,
                        claimed_at: Some(now),
                        updated_at: now,
                        last_run_id: run_id,
                        ..job
                    },
                    ModifyOpts::default(),
                )
                .map(|_| ())
            })
            .await
            .map_err(|e| PressPodsError::store("claim PressPods job", e))
    }

    pub async fn complete_job(&self, job_id: &str) -> Result<(), PressPodsError> {
        self.delete_job(job_id).await
    }

    /// Requeue with backoff, or mark failed once attempts are exhausted. The
    /// update is based on the live row (fields written since selection, such
    /// as the claim's `lastRunId`, survive). A missing row means a concurrent
    /// delete: the outcome is computed for logging but the job is not
    /// resurrected.
    pub async fn record_job_failure(
        &self,
        job: &PressPodsJob,
        error: &str,
        retryable: bool,
    ) -> Result<PressPodsJob, PressPodsError> {
        let snapshot = job.clone();
        let error = error.to_owned();
        self.store
            .write(move |tx| {
                let now = omni_store::DocOps::now_ms(tx);
                let existing = tx.get::<PressPodsJob>(&snapshot.job_id)?;
                let exists = existing.is_some();
                let updated = failed_job(existing.unwrap_or(snapshot), &error, retryable, now);
                if exists {
                    tx.upsert(&updated, UpsertOpts::default())?;
                }
                Ok::<_, StoreError>(updated)
            })
            .await
            .map_err(|e| PressPodsError::store("record PressPods job failure", e))
    }

    /// One job; a row the typed model rejects is `InvalidData`.
    pub async fn get_job(&self, job_id: &str) -> Result<Option<PressPodsJob>, PressPodsError> {
        let key = job_id.to_owned();
        self.store
            .read(move |docs| docs.get::<PressPodsJob>(&key))
            .await
            .map_err(|e| decode_error("decode PressPods job", e))
    }

    /// Manual retry (or a resubmit joining a failed job): reset a failed job
    /// to run now with a fresh attempt budget. Only failed jobs qualify, so
    /// an in-flight or queued attempt is never clobbered.
    pub async fn requeue_job_now(
        &self,
        job_id: &str,
    ) -> Result<Option<PressPodsJob>, PressPodsError> {
        let key = job_id.to_owned();
        self.store
            .write(move |tx| {
                let now = omni_store::DocOps::now_ms(tx);
                let Some(job) = tx.get::<PressPodsJob>(&key)? else {
                    return Ok(None);
                };
                if job.status != JobStatus::Failed {
                    return Ok(None);
                }
                let updated = PressPodsJob {
                    status: JobStatus::Queued,
                    attempts: 0,
                    next_attempt_at: 0,
                    claimed_at: None,
                    last_error: None,
                    updated_at: now,
                    ..job
                };
                tx.upsert(&updated, UpsertOpts::default())?;
                Ok::<_, StoreError>(Some(updated))
            })
            .await
            .map_err(|e| PressPodsError::store("requeue PressPods job", e))
    }

    /// Every job, newest first.
    pub async fn get_all_jobs(&self) -> Result<Vec<PressPodsJob>, PressPodsError> {
        let rows = self
            .store
            .read(|docs| docs.get_all::<PressPodsJob>())
            .await
            .map_err(|e| decode_error("decode PressPods job", e))?;
        Ok(sorted_newest_first(rows, |j| j.created_at))
    }

    /// Boot-time crash recovery: in this single-process deployment any job
    /// left `processing` was orphaned by a restart, so its claim is made
    /// immediately stale. Returns the number of orphaned jobs.
    pub async fn reclaim_processing_jobs_at_boot(&self) -> Result<usize, PressPodsError> {
        self.store
            .write(|tx| {
                let orphaned: Vec<PressPodsJob> = tx
                    .get_all::<PressPodsJob>()?
                    .into_iter()
                    .filter(|job| job.status == JobStatus::Processing)
                    .collect();
                for job in &orphaned {
                    tx.update::<PressPodsJob>(
                        &job.job_id,
                        |job| PressPodsJob {
                            claimed_at: Some(0),
                            ..job
                        },
                        ModifyOpts::default(),
                    )?;
                }
                Ok::<_, StoreError>(orphaned.len())
            })
            .await
            .map_err(|e| PressPodsError::store("reclaim PressPods jobs", e))
    }

    /// Removes an episode row and returns it (the caller removes its audio).
    pub async fn delete_episode(
        &self,
        episode_id: &str,
    ) -> Result<Option<PressPodsEpisode>, PressPodsError> {
        let key = episode_id.to_owned();
        self.store
            .write(move |tx| {
                let Some(episode) = tx.get::<PressPodsEpisode>(&key)? else {
                    return Ok(None);
                };
                tx.delete::<PressPodsEpisode>(&key)?;
                Ok::<_, StoreError>(Some(episode))
            })
            .await
            .map_err(|e| decode_error("delete PressPods episode", e))
    }

    pub async fn delete_job(&self, job_id: &str) -> Result<(), PressPodsError> {
        let key = job_id.to_owned();
        self.store
            .write(move |tx| tx.delete::<PressPodsJob>(&key).map(|_| ()))
            .await
            .map_err(|e| PressPodsError::store("delete PressPods job", e))
    }

    /// Replace semantics for resubmit-as-retry: drops every episode other
    /// than `keep_episode_id` sharing the canonical URL and returns them.
    pub async fn delete_episodes_by_normalized_url_except(
        &self,
        normalized_url: &str,
        keep_episode_id: &str,
    ) -> Result<Vec<PressPodsEpisode>, PressPodsError> {
        let normalized = normalized_url.to_owned();
        let keep = keep_episode_id.to_owned();
        self.store
            .write(move |tx| {
                let stale: Vec<PressPodsEpisode> = tx
                    .get_all::<PressPodsEpisode>()?
                    .into_iter()
                    .filter(|e| e.episode_id != keep && episode_normalized_url(e) == normalized)
                    .collect();
                for episode in &stale {
                    tx.delete::<PressPodsEpisode>(&episode.episode_id)?;
                }
                Ok::<_, StoreError>(stale)
            })
            .await
            .map_err(|e| PressPodsError::store("replace PressPods episodes", e))
    }
}
