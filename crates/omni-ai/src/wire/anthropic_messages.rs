//! Anthropic Messages API (`POST /v1/messages`, `anthropic-version: 2023-06-01`).
//!
//! Structured output uses `output_config.format = {type: "json_schema", schema}` and the
//! reasoning effort `output_config.effort`; thinking is left at the model default.
//! Assistant content blocks (including signed `thinking` blocks) are replayed verbatim.

use serde_json::{Map, Value, json};

use super::{decode_json, join_nonempty, tool_output_text, u64_at};
use crate::{
    AiError, ContentPart, FinishReason, GenerateRequest, GenerateResponse, Message, Provider,
    ReasoningEffort, Role, ToolCall, Usage,
};

pub const API_VERSION: &str = "2023-06-01";

/// `max_tokens` is required; used when the request sets no limit (non-streaming).
pub const DEFAULT_MAX_TOKENS: u32 = 16_000;

pub fn request_body(model: &str, req: &GenerateRequest) -> Value {
    let mut body = Map::new();
    body.insert("model".to_owned(), json!(model));
    body.insert(
        "max_tokens".to_owned(),
        json!(req.max_output_tokens.unwrap_or(DEFAULT_MAX_TOKENS)),
    );
    if let Some(system) = &req.system {
        body.insert("system".to_owned(), json!(system));
    }
    let messages: Vec<Value> = req.messages.iter().map(encode_message).collect();
    body.insert("messages".to_owned(), Value::Array(messages));
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.parameters,
                })
            })
            .collect();
        body.insert("tools".to_owned(), Value::Array(tools));
    }
    let mut output_config = Map::new();
    if let Some(output) = &req.output {
        output_config.insert(
            "format".to_owned(),
            json!({"type": "json_schema", "schema": output.schema}),
        );
    }
    if let Some(effort) = req.reasoning_effort {
        let effort = match effort {
            ReasoningEffort::Minimal | ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
        };
        output_config.insert("effort".to_owned(), json!(effort));
    }
    if !output_config.is_empty() {
        body.insert("output_config".to_owned(), Value::Object(output_config));
    }
    Value::Object(body)
}

fn encode_message(message: &Message) -> Value {
    match message.role {
        Role::User => {
            let content: Vec<Value> = message
                .content
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text { text } => Some(json!({"type": "text", "text": text})),
                    ContentPart::Image {
                        mime_type,
                        data_b64,
                    } => Some(base64_block("image", mime_type, data_b64)),
                    ContentPart::File {
                        mime_type,
                        data_b64,
                        ..
                    } => Some(if mime_type.starts_with("image/") {
                        base64_block("image", mime_type, data_b64)
                    } else {
                        base64_block("document", mime_type, data_b64)
                    }),
                    _ => None,
                })
                .collect();
            json!({"role": "user", "content": content})
        }
        Role::Assistant => {
            let replay = message.content.iter().find_map(|part| match part {
                ContentPart::ProviderItems {
                    provider: Provider::Anthropic,
                    items,
                } => Some(items.clone()),
                _ => None,
            });
            let content = replay.unwrap_or_else(|| {
                message
                    .content
                    .iter()
                    .filter_map(|part| match part {
                        ContentPart::Text { text } => Some(json!({"type": "text", "text": text})),
                        ContentPart::ToolCall(call) => Some(json!({
                            "type": "tool_use",
                            "id": call.call_id,
                            "name": call.name,
                            "input": call.arguments,
                        })),
                        _ => None,
                    })
                    .collect()
            });
            json!({"role": "assistant", "content": content})
        }
        Role::Tool => {
            let content: Vec<Value> = message
                .content
                .iter()
                .filter_map(|part| match part {
                    ContentPart::ToolResult {
                        call_id,
                        output,
                        is_error,
                        ..
                    } => {
                        let mut block = json!({
                            "type": "tool_result",
                            "tool_use_id": call_id,
                            "content": tool_output_text(output),
                        });
                        if *is_error && let Value::Object(map) = &mut block {
                            map.insert("is_error".to_owned(), Value::Bool(true));
                        }
                        Some(block)
                    }
                    _ => None,
                })
                .collect();
            json!({"role": "user", "content": content})
        }
    }
}

fn base64_block(kind: &str, mime_type: &str, data_b64: &str) -> Value {
    json!({
        "type": kind,
        "source": {"type": "base64", "media_type": mime_type, "data": data_b64},
    })
}

pub fn parse_response(body: &[u8]) -> Result<GenerateResponse, AiError> {
    let value = decode_json(body)?;
    if value.get("type").and_then(Value::as_str) == Some("error") {
        let message = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(AiError::Provider {
            status: 200,
            message: message.to_owned(),
        });
    }
    let blocks = value
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut text = Vec::new();
    let mut reasoning = Vec::new();
    let mut tool_calls = Vec::new();
    for block in &blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = block.get("text").and_then(Value::as_str) {
                    text.push(t.to_owned());
                }
            }
            Some("thinking") => {
                if let Some(t) = block.get("thinking").and_then(Value::as_str) {
                    reasoning.push(t.to_owned());
                }
            }
            Some("tool_use") => tool_calls.push(ToolCall {
                call_id: block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                name: block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                arguments: block.get("input").cloned().unwrap_or(Value::Null),
            }),
            _ => {}
        }
    }
    let stop = value.get("stop_reason").and_then(Value::as_str);
    if stop == Some("refusal") {
        let explanation = value
            .pointer("/stop_details/explanation")
            .and_then(Value::as_str)
            .unwrap_or("the model declined the request");
        return Err(AiError::Refused(explanation.to_owned()));
    }
    let finish = match stop {
        Some("end_turn" | "stop_sequence") => FinishReason::Stop,
        Some("max_tokens") => FinishReason::Length,
        Some("tool_use") => FinishReason::ToolCalls,
        _ => FinishReason::Other,
    };
    let usage = value.get("usage").map(parse_usage).unwrap_or_default();
    Ok(GenerateResponse {
        text: text.concat(),
        tool_calls,
        usage,
        finish,
        reasoning: join_nonempty(reasoning),
        provider_items: blocks,
    })
}

/// The input total includes cache reads and writes.
pub fn parse_usage(usage: &Value) -> Usage {
    let input = u64_at(usage, "/input_tokens");
    let cache_write = u64_at(usage, "/cache_creation_input_tokens");
    let cache_read = u64_at(usage, "/cache_read_input_tokens");
    Usage {
        input_tokens: input + cache_write + cache_read,
        input_no_cache_tokens: input,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        output_tokens: u64_at(usage, "/output_tokens"),
        reasoning_tokens: u64_at(usage, "/output_tokens_details/thinking_tokens"),
    }
}
