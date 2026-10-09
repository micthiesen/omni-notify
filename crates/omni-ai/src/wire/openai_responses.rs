//! OpenAI Responses API (`POST /v1/responses`), mirroring `@ai-sdk/openai` 4.x defaults:
//! reasoning models (o-series, `gpt-5`+) get the system prompt as a `developer`
//! message and `reasoning.effort`; structured output is
//! `text.format = {type: "json_schema", strict: true, name, schema}`; `store` is left at
//! the API default (true), so earlier reasoning and message items are replayed as
//! `item_reference`s.

use serde_json::{Map, Value, json};

use super::{data_url, decode_json, join_nonempty, parse_arguments, tool_output_text, u64_at};
use crate::{
    AiError, ContentPart, FinishReason, GenerateRequest, GenerateResponse, Message, Provider, Role,
    ToolCall, Usage,
};

/// `isReasoningModel`: o-series, or `gpt-<major>` with major >= 5 that is not a chat model.
pub fn is_reasoning_model(model: &str) -> bool {
    let bytes = model.as_bytes();
    if bytes.first() == Some(&b'o') && bytes.get(1).is_some_and(u8::is_ascii_digit) {
        return true;
    }
    let Some(rest) = model.strip_prefix("gpt-") else {
        return false;
    };
    let major: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let is_chat = model.contains("-chat");
    major.parse::<u32>().is_ok_and(|m| m >= 5) && !is_chat
}

pub fn request_body(model: &str, req: &GenerateRequest) -> Value {
    let reasoning = is_reasoning_model(model);
    let mut input: Vec<Value> = Vec::new();
    if let Some(system) = &req.system {
        input.push(json!({
            "role": if reasoning { "developer" } else { "system" },
            "content": system,
        }));
    }
    for message in &req.messages {
        encode_message(message, &mut input);
    }
    let mut body = Map::new();
    body.insert("model".to_owned(), json!(model));
    body.insert("input".to_owned(), Value::Array(input));
    if let Some(max) = req.max_output_tokens {
        body.insert("max_output_tokens".to_owned(), json!(max));
    }
    if let Some(output) = &req.output {
        body.insert(
            "text".to_owned(),
            json!({ "format": {
                "type": "json_schema",
                "strict": true,
                "name": output.name,
                "schema": output.schema,
            }}),
        );
    }
    if reasoning && let Some(effort) = req.reasoning_effort {
        body.insert("reasoning".to_owned(), json!({ "effort": effort.as_str() }));
    }
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|tool| {
                let mut spec = json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                });
                // Optional arguments are not valid in strict mode; say so explicitly.
                if !crate::schema::is_strict_compatible(&tool.parameters)
                    && let Value::Object(map) = &mut spec
                {
                    map.insert("strict".to_owned(), Value::Bool(false));
                }
                spec
            })
            .collect();
        body.insert("tools".to_owned(), Value::Array(tools));
        body.insert("tool_choice".to_owned(), json!("auto"));
    }
    Value::Object(body)
}

fn encode_message(message: &Message, input: &mut Vec<Value>) {
    match message.role {
        Role::User => {
            let content: Vec<Value> = message
                .content
                .iter()
                .enumerate()
                .filter_map(|(index, part)| match part {
                    ContentPart::Text { text } => Some(json!({"type": "input_text", "text": text})),
                    ContentPart::Image {
                        mime_type,
                        data_b64,
                    } => Some(json!({
                        "type": "input_image",
                        "image_url": data_url(mime_type, data_b64),
                    })),
                    ContentPart::File {
                        mime_type,
                        data_b64,
                        filename,
                    } => Some(if mime_type.starts_with("image/") {
                        json!({"type": "input_image", "image_url": data_url(mime_type, data_b64)})
                    } else {
                        let filename = filename.clone().unwrap_or_else(|| {
                            if mime_type == "application/pdf" {
                                format!("part-{index}.pdf")
                            } else {
                                format!("part-{index}")
                            }
                        });
                        json!({
                            "type": "input_file",
                            "filename": filename,
                            "file_data": data_url(mime_type, data_b64),
                        })
                    }),
                    _ => None,
                })
                .collect();
            input.push(json!({"role": "user", "content": content}));
        }
        Role::Assistant => {
            let replay = message.content.iter().find_map(|part| match part {
                ContentPart::ProviderItems {
                    provider: Provider::OpenAi,
                    items,
                } => Some(items),
                _ => None,
            });
            if let Some(items) = replay {
                for item in items {
                    input.push(replay_item(item));
                }
                return;
            }
            for part in &message.content {
                match part {
                    ContentPart::Text { text } => input.push(json!({
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": text}],
                    })),
                    ContentPart::ToolCall(call) => input.push(json!({
                        "type": "function_call",
                        "call_id": call.call_id,
                        "name": call.name,
                        "arguments": arguments_text(&call.arguments),
                    })),
                    _ => {}
                }
            }
        }
        Role::Tool => {
            for part in &message.content {
                if let ContentPart::ToolResult {
                    call_id, output, ..
                } = part
                {
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": tool_output_text(output),
                    }));
                }
            }
        }
    }
}

