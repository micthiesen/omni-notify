//! A web-researching agent that pushes at most a few
//! notifications per run.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::future::BoxFuture;
use jiff::tz::TimeZone;
use omni_ai::{
    Ai, AiTool, CostTag, GenerateRequest, ModelRole, ReasoningEffort, StepRecord, ToolSet,
    ToolSpec, Usage,
};
use omni_alerts::{Pushover, PushoverChannel, PushoverMessage};
use omni_config::Config;
use omni_core::clock::SharedClock;
use omni_store::Store;
use omni_tasks::{CronSchedule, RunContext, Task, TaskError, TaskOptions};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::configs::BriefingConfig;
use crate::format::log_timestamp;
use crate::logfile::{LogFile, code_block};
use crate::persistence::{
    BriefingNotificationData, add_notification, complete_delivery, distribute_run_cost,
    release_delivery, reserve_delivery,
};
use crate::placeholders::resolve_all_placeholders;

const LOG: &str = "Briefings";
/// Tool-loop step limit per run.
pub const MAX_STEPS: u32 = 20;

/// Delivers a briefing push (a seam so tests can fail or observe deliveries).
pub trait BriefingNotifier: Send + Sync {
    fn send<'a>(&'a self, message: PushoverMessage) -> BoxFuture<'a, Result<(), String>>;
}

/// Pushover on the `PUSHOVER_BRIEFING_TOKEN` channel (honors `SideEffectMode::Record`).
pub struct PushoverBriefingNotifier(pub Pushover);

impl BriefingNotifier for PushoverBriefingNotifier {
    fn send<'a>(&'a self, message: PushoverMessage) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.0
                .send(PushoverChannel::Briefing, message)
                .await
                .map(drop)
                .map_err(|e| e.to_string())
        })
    }
}

/// Everything a briefing run needs.
#[derive(Clone)]
pub struct BriefingDeps {
    pub store: Store,
    pub clock: SharedClock,
    pub ai: Ai,
    pub config: Arc<Config>,
    pub tz: TimeZone,
    pub notifier: Arc<dyn BriefingNotifier>,
    pub web_search: Arc<dyn AiTool>,
    pub fetch_url: Arc<dyn AiTool>,
    /// `LOGS_PATH`; markdown run logs go to `<logs>/briefings/`.
    pub logs_path: Option<PathBuf>,
}

/// One scheduled briefing.
pub struct BriefingTask {
    config: BriefingConfig,
    deps: BriefingDeps,
}

impl BriefingTask {
    /// `BriefingAgentTask.create`: `None` (logged) without `TAVILY_API_KEY`.
    pub fn create(config: BriefingConfig, deps: BriefingDeps) -> Option<Self> {
        if deps
            .config
            .tavily_api_key
            .as_deref()
            .is_none_or(str::is_empty)
        {
            tracing::info!(target: LOG, "{} disabled: missing TAVILY_API_KEY", config.name);
            return None;
        }
        Some(Self { config, deps })
    }

