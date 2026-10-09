//! Workspace REST routes: statuses,
//! error bodies and payload shapes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use common::Harness;
use omni_api::workspaces::{
    WorkspaceActionType, WorkspacePapercutCategory, WorkspaceResponse, WorkspaceSubjectResponse,
    WorkspaceSubjectStatus, WorkspacesResponse,
};
use omni_tasks::CronSchedule;
use omni_workspaces::WorkspaceTask;
use omni_workspaces::persistence::{NewAction, NewPapercut};
use serde_json::json;

async fn seeded() -> (Harness, axum::Router, String, String) {
    let h = Harness::new().await;
    h.subject(
        "purchase-research",
        "camera",
        WorkspaceSubjectStatus::Active,
    )
    .await;
    let (action, _) = h
        .repo()
        .add_action(NewAction {
            workspace_id: "purchase-research".to_owned(),
            subject_id: "camera".to_owned(),
            action_type: WorkspaceActionType::EmailScope,
            title: "Watch".to_owned(),
            description: "Watch one retailer.".to_owned(),
            payload: json!({"senders":["a@shop.example"],"domains":[],"subjectKeywords":[],"bodyKeywords":[]}).to_string(),
            run_id: Some("run-1".to_owned()),
        })
        .await
        .unwrap();
    let papercut = h
        .repo()
        .report_papercut(NewPapercut {
            workspace_id: "purchase-research".to_owned(),
            subject_id: None,
            run_id: None,
            category: WorkspacePapercutCategory::UiGap,
            title: "No photo upload".to_owned(),
            detail: "d".to_owned(),
            related_tool: None,
        })
        .await
        .unwrap();
    let router = omni_workspaces::routes::router(h.service.clone());
    (h, router, action.action_id, papercut.papercut_id)
}

#[tokio::test]
async fn lists_workspaces_with_counts_and_full_definitions() {
    let (h, router, _, _) = seeded().await;
    let (status, body) = h.app.get_json(&router, "/api/workspaces").await;
    assert_eq!(status, StatusCode::OK);
    let decoded: WorkspacesResponse = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(decoded.workspaces.len(), 2);
    let purchase = &body["workspaces"][0];
    assert_eq!(purchase["id"], "purchase-research");
    assert_eq!(purchase["taskName"], "PurchaseResearch");
    assert!(
        purchase["instructions"]
            .as_str()
            .unwrap()
            .starts_with("You maintain purchase dossiers")
    );
    assert!(purchase.get("scheduledRuns").is_none());
    assert_eq!(body["workspaces"][1]["scheduledRuns"], false);
    assert_eq!(purchase["activeSubjectCount"], 1);
    assert_eq!(purchase["pendingActionCount"], 1);
    assert_eq!(purchase["openPapercutCount"], 1);
    assert_eq!(purchase["subjects"][0]["subjectId"], "camera");
}

#[tokio::test]
async fn serves_workspace_and_subject_details() {
    let (h, router, action_id, papercut_id) = seeded().await;
    let (status, body) = h.app.get_json(&router, "/api/workspaces/nope").await;
    assert_eq!(
        (status, body),
        (StatusCode::NOT_FOUND, json!({"error": "Unknown workspace"}))
    );

    let (status, body) = h
        .app
        .get_json(&router, "/api/workspaces/purchase-research")
        .await;
    assert_eq!(status, StatusCode::OK);
    let detail: WorkspaceResponse = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(detail.actions[0].action_id, action_id);
    assert_eq!(
        body["papercuts"][0]["fingerprint"],
        "purchase-research:ui-gap::no photo upload"
    );
    assert_eq!(body["actions"][0]["type"], "email_scope");

    let (status, body) = h
        .app
        .get_json(
            &router,
            "/api/workspaces/purchase-research/subjects/missing",
        )
        .await;
    assert_eq!(
        (status, body),
        (
            StatusCode::NOT_FOUND,
            json!({"error": "Unknown workspace subject"})
        )
    );
    let (status, body) = h
        .app
        .get_json(&router, "/api/workspaces/purchase-research/subjects/camera")
        .await;
    assert_eq!(status, StatusCode::OK);
    let subject: WorkspaceSubjectResponse = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(subject.subject.subject_id, "camera");
    assert_eq!(body["emailScope"], serde_json::Value::Null);
    assert_eq!(subject.papercuts[0].papercut_id, papercut_id);
    for key in [
        "artifacts",
        "artifactRevisions",
        "messages",
        "sources",
        "actions",
    ] {
        assert!(body[key].is_array(), "{key}");
    }
}

