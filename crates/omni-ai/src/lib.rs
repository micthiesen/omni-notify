//! Language models, tools and cost accounting (ARCHITECTURE.md section 3.8).
//!
//! Ports `src/ai/registry.ts` (model resolution, the five-minute per-call timeout and
//! per-call cost recording), the AI SDK `generateText` behaviour the TS code relies on
//! (structured output, retries, the multi-step tool loop) and three thin provider wire
//! clients: [`wire::openai_responses`], [`wire::anthropic_messages`] and [`wire::gemini`].
//!
//! Cost events are recorded by the [`Ai`] helpers once per successful provider call
//! (`requests: 1`), exactly where the TS `wrapGenerate` middleware recorded them. Calling
//! [`LanguageModel::generate`] directly records nothing.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use omni_api::costs::{CostCategory, CostPriceStatus, CostUsage};
use omni_config::Config;
use omni_http::{HttpClient, HttpError};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub use omni_config::ModelRole;

pub mod costs;
pub mod registry;
pub mod schema;
pub mod tools;
pub mod wire;

use costs::{CostRecorder, NewCostEvent};

const LOG: &str = "AiRegistry";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[serde(rename = "openai")]
    OpenAi,
    Anthropic,
    Google,
}

impl Provider {
    /// The registry prefix (`openai`, `anthropic`, `google`), also the cost `service`.
    pub fn prefix(self) -> &'static str {
        match self {
            Provider::OpenAi => "openai",
            Provider::Anthropic => "anthropic",
            Provider::Google => "google",
        }
    }
}

/// `"<provider>:<model>"`, e.g. `openai:gpt-6-luna`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ModelId {
    pub provider: Provider,
    pub model: String,
}

impl ModelId {
    pub fn parse(s: &str) -> Result<Self, AiError> {
        let (prefix, model) = s
            .split_once(':')
            .ok_or_else(|| AiError::Schema(format!("model id {s:?} must be provider:model")))?;
        let provider = match prefix {
            "openai" => Provider::OpenAi,
            "anthropic" => Provider::Anthropic,
            "google" => Provider::Google,
            other => return Err(AiError::Schema(format!("unknown model provider {other:?}"))),
        };
        if model.is_empty() {
            return Err(AiError::Schema(format!("model id {s:?} has no model name")));
        }
        Ok(Self {
            provider,
            model: model.to_owned(),
        })
    }
}

impl std::fmt::Display for ModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.provider.prefix(), self.model)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ContentPart {
    Text {
        text: String,
    },
    /// An inline image (`image/*`), base64 without a data-URL prefix.
    Image {
        mime_type: String,
        data_b64: String,
    },
    /// An inline file (AI SDK `type: "file"`); image media types are sent as images.
    File {
        mime_type: String,
        data_b64: String,
        filename: Option<String>,
    },
    ToolCall(ToolCall),
    ToolResult {
        call_id: String,
        /// The tool name (Gemini's `functionResponse` is keyed by name).
        name: String,
        output: serde_json::Value,
        is_error: bool,
    },
    /// Opaque provider output items replayed verbatim to the same provider on the next
    /// step (OpenAI reasoning/message ids, Anthropic thinking blocks with signatures,
    /// Gemini thought signatures). Other providers ignore them and use the semantic
    /// parts of the same message instead.
    ProviderItems {
        provider: Provider,
        items: Vec<serde_json::Value>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentPart>,
}

impl Message {
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentPart::Text { text: text.into() }],
        }
    }
}

/// A function tool offered to the model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON schema of the arguments.
    pub parameters: serde_json::Value,
}

/// Structured output: a name plus a strict JSON schema.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputSpec {
    pub name: String,
    pub schema: serde_json::Value,
}

