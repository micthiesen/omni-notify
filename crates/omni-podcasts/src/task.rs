//! `PodcastRecs` task (`task.ts`): scheduled runs target 3 topic picks;
//! manual runs take `{maxRecommendations: 1..=5}`.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use futures::future::BoxFuture;
use omni_tasks::{CronSchedule, RunContext, Task, TaskError, TaskOptions};
use serde_json::Value;

use crate::js::log_timestamp;
use crate::log_file::LogFile;
use crate::pipeline::{
    PipelineError, PodcastDeps, PodcastPipelineOptions, range_error, run_podcast_pipeline,
};

const LOG: &str = "PodcastRecsTask";
pub const TASK_NAME: &str = "PodcastRecs";
const SCHEDULED_TOPIC_TARGET: i64 = 3;

pub struct PodcastRecommendationTask {
    schedule: CronSchedule,
    deps: PodcastDeps,
    last_summary: Mutex<Option<String>>,
}

impl PodcastRecommendationTask {
    pub fn new(schedule: CronSchedule, deps: PodcastDeps) -> Self {
        Self {
            schedule,
            deps,
            last_summary: Mutex::new(None),
        }
    }

    pub async fn run_pipeline(&self, max_recommendations: i64) -> Result<String, PipelineError> {
        let now = self.deps.clock.now_ms();
        let log_file = match self
            .deps
            .config
            .logs_path
            .as_deref()
            .filter(|p| !p.is_empty())
        {
            Some(logs) => Some(
                LogFile::create(
                    PathBuf::from(logs)
                        .join("podcast-recs")
                        .join(format!("{}.md", log_timestamp(now, &self.deps.tz))),
                )
                .await?,
            ),
            None => None,
        };
        tracing::info!(
            target: LOG,
            "Podcast recommendation run requested up to {max_recommendations} episode(s)"
        );
        let summary = run_podcast_pipeline(
            &self.deps,
            log_file.as_ref(),
            PodcastPipelineOptions {
                dry_run: false,
                max_recommendations: Some(max_recommendations),
            },
        )
        .await?;
        *self.last_summary.lock().unwrap_or_else(|p| p.into_inner()) = Some(summary.clone());
        tracing::info!(target: LOG, "Podcast recommendation run finished: {summary}");
        Ok(summary)
    }
}

/// `parseMaxRecommendations`: an integer 1..=5 under `maxRecommendations`.
pub fn parse_max_recommendations(input: &Value) -> Result<i64, String> {
    let value = input.get("maxRecommendations").and_then(Value::as_f64);
    match value {
        Some(v) if v.fract() == 0.0 && (1.0..=5.0).contains(&v) => Ok(v as i64),
        _ => Err(range_error()),
    }
}

impl Task for PodcastRecommendationTask {
    fn name(&self) -> &str {
        TASK_NAME
    }

    fn display_name(&self) -> Option<&str> {
        Some("Podcast Recommendations")
    }

    fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }

    fn options(&self) -> TaskOptions {
        // Off the scheduled instant so Castro is not hit at a predictable time.
        TaskOptions {
            jitter: Duration::from_secs(5 * 60),
            run_on_startup: false,
        }
    }

    fn run<'a>(&'a self, _cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(async move {
            self.run_pipeline(SCHEDULED_TOPIC_TARGET)
                .await
                .map(|_| ())
                .map_err(TaskError::from_error)
        })
    }

    fn accepts_manual_input(&self) -> bool {
        true
    }

    fn run_manual<'a>(
        &'a self,
        _cx: &'a RunContext,
        input: Value,
    ) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(async move {
            let max = parse_max_recommendations(&input).map_err(TaskError::new)?;
            self.run_pipeline(max)
                .await
                .map(|_| ())
                .map_err(TaskError::from_error)
        })
    }

    fn last_run_summary(&self) -> Option<String> {
        self.last_summary
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}
