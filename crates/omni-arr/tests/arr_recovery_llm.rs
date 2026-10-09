//! The model assessment prompt and verdict rules, plus `assess_with_llm`
//! against a scripted model (request bounds and schema-checked verdicts).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_ai::{GenerateResponse, ModelRole};
use omni_arr::arr_recovery::llm::{LLM_TIMEOUT, assess_with_llm, validate_llm_decision};
use omni_arr::arr_recovery::{
    ArrKind, Decision, DecisionSource, Evidence, Grab, ImportFile, QueueItem, Rejection,
    StatusMessage, Target, TargetEpisode,
};
use serde_json::{Value, json};

fn queue() -> QueueItem {
    QueueItem {
        id: 10,
        download_id: "download-1".into(),
        title: "Ambiguous.Release.S01E01".into(),
        status: "completed".into(),
        tracked_download_status: "warning".into(),
        tracked_download_state: "importBlocked".into(),
        status_messages: vec![StatusMessage {
            title: "Import failed".into(),
            messages: vec!["Release could not be mapped automatically".into()],
        }],
        size: 1_000.0,
        sizeleft: 0.0,
        output_path: Some("/downloads/ambiguous".into()),
        added: None,
        series_id: Some(42),
        episode_id: None,
        movie_id: None,
        protocol: None,
        download_client: None,
    }
}

fn target() -> Target {
    Target {
        id: 42,
        title: "The Example".into(),
        year: 2020,
        monitored: true,
        has_file: false,
        path: "/media/The Example".into(),
        episode_ids: vec![101],
        episodes: vec![TargetEpisode {
            id: 101,
            season_number: 1,
            episode_number: 1,
            title: "Pilot".into(),
            has_file: false,
            monitored: true,
        }],
        alternate_titles: vec![],
    }
}

fn file() -> ImportFile {
    ImportFile {
        folder_name: None,
        id: 501,
        path: "/downloads/ambiguous/NqFGW2VSR2C49AkyiFgnB6G.mkv".into(),
        name: "NqFGW2VSR2C49AkyiFgnB6G.mkv".into(),
        size: 1_000.0,
        series_id: Some(42),
        movie_id: None,
        season_number: Some(1),
        episode_ids: vec![101],
        quality: serde_json::Map::new(),
        languages: None,
        release_group: None,
        indexer_flags: None,
        release_type: None,
        rejections: vec![],
    }
}

fn evidence() -> Evidence {
    Evidence {
        download_health: None,
        kind: ArrKind::Sonarr,
        items: vec![queue()],
        target: target(),
        files: vec![file()],
        grabs: vec![Grab {
            download_id: "download-1".into(),
            source_title: "Alias.Name.S01E01".into(),
            series_id: Some(42),
            movie_id: None,
            episode_id: Some(101),
            event_type: "grabbed".into(),
            date: "2026-09-12T00:00:00Z".into(),
        }],
    }
}

fn verdict(overrides: Value) -> Value {
    let mut base = json!({
        "action": "import",
        "diagnosis": "safe_import",
        "reason": "The alternate release title and episode numbering describe the target",
        "confidence": 0.95,
        "replace": false,
        "fileIds": [501],
        "downloadIds": ["download-1"],
    });
    if let (Some(base), Value::Object(extra)) = (base.as_object_mut(), overrides) {
        base.extend(extra);
    }
    base
}

fn removal() -> Value {
    verdict(json!({ "action": "remove", "diagnosis": "wrong_content", "replace": true }))
}

#[test]
fn accepts_a_high_confidence_opaque_filename_import_while_preserving_structural_guards() {
    let decision = validate_llm_decision(&evidence(), &verdict(json!({})));
    assert!(matches!(
        decision,
        Decision::Import {
            source: DecisionSource::Llm,
            ..
        }
    ));
}

#[test]
fn defers_an_import_with() {
    for (name, overrides) in [
        ("unknown file", json!({ "fileIds": [999] })),
        (
            "invented download",
            json!({ "downloadIds": ["download-2"] }),
        ),
        ("low confidence", json!({ "confidence": 0.89 })),
        ("wrong diagnosis", json!({ "diagnosis": "ambiguous" })),
        ("replacement import", json!({ "replace": true })),
    ] {
        let decision = validate_llm_decision(&evidence(), &verdict(overrides));
        assert_eq!(decision.action(), "defer", "{name}");
    }
}

#[test]
fn defers_an_import_with_a_rejection_or_a_file_outside_the_output_path() {
    let rejected = Evidence {
        files: vec![ImportFile {
            rejections: vec![Rejection {
                reason: "Unknown import rejection".into(),
                kind: "quality".into(),
            }],
            ..file()
        }],
        ..evidence()
    };
    let escaped = Evidence {
        files: vec![ImportFile {
            path: "/tmp/injected.mkv".into(),
            ..file()
        }],
        ..evidence()
    };
    assert_eq!(
        validate_llm_decision(&rejected, &verdict(json!({}))).action(),
        "defer"
    );
    assert_eq!(
        validate_llm_decision(&escaped, &verdict(json!({}))).action(),
        "defer"
    );
}

#[test]
fn never_lets_the_model_override_infrastructure_or_media_integrity_evidence() {
    for message in [
        "Permission denied",
        "Sample file detected",
        "Media is corrupt",
    ] {
        let unsafe_evidence = Evidence {
            items: vec![QueueItem {
                status_messages: vec![StatusMessage {
                    title: "Import failed".into(),
                    messages: vec![message.into()],
                }],
                ..queue()
            }],
            ..evidence()
        };
        assert_eq!(
            validate_llm_decision(&unsafe_evidence, &verdict(json!({}))).action(),
            "defer",
            "{message}"
        );
    }
}