impl OutputSpec {
    /// The AI SDK `Output.object` default: name `response`, the strict schema of `T`.
    pub fn of<T: schemars::JsonSchema>() -> Self {
        Self {
            name: "response".to_owned(),
            schema: schema::strict_schema::<T>(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            ReasoningEffort::Minimal => "minimal",
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GenerateRequest {
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub output: Option<OutputSpec>,
    pub max_output_tokens: Option<u32>,
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Retries of retryable provider failures (AI SDK `maxRetries`). Default 2.
    pub max_retries: u32,
    /// Per provider call (`LANGUAGE_MODEL_TIMEOUT`). Default 5 minutes.
    pub timeout: Duration,
}

impl Default for GenerateRequest {
    fn default() -> Self {
        Self {
            system: None,
            messages: Vec::new(),
            tools: Vec::new(),
            output: None,
            max_output_tokens: None,
            reasoning_effort: None,
            max_retries: 2,
            timeout: registry::LANGUAGE_MODEL_TIMEOUT,
        }
    }
}

impl GenerateRequest {
    /// A request with one user text message (AI SDK `prompt`).
    pub fn prompt(text: impl Into<String>) -> Self {
        Self {
            messages: vec![Message::user_text(text)],
            ..Self::default()
        }
    }
}

/// AI SDK `LanguageModelUsage` flattened: totals include cached and reasoning tokens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub input_no_cache_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, other: Usage) {
        self.input_tokens += other.input_tokens;
        self.input_no_cache_tokens += other.input_no_cache_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
        self.output_tokens += other.output_tokens;
        self.reasoning_tokens += other.reasoning_tokens;
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    /// Parsed arguments; a string when the model produced invalid JSON.
    pub arguments: serde_json::Value,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FinishReason {
    #[default]
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    Other,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GenerateResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
    pub finish: FinishReason,
    /// Reasoning text or summaries, when the provider returned any (`reasoningText`).
    pub reasoning: Option<String>,
    /// Raw provider output items, replayed on the next tool-loop step.
    pub provider_items: Vec<serde_json::Value>,
}

impl GenerateResponse {
    /// A plain text response (fakes and tests).
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    /// A response that calls tools (fakes and tests).
    pub fn tool_calls(calls: Vec<ToolCall>) -> Self {
        Self {
            tool_calls: calls,
            finish: FinishReason::ToolCalls,
            ..Self::default()
        }
    }
}

/// One provider model.
pub trait LanguageModel: Send + Sync {
    fn id(&self) -> &ModelId;
    /// The role this model was resolved for, which supplies the cost-feature fallback.
    fn role(&self) -> Option<ModelRole> {
        None
    }
    fn generate<'a>(
        &'a self,
        req: &'a GenerateRequest,
    ) -> BoxFuture<'a, Result<GenerateResponse, AiError>>;
}

/// Cost attribution for a generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CostTag {
    /// Static feature; `None` uses the model role's feature. Either way the current
    /// run's task name takes precedence ([`costs::current_cost_feature`]), as in TS.
    pub feature: Option<&'static str>,
    pub operation: &'static str,
}

impl CostTag {
    /// The `src/ai/registry.ts` feature and default operation of `role`.
    pub fn for_role(role: ModelRole) -> Self {
        let (feature, operation) = registry::role_cost(role);
        Self {
            feature: Some(feature),
            operation,
        }
    }

    /// `role`'s feature with a caller-specific operation (`getWorkspaceModel(operation)`).
    pub fn with_operation(role: ModelRole, operation: &'static str) -> Self {
        Self {
            operation,
            ..Self::for_role(role)
        }
    }
}

/// A tool the model may call in [`Ai::run_tool_loop`].
pub trait AiTool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    /// The tool's output, or an error message the model sees as the tool result.
    fn call<'a>(
        &'a self,
        args: serde_json::Value,
    ) -> BoxFuture<'a, Result<serde_json::Value, String>>;
}

/// Tools by name.
#[derive(Clone, Default)]
pub struct ToolSet {
    tools: BTreeMap<String, Arc<dyn AiTool>>,
}

impl ToolSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a tool; a later tool with the same name replaces the earlier one.
    pub fn with(mut self, tool: Arc<dyn AiTool>) -> Self {
        self.tools.insert(tool.spec().name, tool);
        self
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn AiTool>> {
        self.tools.get(name)
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|tool| tool.spec()).collect()
    }
}