/// Stored reasoning and message items are referenced by id; function calls are sent in
/// full (their `call_id` pairs them with the outputs that follow).
fn replay_item(item: &Value) -> Value {
    let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
    match (kind, item.get("id").and_then(Value::as_str)) {
        ("function_call", _) => {
            let mut call = Map::new();
            for key in ["type", "id", "call_id", "name", "arguments"] {
                if let Some(value) = item.get(key) {
                    call.insert(key.to_owned(), value.clone());
                }
            }
            Value::Object(call)
        }
        (_, Some(id)) => json!({"type": "item_reference", "id": id}),
        _ => item.clone(),
    }
}

fn arguments_text(arguments: &Value) -> String {
    match arguments {
        Value::String(raw) => raw.clone(),
        other => omni_core::js::json_stringify(other),
    }
}

pub fn parse_response(body: &[u8]) -> Result<GenerateResponse, AiError> {
    let value = decode_json(body)?;
    if let Some(message) = value.pointer("/error/message").and_then(Value::as_str) {
        return Err(AiError::Provider {
            status: 200,
            message: message.to_owned(),
        });
    }
    let items = value
        .get("output")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut text = Vec::new();
    let mut reasoning = Vec::new();
    let mut refusals = Vec::new();
    let mut tool_calls = Vec::new();
    for item in &items {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                for part in item
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    match part.get("type").and_then(Value::as_str) {
                        Some("output_text") => {
                            if let Some(t) = part.get("text").and_then(Value::as_str) {
                                text.push(t.to_owned());
                            }
                        }
                        Some("refusal") => {
                            if let Some(t) = part.get("refusal").and_then(Value::as_str) {
                                refusals.push(t.to_owned());
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("reasoning") => {
                for summary in item
                    .get("summary")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(t) = summary.get("text").and_then(Value::as_str) {
                        reasoning.push(t.to_owned());
                    }
                }
            }
            Some("function_call") => {
                let raw = item.get("arguments").and_then(Value::as_str).unwrap_or("");
                tool_calls.push(ToolCall {
                    call_id: item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    arguments: parse_arguments(raw),
                });
            }
            _ => {}
        }
    }
    let text = text.concat();
    if text.is_empty() && tool_calls.is_empty() && !refusals.is_empty() {
        return Err(AiError::Refused(refusals.join("\n")));
    }
    let incomplete = value
        .pointer("/incomplete_details/reason")
        .and_then(Value::as_str);
    let finish = match incomplete {
        Some("max_output_tokens") => FinishReason::Length,
        Some("content_filter") => FinishReason::ContentFilter,
        Some(_) => FinishReason::Other,
        None if !tool_calls.is_empty() => FinishReason::ToolCalls,
        None => FinishReason::Stop,
    };
    let usage = value.get("usage").map(parse_usage).unwrap_or_default();
    Ok(GenerateResponse {
        text,
        tool_calls,
        usage,
        finish,
        reasoning: join_nonempty(reasoning),
        provider_items: items,
    })
}

/// `convertOpenAIResponsesUsage`.
pub fn parse_usage(usage: &Value) -> Usage {
    let input = u64_at(usage, "/input_tokens");
    let cached = u64_at(usage, "/input_tokens_details/cached_tokens");
    let cache_write = u64_at(usage, "/input_tokens_details/cache_write_tokens");
    Usage {
        input_tokens: input,
        input_no_cache_tokens: input.saturating_sub(cached + cache_write),
        cache_read_tokens: cached,
        cache_write_tokens: cache_write,
        output_tokens: u64_at(usage, "/output_tokens"),
        reasoning_tokens: u64_at(usage, "/output_tokens_details/reasoning_tokens"),
    }
}
