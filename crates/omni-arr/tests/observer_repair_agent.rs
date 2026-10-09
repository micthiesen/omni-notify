//! Port of `src/observer-repair/agent.spec.ts`, plus the agent loop against a
//! scripted model and tools.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use omni_ai::{GenerateResponse, ModelRole, ToolCall, ToolSet};
use omni_arr::observer::ObserverIssue;
use omni_arr::observer_repair::agent::{
    RepairAction, assess_issue, issue_evidence, validate_decision,
};
use serde_json::{Value, json};

fn issue(overrides: Value) -> ObserverIssue {
    let mut base = json!({
        "id": 10,
        "issueType": 1,
        "status": 1,
        "problemSeason": 3,
        "problemEpisode": 1,
        "createdAt": "2026-01-01T00:00:00.000Z",
        "updatedAt": "2026-01-01T00:00:00.000Z",
        "media": { "id": 44, "mediaType": "tv", "tmdbId": 123, "tvdbId": 456 },
        "comments": [{ "id": 1, "message": "The episode freezes", "user": null }],
    });
    base.as_object_mut()
        .unwrap()
        .extend(overrides.as_object().unwrap().clone());
    serde_json::from_value(base).unwrap()
}

fn decision(overrides: Value) -> Value {
    let mut base = json!({
        "action": "replace",
        "season": 3,
        "episodes": [1],
        "scopeComment": null,
        "reason": "Replace the faulty file",
    });
    base.as_object_mut()
        .unwrap()
        .extend(overrides.as_object().unwrap().clone());
    base
}

fn error(issue: &ObserverIssue, value: Value) -> String {
    validate_decision(issue, &value).unwrap_err().0
}

#[test]
fn accepts_a_replacement_at_the_reported_episode_scope() {
    let accepted = validate_decision(&issue(json!({})), &decision(json!({}))).unwrap();
    assert_eq!(accepted.action, RepairAction::Replace);
    assert_eq!(accepted.season, Some(3));
    assert_eq!(accepted.episodes, vec![1]);
}

#[test]
fn rejects_a_decision_that_broadens_a_season_or_episode() {
    let report = issue(json!({}));
    assert_eq!(
        error(&report, decision(json!({ "season": 4 }))),
        "Scope exceeds the reported season"
    );
    assert_eq!(
        error(&report, decision(json!({ "episodes": [2] }))),
        "Scope exceeds the reported episode"
    );
    assert_eq!(
        error(&report, decision(json!({ "episodes": [1, 2] }))),
        "Scope exceeds the reported episode"
    );
}

#[test]
fn allows_a_narrower_episode_list_only_with_an_exact_supporting_comment() {
    let all_season = issue(json!({ "problemSeason": 3, "problemEpisode": 0 }));
    let narrowed = "Narrowed scope requires an exact supporting issue comment";
    assert_eq!(
        error(&all_season, decision(json!({ "episodes": [1] }))),
        narrowed
    );
    let accepted = validate_decision(
        &all_season,
        &decision(json!({ "episodes": [1], "scopeComment": "The episode freezes" })),
    )
    .unwrap();
    assert_eq!(accepted.episodes, vec![1]);
    assert_eq!(
        accepted.scope_comment.as_deref(),
        Some("The episode freezes")
    );
    assert_eq!(
        error(
            &all_season,
            decision(json!({ "episodes": [1], "scopeComment": "The episode freezes on startup" }))
        ),
        narrowed
    );
}

#[test]
fn accepts_full_series_scope_and_rejects_unknown_media() {
    let accepted = validate_decision(
        &issue(json!({ "problemSeason": 0, "problemEpisode": 0 })),
        &decision(json!({ "season": null, "episodes": [] })),
    )
    .unwrap();
    assert_eq!(accepted.season, None);
    assert!(accepted.episodes.is_empty());
    assert_eq!(
        error(
            &issue(json!({ "media": { "id": 44, "mediaType": "music" } })),
            decision(json!({}))
        ),
        "Unknown media type"
    );
}