/// One step of a tool loop, reported to `on_step` after its tools ran.
#[derive(Clone, Debug, PartialEq)]
pub struct StepRecord {
    pub index: u32,
    pub text: String,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub tool_results: Vec<(String, Result<serde_json::Value, String>)>,
    pub usage: Usage,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoopResult {
    /// The last step's text (AI SDK `result.text`).
    pub text: String,
    pub steps: u32,
    /// Summed over all steps (AI SDK `result.usage`).
    pub usage: Usage,
    /// The full conversation including every assistant and tool message.
    pub messages: Vec<Message>,
    /// The loop ended because `max_steps` was reached while the model was still calling
    /// tools (AI SDK `stopWhen: isStepCount(n)`); no final answer exists.
    pub stopped_at_step_limit: bool,
}

impl LoopResult {
    /// The structured output of the final step (`result.output`); `StepLimit` when the
    /// loop ran out of steps before the model answered.
    pub fn object<T: DeserializeOwned>(&self) -> Result<T, AiError> {
        if self.stopped_at_step_limit {
            return Err(AiError::StepLimit);
        }
        schema::parse_object(&self.text)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum AiError {
    #[error("language model timed out")]
    Timeout,
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error("provider error {status}: {message}")]
    Provider { status: u16, message: String },
    #[error("schema: {0}")]
    Schema(String),
    #[error("model refused: {0}")]
    Refused(String),
    #[error("tool loop step limit reached")]
    StepLimit,
    #[error("missing API key for {0:?}")]
    MissingKey(Provider),
}

impl AiError {
    /// AI SDK `APICallError.isRetryable`: 408, 409, 429, 5xx and network failures.
    /// The per-call timeout is not retried (TS wraps it in a non-retryable error).
    pub fn is_retryable(&self) -> bool {
        match self {
            AiError::Provider { status, .. } => {
                matches!(*status, 408 | 409 | 429) || *status >= 500
            }
            AiError::Http(error) => error.is_transient(),
            _ => false,
        }
    }
}

/// First retry delay (AI SDK `retryWithExponentialBackoff`: 2 s, doubling).
pub const RETRY_INITIAL_DELAY: Duration = Duration::from_secs(2);

/// Model factory and generation helpers; cheap to clone.
#[derive(Clone)]
pub struct Ai {
    inner: Arc<AiInner>,
}

struct AiInner {
    http: HttpClient,
    keys: BTreeMap<Provider, String>,
    costs: CostRecorder,
    overrides: Option<Arc<dyn ModelOverride>>,
}

/// Test seam: supplies scripted models instead of provider clients
/// (`omni_testkit::FakeModels`).
pub trait ModelOverride: Send + Sync {
    /// A replacement model for this role (when known) and id, or `None` to use the real client.
    fn model(&self, role: Option<ModelRole>, id: &ModelId) -> Option<Arc<dyn LanguageModel>>;
}

impl Ai {
    pub fn new(http: HttpClient, config: &Config, costs: CostRecorder) -> Self {
        let mut keys = BTreeMap::new();
        for (provider, key) in [
            (Provider::OpenAi, &config.openai_api_key),
            (Provider::Anthropic, &config.anthropic_api_key),
            (Provider::Google, &config.google_generative_ai_api_key),
        ] {
            if let Some(key) = key.as_ref().filter(|key| !key.is_empty()) {
                keys.insert(provider, key.clone());
            }
        }
        Self::with_keys(http, keys, costs)
    }

    /// A client with explicit provider keys (tests and tools).
    pub fn with_keys(
        http: HttpClient,
        keys: BTreeMap<Provider, String>,
        costs: CostRecorder,
    ) -> Self {
        Self {
            inner: Arc::new(AiInner {
                http,
                keys,
                costs,
                overrides: None,
            }),
        }
    }

    /// The same client with a model override installed (tests).
    pub fn with_override(self, overrides: Arc<dyn ModelOverride>) -> Self {
        let inner = &self.inner;
        Self {
            inner: Arc::new(AiInner {
                http: inner.http.clone(),
                keys: inner.keys.clone(),
                costs: inner.costs.clone(),
                overrides: Some(overrides),
            }),
        }
    }

    /// The cost recorder every generation reports to.
    pub fn costs(&self) -> &CostRecorder {
        &self.inner.costs
    }

    /// A wire client for `id`; fails with `MissingKey` when its provider has no key.
    pub fn model(&self, id: &ModelId) -> Result<Arc<dyn LanguageModel>, AiError> {
        self.resolve(None, id)
    }

    fn resolve(
        &self,
        role: Option<ModelRole>,
        id: &ModelId,
    ) -> Result<Arc<dyn LanguageModel>, AiError> {
        if let Some(model) = self
            .inner
            .overrides
            .as_ref()
            .and_then(|o| o.model(role, id))
        {
            return Ok(model);
        }
        let key = self
            .inner
            .keys
            .get(&id.provider)
            .ok_or(AiError::MissingKey(id.provider))?
            .clone();
        Ok(wire::client(self.inner.http.clone(), id.clone(), key, role))
    }

    /// The model configured for `role` (env override or code default).
    pub fn model_for(
        &self,
        cfg: &Config,
        role: ModelRole,
    ) -> Result<Arc<dyn LanguageModel>, AiError> {
        self.resolve(Some(role), &ModelId::parse(cfg.model(role))?)
    }