    async fn execute(&self, run_id: &str) -> Result<(), TaskError> {
        let deps = &self.deps;
        let name = self.config.name.as_str();
        let now = deps.clock.now_ms();
        let log_file = match &deps.logs_path {
            Some(logs) => Some(
                LogFile::make(
                    logs.join("briefings")
                        .join(format!("{name}-{}.md", log_timestamp(now, &deps.tz))),
                )
                .await
                .map_err(|e| TaskError::new(format!("create briefing log file failed: {e}")))?,
            ),
            None => None,
        };

        let prompt =
            resolve_all_placeholders(&deps.store, &self.config.prompt, name, now, &deps.tz)
                .await
                .map_err(TaskError::from_error)?;
        let model = deps
            .ai
            .model_for(&deps.config, ModelRole::Briefing)
            .map_err(TaskError::from_error)?;
        let model_id = model.id().to_string();

        match &log_file {
            Some(file) => {
                write_section(
                    file,
                    &format!("Briefing Prompt ({model_id})"),
                    &code_block(&prompt, None),
                )
                .await;
                tracing::info!(
                    target: LOG,
                    "Starting briefing agent ({model_id}) [{} chars]",
                    omni_core::js::utf16_len(&prompt)
                );
            }
            None => {
                tracing::info!(target: LOG, "Starting briefing agent ({model_id}) with prompt:\n{prompt}");
            }
        }

        let sent = Arc::new(AtomicBool::new(false));
        let tools = ToolSet::new()
            .with(deps.web_search.clone())
            .with(deps.fetch_url.clone())
            .with(Arc::new(SendNotification {
                briefing_name: name.to_owned(),
                run_id: run_id.to_owned(),
                store: deps.store.clone(),
                clock: deps.clock.clone(),
                notifier: deps.notifier.clone(),
                sent: sent.clone(),
            }));
        let request = GenerateRequest {
            reasoning_effort: Some(ReasoningEffort::High),
            ..GenerateRequest::prompt(prompt)
        };

        let (sections, mut receiver) = mpsc::unbounded_channel::<(String, String)>();
        let agent = async {
            let mut on_step = move |step: &StepRecord| report_step(step, &sections);
            deps.ai
                .run_tool_loop(
                    model.as_ref(),
                    request,
                    &tools,
                    MAX_STEPS,
                    CostTag::for_role(ModelRole::Briefing),
                    &mut on_step,
                )
                .await
        };
        let writer = async {
            while let Some((heading, content)) = receiver.recv().await {
                if let Some(file) = &log_file {
                    write_section(file, &heading, &content).await;
                }
            }
        };
        let (outcome, ()) = tokio::join!(agent, writer);
        let outcome =
            outcome.map_err(|e| TaskError::new(format!("generate briefing failed: {e}")))?;

        match &log_file {
            Some(file) => {
                write_section(
                    file,
                    "Result",
                    &format!("Completed in {} steps", outcome.steps),
                )
                .await;
                tracing::info!(target: LOG, "Completed in {} steps", outcome.steps);
            }
            None => tracing::info!(target: LOG, "Agent completed in {} steps", outcome.steps),
        }

        // Total usage is only known now, but the notifications were stored
        // earlier inside the tool, so the cost is backfilled onto their rows.
        if sent.load(Ordering::SeqCst) {
            let usage = Usage {
                input_tokens: outcome.usage.input_tokens,
                output_tokens: outcome.usage.output_tokens,
                ..Usage::default()
            };
            let cents = omni_ai::costs::llm_cost_cents(&model.id().model, &usage);
            if cents.is_none() {
                tracing::debug!(target: LOG, "No pricing data for model {model_id}; cost not recorded");
            }
            distribute_run_cost(&deps.store, name, Some(run_id), cents)
                .await
                .map_err(TaskError::from_error)?;
        }
        Ok(())
    }
}

async fn write_section(file: &LogFile, heading: &str, content: &str) {
    if let Err(error) = file.section(heading, content).await {
        tracing::warn!(
            target: LOG,
            "Writing briefing log {} failed: {error}",
            file.path().display()
        );
    }
}

/// Console lines plus log-file sections.
fn report_step(step: &StepRecord, sections: &mpsc::UnboundedSender<(String, String)>) {
    let section = |heading: String, content: String| {
        // The writer only stops after the agent future (which owns this sender) ends.
        let _ = sections.send((heading, content));
    };
    if let Some(reasoning) = step.reasoning.as_deref().filter(|r| !r.is_empty()) {
        section("Reasoning".to_owned(), code_block(reasoning, None));
    }
    if !step.text.is_empty() {
        section("Step Text".to_owned(), step.text.clone());
        tracing::debug!(target: LOG, "Step text: {}", step.text);
    }
    for call in &step.tool_calls {
        match call.name.as_str() {
            "web_search" => {
                let query = call.arguments.get("query").map_or_else(
                    || "undefined".to_owned(),
                    |q| q.as_str().map_or_else(|| q.to_string(), str::to_owned),
                );
                tracing::info!(target: LOG, "Search: \"{query}\"");
            }
            "fetch_url" => {
                let url = call.arguments.get("url").map_or_else(
                    || "undefined".to_owned(),
                    |u| u.as_str().map_or_else(|| u.to_string(), str::to_owned),
                );
                tracing::info!(target: LOG, "Fetching: {url}");
            }
            "send_notification" => {}
            other => tracing::info!(target: LOG, input = %call.arguments, "Tool call: {other}"),
        }
        section(
            format!("Tool Call: {}", call.name),
            code_block(
                &omni_core::js::json_stringify_pretty2(&call.arguments),
                Some("json"),
            ),
        );
    }
    for (name, result) in &step.tool_results {
        let Ok(output) = result else { continue };
        match name.as_str() {
            "web_search" => {
                let count = output
                    .get("results")
                    .and_then(Value::as_array)
                    .map_or_else(|| "?".to_owned(), |r| r.len().to_string());
                let time = output
                    .get("responseTime")
                    .and_then(Value::as_f64)
                    .map(|t| format!(" ({t:.1}s)"))
                    .unwrap_or_default();
                tracing::debug!(target: LOG, "Search returned {count} results{time}");
            }
            "fetch_url" => {
                let chars = output.get("content").and_then(Value::as_str).map_or_else(
                    || "?".to_owned(),
                    |c| omni_core::js::utf16_len(c).to_string(),
                );
                let truncated = if output.get("truncated").and_then(Value::as_bool) == Some(true) {
                    " (truncated)"
                } else {
                    ""
                };
                tracing::debug!(target: LOG, "Fetched {chars} chars{truncated}");
            }
            "send_notification" => {}
            other => tracing::info!(target: LOG, output = %output, "Tool result: {other}"),
        }
        let pretty = omni_core::js::json_stringify_pretty2(output);
        section(
            format!("Tool Result: {name}"),
            code_block(&omni_core::js::utf16_slice(&pretty, 0, 5_000), Some("json")),
        );
    }
}

