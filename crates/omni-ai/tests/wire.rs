//! Provider wire formats: request bodies and response parsing (pure), matching what
//! `@ai-sdk/openai`, `@ai-sdk/anthropic` and `@ai-sdk/google` send and read.
#![allow(clippy::unwrap_used)]

use omni_ai::wire::{anthropic_messages, gemini, openai_responses};
use omni_ai::{
    AiError, ContentPart, FinishReason, GenerateRequest, Message, OutputSpec, Provider,
    ReasoningEffort, Role, ToolCall, ToolSpec, Usage,
};
use serde_json::{Value, json};

#[derive(schemars::JsonSchema, serde::Deserialize)]
#[allow(dead_code)]
struct Decision {
    approve: bool,
    reason: Option<String>,
}

fn tool(name: &str, parameters: Value) -> ToolSpec {
    ToolSpec {
        name: name.to_owned(),
        description: format!("{name} tool"),
        parameters,
    }
}

fn conversation() -> GenerateRequest {
    GenerateRequest {
        system: Some("Be brief.".to_owned()),
        messages: vec![
            Message {
                role: Role::User,
                content: vec![
                    ContentPart::Text {
                        text: "Look at this".to_owned(),
                    },
                    ContentPart::File {
                        mime_type: "application/pdf".to_owned(),
                        data_b64: "JVBERg==".to_owned(),
                        filename: None,
                    },
                    ContentPart::File {
                        mime_type: "image/png".to_owned(),
                        data_b64: "iVBO".to_owned(),
                        filename: Some("x.png".to_owned()),
                    },
                ],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentPart::ToolCall(ToolCall {
                    call_id: "call_1".to_owned(),
                    name: "web_search".to_owned(),
                    arguments: json!({"query": "rust"}),
                })],
            },
            Message {
                role: Role::Tool,
                content: vec![ContentPart::ToolResult {
                    call_id: "call_1".to_owned(),
                    name: "web_search".to_owned(),
                    output: json!({"results": []}),
                    is_error: false,
                }],
            },
        ],
        tools: vec![tool(
            "web_search",
            json!({
                "type": "object",
                "properties": {"query": {"type": "string"}, "topic": {"type": "string"}},
                "required": ["query"],
                "additionalProperties": false
            }),
        )],
        output: Some(OutputSpec::of::<Decision>()),
        max_output_tokens: Some(700),
        reasoning_effort: Some(ReasoningEffort::High),
        ..GenerateRequest::default()
    }
}

#[test]
fn openai_request_matches_the_ai_sdk_responses_shape() {
    let body = openai_responses::request_body("gpt-6-luna", &conversation());
    assert_eq!(body["model"], "gpt-6-luna");
    assert_eq!(body["max_output_tokens"], 700);
    assert_eq!(body["reasoning"], json!({"effort": "high"}));
    assert_eq!(body["text"]["format"]["type"], "json_schema");
    assert_eq!(body["text"]["format"]["strict"], true);
    assert_eq!(body["text"]["format"]["name"], "response");
    assert_eq!(
        body["text"]["format"]["schema"]["required"],
        json!(["approve", "reason"])
    );
    let input = body["input"].as_array().unwrap();
    assert_eq!(
        input[0],
        json!({"role": "developer", "content": "Be brief."})
    );
    assert_eq!(
        input[1]["content"],
        json!([
            {"type": "input_text", "text": "Look at this"},
            {"type": "input_file", "filename": "part-1.pdf", "file_data": "data:application/pdf;base64,JVBERg=="},
            {"type": "input_image", "image_url": "data:image/png;base64,iVBO"},
        ])
    );
    assert_eq!(
        input[2],
        json!({"type": "function_call", "call_id": "call_1", "name": "web_search", "arguments": "{\"query\":\"rust\"}"})
    );
    assert_eq!(
        input[3],
        json!({"type": "function_call_output", "call_id": "call_1", "output": "{\"results\":[]}"})
    );
    assert_eq!(
        body["tools"][0]["strict"], false,
        "optional arguments are not strict"
    );
    assert_eq!(body["tool_choice"], "auto");
}

#[test]
fn openai_non_reasoning_models_get_a_system_message_and_no_effort() {
    let body = openai_responses::request_body("gpt-4.1-mini", &conversation());
    assert_eq!(body["input"][0]["role"], "system");
    assert!(body.get("reasoning").is_none());
    assert!(openai_responses::is_reasoning_model("o3"));
    assert!(openai_responses::is_reasoning_model("gpt-5.6-luna"));
    assert!(!openai_responses::is_reasoning_model("gpt-5-chat-latest"));
}