    /// Strict structured output (AI SDK `generateText` + `Output.object`); records cost.
    /// `req.output` defaults to [`OutputSpec::of::<T>`].
    pub async fn generate_object<T: DeserializeOwned + schemars::JsonSchema>(
        &self,
        m: &dyn LanguageModel,
        mut req: GenerateRequest,
        cost: CostTag,
    ) -> Result<(T, Usage), AiError> {
        if req.output.is_none() {
            req.output = Some(OutputSpec::of::<T>());
        }
        let response = self.step(m, &req, cost).await?;
        if response.finish == FinishReason::Length {
            return Err(AiError::Schema(
                "No object generated: the response was cut off at the output token limit"
                    .to_owned(),
            ));
        }
        let value = schema::parse_object(&response.text)?;
        Ok((value, response.usage))
    }

    /// Plain text generation; records cost.
    pub async fn generate_text(
        &self,
        m: &dyn LanguageModel,
        req: GenerateRequest,
        cost: CostTag,
    ) -> Result<(String, Usage), AiError> {
        let response = self.step(m, &req, cost).await?;
        Ok((response.text, response.usage))
    }

    /// One provider call with the full response (text, tool calls, reasoning); records cost.
    pub async fn generate(
        &self,
        m: &dyn LanguageModel,
        req: &GenerateRequest,
        cost: CostTag,
    ) -> Result<GenerateResponse, AiError> {
        self.step(m, req, cost).await
    }

    /// AI SDK `generateText({ tools, stopWhen: isStepCount(max_steps) })`: each step's
    /// tool calls run concurrently and their results (errors included, as error text the
    /// model sees) feed the next step, until a step makes no tool calls or `max_steps`
    /// steps have run. Tools offered are `tools`; `req.tools` is ignored.
    pub async fn run_tool_loop(
        &self,
        m: &dyn LanguageModel,
        mut req: GenerateRequest,
        tools: &ToolSet,
        max_steps: u32,
        cost: CostTag,
        on_step: &mut (dyn FnMut(&StepRecord) + Send),
    ) -> Result<LoopResult, AiError> {
        req.tools = tools.specs();
        let mut usage = Usage::default();
        let mut index = 0u32;
        loop {
            let response = self.step(m, &req, cost).await?;
            usage += response.usage;
            let assistant = assistant_message(m.id().provider, &response);
            if response.tool_calls.is_empty() {
                on_step(&StepRecord {
                    index,
                    text: response.text.clone(),
                    reasoning: response.reasoning.clone(),
                    tool_calls: Vec::new(),
                    tool_results: Vec::new(),
                    usage: response.usage,
                });
                req.messages.push(assistant);
                return Ok(LoopResult {
                    text: response.text,
                    steps: index + 1,
                    usage,
                    messages: req.messages,
                    stopped_at_step_limit: false,
                });
            }
            let results = futures::future::join_all(
                response.tool_calls.iter().map(|call| run_tool(tools, call)),
            )
            .await;
            on_step(&StepRecord {
                index,
                text: response.text.clone(),
                reasoning: response.reasoning.clone(),
                tool_calls: response.tool_calls.clone(),
                tool_results: response
                    .tool_calls
                    .iter()
                    .zip(&results)
                    .map(|(call, result)| (call.name.clone(), result.clone()))
                    .collect(),
                usage: response.usage,
            });
            req.messages.push(assistant);
            req.messages.push(Message {
                role: Role::Tool,
                content: response
                    .tool_calls
                    .iter()
                    .zip(results)
                    .map(|(call, result)| {
                        let (output, is_error) = match result {
                            Ok(output) => (output, false),
                            Err(message) => (serde_json::Value::String(message), true),
                        };
                        ContentPart::ToolResult {
                            call_id: call.call_id.clone(),
                            name: call.name.clone(),
                            output,
                            is_error,
                        }
                    })
                    .collect(),
            });
            index += 1;
            if index >= max_steps {
                return Ok(LoopResult {
                    text: response.text,
                    steps: index,
                    usage,
                    messages: req.messages,
                    stopped_at_step_limit: true,
                });
            }
        }
    }

    /// One provider call with timeout, retries and cost recording.
    async fn step(
        &self,
        m: &dyn LanguageModel,
        req: &GenerateRequest,
        cost: CostTag,
    ) -> Result<GenerateResponse, AiError> {
        let response = call_with_retries(m, req).await?;
        self.record_llm_cost(m, &response.usage, cost).await;
        Ok(response)
    }

