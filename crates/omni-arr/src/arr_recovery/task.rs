//! The `ArrRecovery` scheduled task.

use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use omni_ai::{Ai, ModelRole};
use omni_alerts::{PushoverChannel, PushoverMessage};
use omni_config::Config;
use omni_core::clock::SharedClock;
use omni_runtime::AppContext;
use omni_store::Store;
use omni_tasks::{CronSchedule, RunContext, Task, TaskError, TaskOptions};

use super::client::{ArrClientConfig, HttpArrClient};
use super::llm::assess_with_llm;
use super::nzbget::NzbGetClient;
use super::service::{
    Assessor, HealthSource, IMPORT_SETTLE_DELAY, Notifier, RecoveryContext, run_recovery,
};
use super::types::{ArrCause, ArrKind, ArrRecoveryError, ArrResult, Decision, Evidence};
use crate::side_effects::SideEffects;

const LOG: &str = "Main";
pub const NAME: &str = "ArrRecovery";
pub const DISPLAY_NAME: &str = "Sonarr / Radarr Recovery";
pub const SCHEDULE: &str = "0 */5 * * * *";

/// Luna via the configured `ArrRecovery` model role.
pub struct LunaAssessor {
    ai: Ai,
    config: Arc<Config>,
}

impl LunaAssessor {
    pub fn new(ai: Ai, config: Arc<Config>) -> Self {
        Self { ai, config }
    }
}

impl Assessor for LunaAssessor {
    fn assess<'a>(&'a self, evidence: &'a Evidence) -> BoxFuture<'a, ArrResult<Decision>> {
        Box::pin(async move {
            let model = self
                .ai
                .model_for(&self.config, ModelRole::ArrRecovery)
                .map_err(|e| {
                    ArrRecoveryError::new("assess ARR recovery with model", ArrCause::Ai(e))
                })?;
            assess_with_llm(&self.ai, model.as_ref(), evidence).await
        })
    }
}

/// Pushover on the recommendations channel (falling back to the general token).
pub struct PushoverNotifier {
    pushover: omni_alerts::Pushover,
}

impl PushoverNotifier {
    pub fn new(pushover: omni_alerts::Pushover) -> Self {
        Self { pushover }
    }
}

impl Notifier for PushoverNotifier {
    fn send<'a>(&'a self, kind: ArrKind, message: &'a str) -> BoxFuture<'a, ArrResult<()>> {
        Box::pin(async move {
            self.pushover
                .send(
                    PushoverChannel::Recs,
                    PushoverMessage {
                        message: message.to_owned(),
                        title: Some(format!("Omni {kind} recovery")),
                        url: Some("http://omni.boris/".to_owned()),
                        url_title: Some("View task runs".to_owned()),
                        ..PushoverMessage::default()
                    },
                )
                .await
                .map(|_| ())
                .map_err(|e| {
                    ArrRecoveryError::new("send recovery notification", ArrCause::Pushover(e))
                })
        })
    }
}

/// Recovers stuck Sonarr/Radarr imports every five minutes and at startup.
pub struct ArrRecoveryTask {
    schedule: CronSchedule,
    clients: Vec<HttpArrClient>,
    store: Store,
    clock: SharedClock,
    assessor: LunaAssessor,
    notifier: PushoverNotifier,
    nzbget: Option<NzbGetClient>,
    last_summary: Mutex<Option<String>>,
}

impl ArrRecoveryTask {
    /// `ArrRecoveryTask.create`: `None` when disabled, when no Arr service is
    /// configured, or (with a warning) without OpenAI and Pushover credentials.
    pub fn create(ctx: &AppContext, side_effects: SideEffects) -> Option<Self> {
        let config = &ctx.config;
        if !config.arr_recovery_enabled {
            return None;
        }
        let mut clients = Vec::new();
        for (kind, url, api_key) in [
            (ArrKind::Sonarr, &config.sonarr_url, &config.sonarr_api_key),
            (ArrKind::Radarr, &config.radarr_url, &config.radarr_api_key),
        ] {
            let (Some(url), Some(api_key)) = (url, api_key) else {
                continue;
            };
            match HttpArrClient::new(ArrClientConfig {
                kind,
                url: url.clone(),
                api_key: api_key.clone(),
                http: ctx.http.clone(),
                side_effects: side_effects.clone(),
                local_files: config.arr_recovery_local_files,
            }) {
                Ok(client) => clients.push(client),
                Err(error) => {
                    tracing::warn!(target: LOG, error = %error, "Arr recovery: skipping {kind}");
                }
            }
        }
        if clients.is_empty() {
            return None;
        }
        if config.openai_api_key.is_none()
            || config.pushover_user.is_none()
            || config.pushover_token(PushoverChannel::Recs).is_none()
        {
            tracing::warn!(
                target: LOG,
                "Arr recovery disabled: requires OpenAI and Pushover credentials"
            );
            return None;
        }
        let tz = crate::time_zone(config);
        let schedule = match CronSchedule::parse(SCHEDULE, &tz) {
            Ok(schedule) => schedule,
            Err(error) => {
                tracing::error!(target: LOG, error = %error, "Arr recovery schedule is invalid");
                return None;
            }
        };
        Some(Self {
            schedule,
            clients,
            store: ctx.store.clone(),
            clock: ctx.clock.clone(),
            assessor: LunaAssessor::new(ctx.ai.clone(), ctx.config.clone()),
            notifier: PushoverNotifier::new(ctx.pushover.clone()),
            nzbget: config
                .nzbget_url
                .as_ref()
                .map(|url| NzbGetClient::new(ctx.http.clone(), url.clone())),
            last_summary: Mutex::new(None),
        })
    }

    fn set_summary(&self, summary: Option<String>) {
        *self
            .last_summary
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = summary;
    }
}

impl Task for ArrRecoveryTask {
    fn name(&self) -> &str {
        NAME
    }

    fn display_name(&self) -> Option<&str> {
        Some(DISPLAY_NAME)
    }

    fn schedule(&self) -> &CronSchedule {
        &self.schedule
    }

    fn options(&self) -> TaskOptions {
        TaskOptions {
            run_on_startup: true,
            ..TaskOptions::default()
        }
    }

    fn run<'a>(&'a self, _cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(async move {
            self.set_summary(None);
            let cx = RecoveryContext {
                store: &self.store,
                clock: &self.clock,
                assessor: &self.assessor,
                notifier: &self.notifier,
                health: self.nzbget.as_ref().map(|n| n as &dyn HealthSource),
                import_settle_delay: IMPORT_SETTLE_DELAY,
            };
            let summary = run_recovery(&self.clients, &cx)
                .await
                .map_err(TaskError::from_error)?;
            self.set_summary(Some(summary));
            Ok(())
        })
    }

    fn last_run_summary(&self) -> Option<String> {
        self.last_summary
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}
