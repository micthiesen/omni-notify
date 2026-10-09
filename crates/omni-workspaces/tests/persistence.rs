//! Workspace persistence and the repository semantics the engine and routes
//! rely on.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::Harness;
use omni_api::workspaces::{
    WorkspacePapercutCategory, WorkspacePapercutStatus, WorkspaceSubjectStatus,
};
use omni_store::StoreError;
use omni_store::cbor::Extra;
use omni_workspaces::entities::SubjectRow;
use omni_workspaces::persistence::NewPapercut;

#[tokio::test]
async fn rolls_back_every_write_when_the_commit_fails() {
    let h = Harness::new().await;
    let result: Result<(), StoreError> = h
        .app
        .ctx
        .store
        .write(|tx| {
            let subject = SubjectRow {
                workspace_id: "purchase-research".to_owned(),
                subject_id: "subject-1".to_owned(),
                title: "Camera".to_owned(),
                status: WorkspaceSubjectStatus::Active,
                summary: "Researching cameras".to_owned(),
                created_at: 1,
                updated_at: 1,
                last_researched_at: None,
                extra: Extra::new(),
            };
            omni_store::EntityWrite::upsert(tx, &subject, Default::default())?;
            Err(StoreError::Sqlite("abort commit".to_owned()))
        })
        .await;
    assert!(result.is_err());
    assert!(
        h.repo()
            .get_subject("purchase-research", "subject-1")
            .await
            .unwrap()
            .is_none()
    );
}

fn papercut(title: &str, run: &str) -> NewPapercut {
    NewPapercut {
        workspace_id: "purchase-research".to_owned(),
        subject_id: Some("camera".to_owned()),
        run_id: Some(run.to_owned()),
        category: WorkspacePapercutCategory::MissingCapability,
        title: title.to_owned(),
        detail: format!("detail {run}"),
        related_tool: Some("web_search".to_owned()),
    }
}

#[tokio::test]
async fn folds_repeated_open_papercuts_and_sorts_open_first() {
    let h = Harness::new().await;
    let first = h
        .repo()
        .report_papercut(papercut("Price API", "r1"))
        .await
        .unwrap();
    let again = h
        .repo()
        .report_papercut(papercut("  price api ", "r2"))
        .await
        .unwrap();
    assert_eq!(again.papercut_id, first.papercut_id);
    assert_eq!(again.occurrences, 2);
    assert_eq!(again.detail, "detail r2");
    assert_eq!(again.run_id.as_deref(), Some("r2"));
    assert_eq!(
        again.fingerprint,
        "purchase-research:missing-capability:web_search:price api"
    );
    h.repo()
        .resolve_papercut(
            &first.papercut_id,
            WorkspacePapercutStatus::Addressed,
            "Fixed",
        )
        .await
        .unwrap();
    let fresh = h
        .repo()
        .report_papercut(papercut("Price API", "r3"))
        .await
        .unwrap();
    assert_ne!(
        fresh.papercut_id, first.papercut_id,
        "a resolved papercut is not reopened"
    );
    let listed = h.repo().list_papercuts(None, None).await.unwrap();
    assert_eq!(listed[0].papercut_id, fresh.papercut_id);
    assert_eq!(listed[1].status, WorkspacePapercutStatus::Addressed);
    assert_eq!(listed[1].resolution.as_deref(), Some("Fixed"));
}

// Paused time: the test clock follows tokio time, so a busy machine cannot move
// it between the upsert and the comparison.
#[tokio::test(start_paused = true)]
async fn subject_upserts_keep_creation_and_research_times() {
    let h = Harness::new().await;
    h.subject(
        "purchase-research",
        "camera",
        WorkspaceSubjectStatus::Active,
    )
    .await;
    let updated = h
        .repo()
        .upsert_subject(omni_workspaces::persistence::SubjectUpsert {
            workspace_id: "purchase-research".to_owned(),
            subject_id: "camera".to_owned(),
            title: "Camera".to_owned(),
            status: WorkspaceSubjectStatus::Paused,
            summary: "s".to_owned(),
            created_at: Some(99),
            updated_at: None,
            last_researched_at: None,
        })
        .await
        .unwrap();
    assert_eq!(updated.created_at, 1);
    assert_eq!(updated.updated_at, h.repo().now_ms());
    assert_eq!(updated.status, WorkspaceSubjectStatus::Paused);
}
