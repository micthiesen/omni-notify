//! Port of `src/workspaces/schema.test.ts`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_workspaces::WorkspaceOutput;
use omni_workspaces::engine::workspace_output_schema;
use serde_json::{Value, json};

fn find_keyword(value: &Value, keyword: &str) -> bool {
    match value {
        Value::Array(items) => items.iter().any(|item| find_keyword(item, keyword)),
        Value::Object(map) => {
            map.contains_key(keyword) || map.values().any(|item| find_keyword(item, keyword))
        }
        _ => false,
    }
}

#[test]
fn does_not_emit_one_of_which_openai_response_formats_reject() {
    let schema = workspace_output_schema();
    assert!(!find_keyword(&schema, "oneOf"));
    assert!(omni_ai::schema::is_strict_compatible(&schema));
}

#[test]
fn uses_one_shared_proposal_shape_for_email_and_calendar_actions() {
    let output: WorkspaceOutput = serde_json::from_value(json!({
        "response": "Done",
        "subjects": [],
        "sources": [],
        "proposals": [{
            "subject_id": "subject-1",
            "title": "Proposal",
            "description": "Review this",
            "senders": [],
            "domains": [],
            "subject_keywords": [],
            "body_keywords": [],
            "type": "email_scope",
            "event": null
        }],
        "notification": null
    }))
    .unwrap();
    assert!(output.proposals[0].event.is_none());
}