#[test]
fn openai_response_items_are_parsed_and_replayed_by_reference() {
    let body = json!({
        "id": "resp_1",
        "output": [
            {"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "thinking"}]},
            {"type": "message", "id": "msg_1", "content": [{"type": "output_text", "text": "Checking."}]},
            {"type": "function_call", "id": "fc_1", "call_id": "call_9", "name": "fetch_url", "arguments": "{\"url\":\"https://a.test\"}", "status": "completed"}
        ],
        "usage": {
            "input_tokens": 1553,
            "input_tokens_details": {"cached_tokens": 0, "cache_write_tokens": 1550},
            "output_tokens": 33,
            "output_tokens_details": {"reasoning_tokens": 20}
        }
    });
    let response = openai_responses::parse_response(body.to_string().as_bytes()).unwrap();
    assert_eq!(response.text, "Checking.");
    assert_eq!(response.reasoning.as_deref(), Some("thinking"));
    assert_eq!(response.finish, FinishReason::ToolCalls);
    assert_eq!(
        response.tool_calls[0].arguments,
        json!({"url": "https://a.test"})
    );
    // Same numbers as a stored production cost-event row.
    assert_eq!(
        response.usage,
        Usage {
            input_tokens: 1553,
            input_no_cache_tokens: 3,
            cache_read_tokens: 0,
            cache_write_tokens: 1550,
            output_tokens: 33,
            reasoning_tokens: 20,
        }
    );

    let replay = GenerateRequest {
        messages: vec![Message {
            role: Role::Assistant,
            content: vec![ContentPart::ProviderItems {
                provider: Provider::OpenAi,
                items: response.provider_items.clone(),
            }],
        }],
        ..GenerateRequest::default()
    };
    let input = openai_responses::request_body("gpt-6-luna", &replay)["input"].clone();
    assert_eq!(
        input,
        json!([
            {"type": "item_reference", "id": "rs_1"},
            {"type": "item_reference", "id": "msg_1"},
            {"type": "function_call", "id": "fc_1", "call_id": "call_9", "name": "fetch_url", "arguments": "{\"url\":\"https://a.test\"}"}
        ])
    );
}

#[test]
fn openai_refusals_and_truncation_are_reported() {
    let refusal =
        json!({"output": [{"type": "message", "content": [{"type": "refusal", "refusal": "no"}]}]});
    assert!(matches!(
        openai_responses::parse_response(refusal.to_string().as_bytes()),
        Err(AiError::Refused(message)) if message == "no"
    ));
    let cut = json!({"output": [{"type": "message", "content": [{"type": "output_text", "text": "{\"a\":"}]}],
        "incomplete_details": {"reason": "max_output_tokens"}});
    let response = openai_responses::parse_response(cut.to_string().as_bytes()).unwrap();
    assert_eq!(response.finish, FinishReason::Length);
}

#[test]
fn provider_errors_carry_the_api_message() {
    let error = omni_ai::wire::provider_error(
        429,
        br#"{"error":{"message":"Rate limit reached","type":"requests"}}"#,
    );
    assert!(matches!(
        &error,
        AiError::Provider { status: 429, message } if message == "Rate limit reached"
    ));
    assert!(error.is_retryable());
}

#[test]
fn anthropic_request_and_response() {
    let body = anthropic_messages::request_body("claude-sonnet-5-5", &conversation());
    assert_eq!(body["max_tokens"], 700);
    assert_eq!(body["system"], "Be brief.");
    assert_eq!(body["output_config"]["format"]["type"], "json_schema");
    assert_eq!(body["output_config"]["effort"], "high");
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0]["content"][1]["type"], "document");
    assert_eq!(messages[0]["content"][2]["type"], "image");
    assert_eq!(
        messages[1]["content"][0],
        json!({"type": "tool_use", "id": "call_1", "name": "web_search", "input": {"query": "rust"}})
    );
    assert_eq!(
        messages[2],
        json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "{\"results\":[]}"}]})
    );
    assert_eq!(
        body["tools"][0]["input_schema"]["required"],
        json!(["query"])
    );

    let reply = json!({
        "content": [
            {"type": "thinking", "thinking": "hmm", "signature": "sig"},
            {"type": "tool_use", "id": "toolu_1", "name": "web_search", "input": {"query": "x"}}
        ],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 10, "cache_creation_input_tokens": 5, "cache_read_input_tokens": 20, "output_tokens": 7}
    });
    let response = anthropic_messages::parse_response(reply.to_string().as_bytes()).unwrap();
    assert_eq!(response.finish, FinishReason::ToolCalls);
    assert_eq!(response.reasoning.as_deref(), Some("hmm"));
    assert_eq!(response.usage.input_tokens, 35);
    assert_eq!(response.usage.input_no_cache_tokens, 10);
    assert_eq!(
        response.provider_items.len(),
        2,
        "signed thinking is replayed"
    );

    let refusal = json!({"content": [], "stop_reason": "refusal", "stop_details": {"type": "refusal", "explanation": "declined"}});
    assert!(matches!(
        anthropic_messages::parse_response(refusal.to_string().as_bytes()),
        Err(AiError::Refused(_))
    ));
}

#[test]
fn gemini_request_and_response() {
    let body = gemini::request_body(&conversation());
    assert_eq!(
        body["systemInstruction"],
        json!({"parts": [{"text": "Be brief."}]})
    );
    assert_eq!(
        body["generationConfig"]["responseMimeType"],
        "application/json"
    );
    assert_eq!(body["generationConfig"]["maxOutputTokens"], 700);
    assert_eq!(
        body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "high"
    );
    let contents = body["contents"].as_array().unwrap();
    assert_eq!(contents[1]["role"], "model");
    assert_eq!(
        contents[1]["parts"][0],
        json!({"functionCall": {"name": "web_search", "args": {"query": "rust"}}})
    );
    assert_eq!(
        contents[2]["parts"][0]["functionResponse"]["response"]["content"],
        json!({"results": []})
    );

    let reply = json!({
        "candidates": [{"content": {"role": "model", "parts": [
            {"text": "pondering", "thought": true},
            {"text": "{\"approve\":true,\"reason\":null}"}
        ]}, "finishReason": "STOP"}],
        "usageMetadata": {"promptTokenCount": 6177, "cachedContentTokenCount": 3794, "candidatesTokenCount": 110, "thoughtsTokenCount": 990}
    });
    let response = gemini::parse_response(reply.to_string().as_bytes()).unwrap();
    assert_eq!(response.text, "{\"approve\":true,\"reason\":null}");
    assert_eq!(response.reasoning.as_deref(), Some("pondering"));
    // Same numbers as a production google cost-event row.
    assert_eq!(response.usage.input_no_cache_tokens, 2383);
    assert_eq!(response.usage.output_tokens, 1100);
    assert_eq!(response.usage.reasoning_tokens, 990);
}