    async fn record_llm_cost(&self, m: &dyn LanguageModel, usage: &Usage, cost: CostTag) {
        let id = m.id();
        let fallback = cost
            .feature
            .or_else(|| m.role().map(|role| registry::role_cost(role).0))
            .unwrap_or("unknown");
        let cents = costs::llm_cost_cents(&id.model, usage);
        #[allow(clippy::cast_precision_loss)]
        let count = |n: u64| Some(n as f64);
        self.inner
            .costs
            .record(NewCostEvent {
                category: CostCategory::Llm,
                feature: costs::current_cost_feature(fallback).to_owned(),
                operation: cost.operation.to_owned(),
                service: id.provider.prefix().to_owned(),
                model: Some(id.model.clone()),
                cost_cents: cents,
                price_status: if cents.is_some() {
                    CostPriceStatus::Estimated
                } else {
                    CostPriceStatus::Unknown
                },
                usage: CostUsage {
                    input_tokens: count(usage.input_tokens),
                    input_no_cache_tokens: count(usage.input_no_cache_tokens),
                    cache_read_tokens: count(usage.cache_read_tokens),
                    cache_write_tokens: count(usage.cache_write_tokens),
                    output_tokens: count(usage.output_tokens),
                    reasoning_tokens: count(usage.reasoning_tokens),
                    requests: Some(1.0),
                    ..CostUsage::default()
                },
                event_id: None,
                incurred_at: None,
                run_id: None,
            })
            .await;
    }
}

/// `callLanguageModelEffect` + AI SDK retries: every attempt is bounded by
/// `req.timeout` (dropping the request future aborts the HTTP call); retryable
/// failures are retried `req.max_retries` times with 2 s, 4 s, ... delays.
pub async fn call_with_retries(
    m: &dyn LanguageModel,
    req: &GenerateRequest,
) -> Result<GenerateResponse, AiError> {
    let mut attempt = 0u32;
    loop {
        let result = match tokio::time::timeout(req.timeout, m.generate(req)).await {
            Ok(result) => result,
            Err(_) => Err(AiError::Timeout),
        };
        match result {
            Err(error) if attempt < req.max_retries && error.is_retryable() => {
                let delay = RETRY_INITIAL_DELAY.saturating_mul(1 << attempt.min(16));
                tracing::warn!(
                    target: LOG,
                    model = %m.id(),
                    attempt = attempt + 1,
                    %error,
                    "Retrying language-model call"
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
            other => return other,
        }
    }
}

fn assistant_message(provider: Provider, response: &GenerateResponse) -> Message {
    let mut content = Vec::new();
    if !response.provider_items.is_empty() {
        content.push(ContentPart::ProviderItems {
            provider,
            items: response.provider_items.clone(),
        });
    }
    if !response.text.is_empty() {
        content.push(ContentPart::Text {
            text: response.text.clone(),
        });
    }
    content.extend(
        response
            .tool_calls
            .iter()
            .cloned()
            .map(ContentPart::ToolCall),
    );
    Message {
        role: Role::Assistant,
        content,
    }
}

async fn run_tool(tools: &ToolSet, call: &ToolCall) -> Result<serde_json::Value, String> {
    match tools.get(&call.name) {
        Some(tool) => tool.call(call.arguments.clone()).await,
        None => Err(format!(
            "Model tried to call unavailable tool '{}'",
            call.name
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_model_ids() {
        let id = ModelId::parse("openai:gpt-6-luna").ok();
        assert_eq!(id.as_ref().map(|i| i.provider), Some(Provider::OpenAi));
        assert_eq!(
            id.map(|i| i.to_string()).as_deref(),
            Some("openai:gpt-6-luna")
        );
        assert!(ModelId::parse("gpt-6-luna").is_err());
        assert!(ModelId::parse("mistral:x").is_err());
    }

    #[test]
    fn retry_classification_matches_ai_sdk() {
        let provider = |status| AiError::Provider {
            status,
            message: String::new(),
        };
        assert!(provider(429).is_retryable());
        assert!(provider(503).is_retryable());
        assert!(provider(408).is_retryable());
        assert!(!provider(400).is_retryable());
        assert!(!AiError::Timeout.is_retryable());
        assert!(AiError::Http(HttpError::Network("reset".to_owned())).is_retryable());
    }
}