fn wrong_content_file() -> ImportFile {
    ImportFile {
        series_id: Some(999),
        episode_ids: vec![],
        ..file()
    }
}

#[test]
fn allows_replacement_only_for_confidently_wrong_content_with_matching_grab_history() {
    let wrong_content = Evidence {
        files: vec![wrong_content_file()],
        ..evidence()
    };
    let decision = validate_llm_decision(
        &wrong_content,
        &verdict(json!({
            "action": "remove",
            "diagnosis": "wrong_content",
            "reason": "The file belongs to an unrelated series",
            "replace": true,
        })),
    );
    assert!(matches!(
        decision,
        Decision::Remove {
            replace: true,
            source: DecisionSource::Llm,
            ..
        }
    ));
}

#[test]
fn defers_replacement_when_it_would_discard_a_valid_requested_partial_file() {
    assert_eq!(
        validate_llm_decision(&evidence(), &removal()).action(),
        "defer"
    );
}

#[test]
fn defers_replacement_without_known_matching_grab_history() {
    let wrong_content = Evidence {
        files: vec![wrong_content_file()],
        grabs: vec![],
        ..evidence()
    };
    assert_eq!(
        validate_llm_decision(&wrong_content, &removal()).action(),
        "defer"
    );
}

#[test]
fn defers_replacement_of_an_active_download_or_an_empty_preview() {
    let active = Evidence {
        items: vec![QueueItem {
            status: "downloading".into(),
            tracked_download_state: "downloading".into(),
            sizeleft: 500.0,
            ..queue()
        }],
        files: vec![wrong_content_file()],
        ..evidence()
    };
    assert_eq!(validate_llm_decision(&active, &removal()).action(), "defer");
    let empty = Evidence {
        files: vec![],
        ..evidence()
    };
    let mut no_files = removal();
    no_files["fileIds"] = json!([]);
    assert_eq!(validate_llm_decision(&empty, &no_files).action(), "defer");
}

#[test]
fn returns_a_model_defer_reason_without_making_it_executable() {
    let decision = validate_llm_decision(
        &evidence(),
        &verdict(json!({
            "action": "defer",
            "diagnosis": "ambiguous",
            "reason": "Numbering remains ambiguous",
            "confidence": 0.5,
        })),
    );
    assert_eq!(
        decision,
        Decision::Defer {
            reason: "Numbering remains ambiguous".into(),
            source: DecisionSource::Llm,
        }
    );
}

#[test]
fn turns_malformed_output_into_a_conservative_defer() {
    let decision = validate_llm_decision(&evidence(), &json!({ "action": "import" }));
    assert_eq!(decision.action(), "defer");
    assert_eq!(decision.source(), DecisionSource::Llm);
}

#[tokio::test]
async fn assess_with_llm_sends_one_bounded_unretried_request_and_validates_the_verdict() {
    let app = omni_testkit::TestApp::new().await;
    app.ai.script(
        ModelRole::ArrRecovery,
        vec![GenerateResponse::text(verdict(json!({})).to_string())],
    );
    let model = app
        .ctx
        .ai
        .model_for(&app.ctx.config, ModelRole::ArrRecovery)
        .unwrap();
    let decision = assess_with_llm(&app.ctx.ai, model.as_ref(), &evidence())
        .await
        .unwrap();
    assert!(matches!(decision, Decision::Import { .. }));
    let requests = app.ai.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0].1;
    assert_eq!(request.max_retries, 0);
    assert_eq!(request.timeout, LLM_TIMEOUT);
    assert_eq!(request.max_output_tokens, Some(2_000));
    assert!(request.output.is_some());
}

#[tokio::test]
async fn assess_with_llm_skips_the_model_when_safeguards_prohibit_an_action() {
    let app = omni_testkit::TestApp::new().await;
    let model = app
        .ctx
        .ai
        .model_for(&app.ctx.config, ModelRole::ArrRecovery)
        .unwrap();
    let unsafe_evidence = Evidence {
        items: vec![QueueItem {
            status_messages: vec![StatusMessage {
                title: "Import failed".into(),
                messages: vec!["Permission denied".into()],
            }],
            ..queue()
        }],
        ..evidence()
    };
    let decision = assess_with_llm(&app.ctx.ai, model.as_ref(), &unsafe_evidence)
        .await
        .unwrap();
    assert_eq!(decision.action(), "defer");
    assert!(app.ai.requests().is_empty());
}

#[tokio::test]
async fn assess_with_llm_fails_on_output_outside_the_strict_schema() {
    // A strict-schema violation fails the assessment so the next pass retries
    // it, rather than recording a defer.
    for output in [
        verdict(json!({ "extra": true })),
        verdict(json!({ "reason": "x".repeat(501) })),
        verdict(json!({ "fileIds": [501.5] })),
        verdict(json!({ "downloadIds": [""] })),
        verdict(json!({ "confidence": 1.5 })),
    ] {
        let app = omni_testkit::TestApp::new().await;
        app.ai.script(
            ModelRole::ArrRecovery,
            vec![GenerateResponse::text(output.to_string())],
        );
        let model = app
            .ctx
            .ai
            .model_for(&app.ctx.config, ModelRole::ArrRecovery)
            .unwrap();
        let error = assess_with_llm(&app.ctx.ai, model.as_ref(), &evidence())
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("assess ARR recovery with model: No object generated"),
            "{error}"
        );
    }
}
