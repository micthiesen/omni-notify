//! Gemini `models/{model}:generateContent` (used only when a `google:` model is
//! configured). Structured output uses `responseMimeType: application/json` with
//! `responseJsonSchema`; tools use `parametersJsonSchema`; the reasoning effort maps to
//! `thinkingConfig.thinkingLevel` (the TS briefing agent's `thinkingLevel: "high"`).
//! Model turns are replayed verbatim so thought signatures survive tool steps.

use serde_json::{Map, Value, json};

use super::{join_nonempty, u64_at};
use crate::{
    AiError, ContentPart, FinishReason, GenerateRequest, GenerateResponse, Message, Provider, Role,
    ToolCall, Usage,
};

pub fn request_body(req: &GenerateRequest) -> Value {
    let mut body = Map::new();
    let contents: Vec<Value> = req.messages.iter().map(encode_message).collect();
    body.insert("contents".to_owned(), Value::Array(contents));
    if let Some(system) = &req.system {
        body.insert(
            "systemInstruction".to_owned(),
            json!({"parts": [{"text": system}]}),
        );
    }
    let mut config = Map::new();
    if let Some(max) = req.max_output_tokens {
        config.insert("maxOutputTokens".to_owned(), json!(max));
    }
    if let Some(output) = &req.output {
        config.insert("responseMimeType".to_owned(), json!("application/json"));
        config.insert("responseJsonSchema".to_owned(), output.schema.clone());
    }
    if let Some(effort) = req.reasoning_effort {
        config.insert(
            "thinkingConfig".to_owned(),
            json!({"thinkingLevel": effort.as_str()}),
        );
    }
    if !config.is_empty() {
        body.insert("generationConfig".to_owned(), Value::Object(config));
    }
    if !req.tools.is_empty() {
        let declarations: Vec<Value> = req
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parametersJsonSchema": tool.parameters,
                })
            })
            .collect();
        body.insert(
            "tools".to_owned(),
            json!([{"functionDeclarations": declarations}]),
        );
    }
    Value::Object(body)
}

fn inline(mime_type: &str, data_b64: &str) -> Value {
    json!({"inlineData": {"mimeType": mime_type, "data": data_b64}})
}

fn encode_message(message: &Message) -> Value {
    match message.role {
        Role::User => {
            let parts: Vec<Value> = message
                .content
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text { text } => Some(json!({"text": text})),
                    ContentPart::Image {
                        mime_type,
                        data_b64,
                    }
                    | ContentPart::File {
                        mime_type,
                        data_b64,
                        ..
                    } => Some(inline(mime_type, data_b64)),
                    _ => None,
                })
                .collect();
            json!({"role": "user", "parts": parts})
        }
        Role::Assistant => {
            let replay = message.content.iter().find_map(|part| match part {
                ContentPart::ProviderItems {
                    provider: Provider::Google,
                    items,
                } => Some(items.clone()),
                _ => None,
            });
            let parts = replay.unwrap_or_else(|| {
                message
                    .content
                    .iter()
                    .filter_map(|part| match part {
                        ContentPart::Text { text } => Some(json!({"text": text})),
                        ContentPart::ToolCall(call) => Some(json!({
                            "functionCall": {"name": call.name, "args": call.arguments},
                        })),
                        _ => None,
                    })
                    .collect()
            });
            json!({"role": "model", "parts": parts})
        }
        Role::Tool => {
            let parts: Vec<Value> = message
                .content
                .iter()
                .filter_map(|part| match part {
                    ContentPart::ToolResult { name, output, .. } => Some(json!({
                        "functionResponse": {
                            "name": name,
                            "response": {"name": name, "content": output},
                        },
                    })),
                    _ => None,
                })
                .collect();
            json!({"role": "user", "parts": parts})
        }
    }
}

pub fn parse_response(body: &[u8]) -> Result<GenerateResponse, AiError> {
    let value = super::decode_json(body)?;
    if let Some(reason) = value
        .pointer("/promptFeedback/blockReason")
        .and_then(Value::as_str)
    {
        return Err(AiError::Refused(format!("prompt blocked: {reason}")));
    }
    let candidate = value
        .pointer("/candidates/0")
        .cloned()
        .unwrap_or(Value::Null);
    let parts = candidate
        .pointer("/content/parts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut text = Vec::new();
    let mut reasoning = Vec::new();
    let mut tool_calls = Vec::new();
    for part in &parts {
        if let Some(call) = part.get("functionCall") {
            tool_calls.push(ToolCall {
                call_id: omni_core::ids::uuid_v4(),
                name: call
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                arguments: call.get("args").cloned().unwrap_or_else(|| json!({})),
            });
        } else if let Some(t) = part.get("text").and_then(Value::as_str) {
            if part.get("thought").and_then(Value::as_bool) == Some(true) {
                reasoning.push(t.to_owned());
            } else {
                text.push(t.to_owned());
            }
        }
    }
    let finish = match candidate.get("finishReason").and_then(Value::as_str) {
        Some("MAX_TOKENS") => FinishReason::Length,
        Some("SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII") => {
            FinishReason::ContentFilter
        }
        Some("STOP") | None if !tool_calls.is_empty() => FinishReason::ToolCalls,
        Some("STOP") | None => FinishReason::Stop,
        Some(_) => FinishReason::Other,
    };
    let usage = value
        .get("usageMetadata")
        .map(parse_usage)
        .unwrap_or_default();
    Ok(GenerateResponse {
        text: text.concat(),
        tool_calls,
        usage,
        finish,
        reasoning: join_nonempty(reasoning),
        provider_items: parts,
    })
}

/// `convertGoogleUsage`: output total = candidates + thoughts.
pub fn parse_usage(usage: &Value) -> Usage {
    let prompt = u64_at(usage, "/promptTokenCount");
    let cached = u64_at(usage, "/cachedContentTokenCount");
    let candidates = u64_at(usage, "/candidatesTokenCount");
    let thoughts = u64_at(usage, "/thoughtsTokenCount");
    Usage {
        input_tokens: prompt,
        input_no_cache_tokens: prompt.saturating_sub(cached),
        cache_read_tokens: cached,
        cache_write_tokens: 0,
        output_tokens: candidates + thoughts,
        reasoning_tokens: thoughts,
    }
}
