//! The `Recommendations` task (`src/recommendations/task.ts`).

use std::path::PathBuf;
use std::sync::Mutex;

use futures::future::BoxFuture;
use omni_ai::ModelRole;
use omni_tasks::{CronSchedule, RunContext, Task, TaskError, TaskOptions};

use crate::error::RecommendationError;
use crate::js::log_timestamp;
use crate::pipeline::{PipelineOptions, run_recommendation_pipeline, validate_max_recommendations};
use crate::run_log::RunLogFile;
use crate::services::{MediaServices, configured, provider_credential};

pub const TASK_NAME: &str = "Recommendations";
const LOG: &str = "Main:RecsTask";

/// Missing settings that disable the task (`MediaRecommendationTask.create`).
pub fn missing_settings(services: &MediaServices) -> Vec<String> {
    let config = &services.config;
    let mut missing = Vec::new();
    let mut check = |name: &str, value: bool| {
        if !value {
            missing.push(name.to_owned());
        }
    };
    check("TMDB_API_KEY", configured(config.tmdb_api_key.as_deref()));
    check(
        "TAVILY_API_KEY",
        configured(config.tavily_api_key.as_deref()),
    );
    let providers: Vec<&str> = [ModelRole::RecsShortlist, ModelRole::RecsSelection]
        .into_iter()
        .map(|role| config.model(role).split(':').next().unwrap_or_default())
        .collect();
    for provider in ["openai", "anthropic", "google"] {
        if providers.contains(&provider) {
            let (name, value) = provider_credential(config, provider);
            check(&name, configured(value));
        }
    }
    check("PLEX_URL", configured(config.plex_url.as_deref()));
    check("PLEX_TOKEN", configured(config.plex_token.as_deref()));
    check("RADARR_URL", configured(config.radarr_url.as_deref()));
    check(
        "RADARR_API_KEY",
        configured(config.radarr_api_key.as_deref()),
    );
    check(
        "RADARR_ROOT_FOLDER_PATH",
        configured(config.radarr_root_folder_path.as_deref()),
    );
    check(
        "RADARR_QUALITY_PROFILE_ID",
        config.radarr_quality_profile_id.is_some_and(|id| id != 0),
    );
    check("SONARR_URL", configured(config.sonarr_url.as_deref()));
    check(
        "SONARR_API_KEY",
        configured(config.sonarr_api_key.as_deref()),
    );
    check(
        "SONARR_ROOT_FOLDER_PATH",
        configured(config.sonarr_root_folder_path.as_deref()),
    );
    check(
        "SONARR_QUALITY_PROFILE_ID",
        config.sonarr_quality_profile_id.is_some_and(|id| id != 0),
    );
    missing
}

/// Media recommendations on `RECS_SCHEDULE`; never on startup (LLM-backed).
pub struct MediaRecommendationTask {
    services: MediaServices,
    schedule: CronSchedule,
    tz: jiff::tz::TimeZone,
    last_run_summary: Mutex<Option<String>>,
}

impl MediaRecommendationTask {
    /// `None` (logged) when a required setting is missing.
    pub fn create(
        services: MediaServices,
        schedule: CronSchedule,
        tz: jiff::tz::TimeZone,
    ) -> Option<Self> {
        let missing = missing_settings(&services);
        if !missing.is_empty() {
            tracing::info!(
                target: "Main",
                "Recommendations disabled: missing {}",
                missing.join(", ")
            );
            return None;
        }
        Some(Self {
            services,
            schedule,
            tz,
            last_run_summary: Mutex::new(None),
        })
    }

    async fn run_pipeline(&self, max_recommendations: f64) -> Result<(), TaskError> {
        let now = self.services.now();
        let log_file = match self.services.config.logs_path.as_deref() {
            Some(logs_path) if !logs_path.is_empty() => Some(
                RunLogFile::create(PathBuf::from(format!(
                    "{logs_path}/recommendations/{}.md",
                    log_timestamp(now, &self.tz)
                )))
                .await
                .map_err(TaskError::from_error)?,
            ),
            _ => None,
        };
        tracing::info!(
            target: LOG,
            "Recommendation run requested up to {} item(s)",
            crate::js::number(max_recommendations)
        );
        let summary = run_recommendation_pipeline(
            &self.services,
            log_file.as_ref(),
            PipelineOptions {
                dry_run: false,
                max_recommendations: Some(max_recommendations),
            },
        )
        .await
        .map_err(TaskError::from_error)?;
        if let Ok(mut last) = self.last_run_summary.lock() {
            *last = Some(summary.clone());
        }
        tracing::info!(target: LOG, "Recommendation run finished: {summary}");
        Ok(())
    }
}

/// `decodeManualInput`: `{ maxRecommendations: integer 1..=10 }`.
pub fn decode_manual_input(input: &serde_json::Value) -> Result<f64, RecommendationError> {
    let value = input
        .get("maxRecommendations")
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| {
            RecommendationError::Input(crate::error::max_recommendations_message(
                crate::pipeline::MAX_RECOMMENDATIONS_PER_RUN,
            ))
        })?;
    validate_max_recommendations(value)?;
    Ok(value)
}

impl Task for MediaRecommendationTask {
    fn name(&self) -> &str {
        TASK_NAME
    }

    fn display_name(&self) -> Option<&str> {
        Some("Media Recommendations")
    }

    fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }

    fn options(&self) -> TaskOptions {
        TaskOptions {
            jitter: std::time::Duration::ZERO,
            run_on_startup: false,
        }
    }

    fn run<'a>(&'a self, _cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(self.run_pipeline(1.0))
    }

    fn accepts_manual_input(&self) -> bool {
        true
    }

    fn run_manual<'a>(
        &'a self,
        _cx: &'a RunContext,
        input: serde_json::Value,
    ) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(async move {
            let max = decode_manual_input(&input).map_err(TaskError::from_error)?;
            self.run_pipeline(max).await
        })
    }

    fn last_run_summary(&self) -> Option<String> {
        self.last_run_summary.lock().ok().and_then(|s| s.clone())
    }
}