impl Task for BriefingTask {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn schedule(&self) -> &CronSchedule {
        &self.config.schedule
    }

    fn options(&self) -> TaskOptions {
        TaskOptions::default()
    }

    fn run<'a>(&'a self, cx: &'a RunContext) -> BoxFuture<'a, Result<(), TaskError>> {
        Box::pin(self.execute(&cx.run_id))
    }
}

#[derive(Deserialize)]
struct SendNotificationArgs {
    title: String,
    message: String,
    url: String,
    url_title: String,
}

/// The `send_notification` tool: reserve, push, then record.
struct SendNotification {
    briefing_name: String,
    run_id: String,
    store: Store,
    clock: SharedClock,
    notifier: Arc<dyn BriefingNotifier>,
    sent: Arc<AtomicBool>,
}

/// `${runId}:${sha256(JSON.stringify({title, message, url})).slice(0, 24)}`.
pub fn delivery_id(run_id: &str, title: &str, message: &str, url: &str) -> String {
    let content = omni_core::js::json_stringify(&json!({
        "title": title,
        "message": message,
        "url": url,
    }));
    let hash = omni_core::digest::sha256_hex(content.as_bytes());
    format!("{run_id}:{}", &hash[..24])
}

impl SendNotification {
    async fn deliver(&self, args: SendNotificationArgs) -> Result<Value, String> {
        let name = self.briefing_name.as_str();
        let delivery_id = delivery_id(&self.run_id, &args.title, &args.message, &args.url);
        let reserved = reserve_delivery(&self.store, name, &delivery_id)
            .await
            .map_err(|e| e.to_string())?;
        if !reserved {
            return Ok(json!({ "success": true, "duplicate": true }));
        }
        tracing::info!(target: LOG, "Sending notification: {}", args.title);
        let message = PushoverMessage {
            message: args.message.clone(),
            title: Some(args.title.clone()),
            url: Some(args.url.clone()),
            url_title: Some(args.url_title),
            ..PushoverMessage::default()
        };
        if let Err(error) = self.notifier.send(message).await {
            release_delivery(&self.store, name, &delivery_id)
                .await
                .map_err(|e| e.to_string())?;
            return Err(error);
        }
        // Delivered is recorded before the archive: if the process dies between
        // these local writes, a retry is suppressed rather than duplicated.
        complete_delivery(&self.store, name, &delivery_id)
            .await
            .map_err(|e| e.to_string())?;
        let mut notification =
            BriefingNotificationData::new(args.title, args.message, args.url, self.clock.now_ms());
        notification.run_id = Some(self.run_id.clone());
        add_notification(&self.store, name, notification)
            .await
            .map_err(|e| e.to_string())?;
        self.sent.store(true, Ordering::SeqCst);
        Ok(json!({ "success": true }))
    }
}

impl AiTool for SendNotification {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "send_notification".to_owned(),
            description: "Send a push notification to the user with your briefing. Call this once you have something interesting to share.".to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Short title for the notification, prefixed with a relevant emoji (e.g. '🌸 Cherry Blossom Festival in Vancouver')"
                    },
                    "message": {
                        "type": "string",
                        "description": "The notification body with your summary"
                    },
                    "url": {
                        "type": "string",
                        "format": "uri",
                        "description": "URL to the source"
                    },
                    "url_title": {
                        "type": "string",
                        "description": "Link text for the URL (e.g. 'Read more')"
                    }
                },
                "required": ["title", "message", "url", "url_title"],
                "additionalProperties": false
            }),
        }
    }

    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            let args: SendNotificationArgs =
                serde_json::from_value(args).map_err(|e| format!("Invalid input: {e}"))?;
            url::Url::parse(&args.url).map_err(|_| "Invalid input: url: Invalid URL".to_owned())?;
            self.deliver(args).await
        })
    }
}