#[tokio::test]
async fn queues_messages_through_the_workspace_task() {
    let (h, router, _, _) = seeded().await;
    let definition = h.service.definition("purchase-research").unwrap().clone();
    let schedule = CronSchedule::parse(&definition.schedule, &jiff::tz::TimeZone::UTC).unwrap();
    let path = "/api/workspaces/purchase-research/messages";

    let (status, body) = h
        .app
        .post_json(&router, path, &json!({"message": "Find a lens"}))
        .await;
    assert_eq!(
        (status, body),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error": "Workspace task is unavailable"})
        )
    );
    h.app
        .ctx
        .tasks
        .track(Arc::new(WorkspaceTask::new(
            h.service.clone(),
            definition,
            schedule,
        )))
        .unwrap();

    for bad in [
        json!({"message": " padded "}),
        json!({"message": ""}),
        json!({}),
        json!({"message": "x", "subjectId": ""}),
    ] {
        let (status, body) = h.app.post_json(&router, path, &bad).await;
        assert_eq!(
            (status, body),
            (
                StatusCode::BAD_REQUEST,
                json!({"error": "A message is required"})
            )
        );
    }
    let (status, _) = h
        .app
        .post_json(
            &router,
            "/api/workspaces/nope/messages",
            &json!({"message": "x"}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = h
        .app
        .post_json(
            &router,
            path,
            &json!({"message": "x", "subjectId": "missing"}),
        )
        .await;
    assert_eq!(
        (status, body),
        (
            StatusCode::NOT_FOUND,
            json!({"error": "Unknown workspace subject"})
        )
    );
    let (status, body) = h
        .app
        .post_json(
            &router,
            path,
            &json!({"message": "Find a lens", "subjectId": "camera"}),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(
        body["runId"]
            .as_str()
            .unwrap()
            .starts_with("PurchaseResearch:")
    );
    let (status, body) = h
        .app
        .post_json(&router, path, &json!({"message": "Again"}))
        .await;
    assert_eq!(
        (status, body),
        (
            StatusCode::CONFLICT,
            json!({"error": "Workspace agent is already running"})
        )
    );
}

#[tokio::test]
async fn changes_subject_status() {
    let (h, router, _, _) = seeded().await;
    let path = "/api/workspaces/purchase-research/subjects/camera/status";
    let (status, body) = h
        .app
        .post_json(&router, path, &json!({"status": "bogus"}))
        .await;
    assert_eq!(
        (status, body),
        (
            StatusCode::BAD_REQUEST,
            json!({"error": "A valid status is required"})
        )
    );
    let (status, body) = h
        .app
        .post_json(&router, path, &json!({"status": "archived"}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["subject"]["status"], "archived");
    assert_eq!(body["subject"]["createdAt"], 1);
    assert!(body["subject"]["updatedAt"].as_i64().unwrap() > 1);
    let (status, _) = h
        .app
        .post_json(
            &router,
            "/api/workspaces/purchase-research/subjects/x/status",
            &json!({"status": "active"}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn approves_rejects_and_reports_conflicts() {
    let (h, router, action_id, _) = seeded().await;
    let (status, body) = h
        .app
        .post_json(
            &router,
            &format!("/api/workspace-actions/{action_id}/approve"),
            &json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["action"]["status"], "approved");
    assert_eq!(body["action"]["result"], "Email scope enabled");
    let (status, body) = h
        .app
        .post_json(
            &router,
            &format!("/api/workspace-actions/{action_id}/reject"),
            &json!({}),
        )
        .await;
    assert_eq!(
        (status, body),
        (
            StatusCode::CONFLICT,
            json!({"error": "Workspace action is already approved"})
        )
    );
    let (status, body) = h
        .app
        .post_json(
            &router,
            "/api/workspace-actions/missing/approve",
            &json!({}),
        )
        .await;
    assert_eq!(
        (status, body),
        (
            StatusCode::CONFLICT,
            json!({"error": "Workspace action not found"})
        )
    );
}

#[tokio::test]
async fn lists_and_resolves_papercuts() {
    let (h, router, _, papercut_id) = seeded().await;
    let (_, body) = h
        .app
        .get_json(&router, "/api/workspace-papercuts?status=open")
        .await;
    assert_eq!(body["papercuts"].as_array().unwrap().len(), 1);
    let (_, body) = h
        .app
        .get_json(&router, "/api/workspace-papercuts?status=dismissed")
        .await;
    assert_eq!(body["papercuts"], json!([]));
    let (_, body) = h
        .app
        .get_json(&router, "/api/workspace-papercuts?status=bogus")
        .await;
    assert_eq!(
        body["papercuts"].as_array().unwrap().len(),
        1,
        "invalid status filters nothing"
    );
    let (_, body) = h
        .app
        .get_json(
            &router,
            "/api/workspace-papercuts?workspaceId=marketplace-selling",
        )
        .await;
    assert_eq!(body["papercuts"], json!([]));

    let path = format!("/api/workspace-papercuts/{papercut_id}/resolve");
    let (status, body) = h
        .app
        .post_json(
            &router,
            &path,
            &json!({"status": "addressed", "resolution": " padded"}),
        )
        .await;
    assert_eq!(
        (status, body),
        (
            StatusCode::BAD_REQUEST,
            json!({"error": "Status and resolution are required"})
        )
    );
    let (status, body) = h
        .app
        .post_json(
            &router,
            &path,
            &json!({"status": "addressed", "resolution": "Added uploads"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["papercut"]["status"], "addressed");
    assert_eq!(body["papercut"]["resolution"], "Added uploads");
    let (status, body) = h
        .app
        .post_json(
            &router,
            "/api/workspace-papercuts/missing/resolve",
            &json!({"status": "dismissed", "resolution": "n/a"}),
        )
        .await;
    assert_eq!(
        (status, body),
        (StatusCode::NOT_FOUND, json!({"error": "Unknown papercut"}))
    );
}