#[test]
fn rejects_tv_scope_for_movies() {
    let movie = issue(json!({ "media": { "id": 44, "mediaType": "movie", "tmdbId": 123 } }));
    let accepted =
        validate_decision(&movie, &decision(json!({ "season": null, "episodes": [] }))).unwrap();
    assert_eq!(accepted.action, RepairAction::Replace);
    assert_eq!(accepted.season, None);
    assert!(accepted.episodes.is_empty());
    assert_eq!(
        error(&movie, decision(json!({}))),
        "Movie scope cannot contain episodes"
    );
}

#[test]
fn rejects_duplicate_episodes_and_permits_cannot_handle() {
    assert_eq!(
        error(
            &issue(json!({ "problemEpisode": 0 })),
            decision(json!({ "episodes": [1, 1] }))
        ),
        "Duplicate episodes in scope"
    );
    let accepted = validate_decision(
        &issue(json!({})),
        &decision(json!({ "action": "cannot_handle", "season": null, "episodes": [] })),
    )
    .unwrap();
    assert_eq!(accepted.action, RepairAction::CannotHandle);
}

#[test]
fn rejects_output_outside_the_strict_schema() {
    let report = issue(json!({}));
    assert!(validate_decision(&report, &decision(json!({ "extra": true }))).is_err());
    assert!(validate_decision(&report, &decision(json!({ "reason": "" }))).is_err());
    assert!(validate_decision(&report, &decision(json!({ "episodes": [0] }))).is_err());
    assert!(validate_decision(&report, &decision(json!({ "season": 1.5 }))).is_err());
}

#[test]
fn issue_evidence_keeps_absent_and_null_apart() {
    let evidence = issue_evidence(&issue(json!({ "problemEpisode": null, "comments": null })));
    assert_eq!(evidence["episode"], Value::Null);
    assert!(evidence.get("comments").is_none());
    assert_eq!(evidence["media"]["tvdbId"], json!(456));
}

#[tokio::test]
async fn assess_issue_runs_the_tool_loop_and_validates_the_final_decision() {
    let app = omni_testkit::TestApp::new().await;
    app.ai.script(
        ModelRole::ObserverRepair,
        vec![
            GenerateResponse::tool_calls(vec![ToolCall {
                call_id: "c1".into(),
                name: "inspect_target".into(),
                arguments: json!({}),
            }]),
            GenerateResponse::text(decision(json!({})).to_string()),
        ],
    );
    let inspect =
        omni_testkit::FakeTool::new("inspect_target", vec![Ok(json!({ "fileCount": 1 }))]);
    let history = omni_testkit::FakeTool::new("historical_issues", vec![]);
    let tools = ToolSet::new()
        .with(Arc::new(inspect.clone()))
        .with(Arc::new(history));
    let model = app
        .ctx
        .ai
        .model_for(&app.ctx.config, ModelRole::ObserverRepair)
        .unwrap();

    let accepted = assess_issue(
        &app.ctx.ai,
        model.as_ref(),
        &issue(json!({})),
        json!({ "title": "Show" }),
        &tools,
    )
    .await
    .unwrap();

    assert_eq!(accepted.action, RepairAction::Replace);
    assert_eq!(inspect.calls().len(), 1);
    let requests = app.ai.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].1.max_retries, 0);
    assert_eq!(requests[0].1.max_output_tokens, Some(2_500));
    assert_eq!(requests[0].1.tools.len(), 2);
}

#[tokio::test]
async fn assess_issue_refuses_a_broadened_scope_from_the_model() {
    let app = omni_testkit::TestApp::new().await;
    app.ai.script(
        ModelRole::ObserverRepair,
        vec![GenerateResponse::text(
            decision(json!({ "season": 4 })).to_string(),
        )],
    );
    let model = app
        .ctx
        .ai
        .model_for(&app.ctx.config, ModelRole::ObserverRepair)
        .unwrap();
    let error = assess_issue(
        &app.ctx.ai,
        model.as_ref(),
        &issue(json!({})),
        json!({}),
        &ToolSet::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.operation_name(), Some("assess issue"));
    assert!(
        error
            .to_string()
            .contains("Scope exceeds the reported season")
    );
}
