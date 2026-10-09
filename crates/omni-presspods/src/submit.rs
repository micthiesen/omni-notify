//! The shared submission path (`src/press-pods/submit.ts`) for the public
//! endpoint, the web UI and MCP.
//!
//! Resubmitting a URL is a retry, not a new entry: an in-flight job for the
//! same canonical URL is joined, a failed one is requeued to run now. A URL
//! that already produced an episode still enqueues a fresh job (the pipeline
//! replaces the older episode on completion). New submissions are bookmarked
//! in Karakeep (best-effort) and the worker is kicked immediately.

use omni_http::public::assert_public_http_url_syntax;

use crate::error::PressPodsError;
use crate::model::PressPodsJob;
use crate::service::PressPods;
use crate::url::normalize_url;

const LOG: &str = "PressPods";

/// iOS Shortcuts sometimes duplicate the URL after a newline: keep the first
/// line, trimmed, and require a public HTTP(S) URL syntactically.
pub fn first_line_public_url(raw: &str) -> Result<String, String> {
    let url = raw.split('\n').next().unwrap_or("").trim().to_owned();
    assert_public_http_url_syntax(&url).map_err(|e| e.to_string())?;
    Ok(url)
}

impl PressPods {
    pub async fn submit_episode_url(&self, url: &str) -> Result<PressPodsJob, PressPodsError> {
        let public = self.deps.url_guard.check(url).await?;
        let validated = public.to_string();
        let normalized = normalize_url(&validated);
        let persistence = self.persistence();

        if let Some(active) = persistence
            .find_active_job_by_normalized_url(&normalized)
            .await?
        {
            tracing::info!(
                target: LOG,
                "Episode job already {} for {validated}; joining it",
                active.status.as_str()
            );
            self.kick_worker()?;
            return Ok(active);
        }

        if let Some(failed) = persistence
            .find_failed_job_by_normalized_url(&normalized)
            .await?
            && let Some(requeued) = persistence.requeue_job_now(&failed.job_id).await?
        {
            tracing::info!(target: LOG, "Retrying previously-failed episode job for {validated}");
            self.kick_worker()?;
            return Ok(requeued);
        }

        let job = persistence.enqueue_episode_job(&validated).await?;
        tracing::info!(target: LOG, "Episode job enqueued for {validated}");
        if let Err(error) = self
            .deps
            .bookmarks
            .add_bookmark(&validated, &["PressPods"])
            .await
        {
            tracing::debug!(target: LOG, %error, "Karakeep bookmark skipped");
        }
        self.kick_worker()?;
        Ok(job)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_first_line_and_requires_a_public_url() {
        assert_eq!(
            first_line_public_url("  https://a.test/x \nhttps://a.test/x").unwrap(),
            "https://a.test/x"
        );
        assert!(first_line_public_url("http://localhost/x").is_err());
        assert!(first_line_public_url("not a url").is_err());
    }
}
