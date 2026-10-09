//! The subsystem app wiring receives: tasks, tools, entities, data-manager rows and the
//! email handler.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_testkit::TestApp;

#[tokio::test]
async fn builds_the_full_subsystem() {
    let app = TestApp::new().await;
    let subsystem = omni_workspaces::subsystem(&app.ctx).unwrap();
    assert_eq!(subsystem.name, "workspaces");
    let tasks: Vec<(String, Option<String>, String, bool)> = subsystem
        .tasks
        .iter()
        .map(|t| {
            (
                t.name().to_owned(),
                t.display_name().map(str::to_owned),
                t.schedule().as_str().to_owned(),
                t.accepts_manual_input(),
            )
        })
        .collect();
    assert_eq!(
        tasks,
        [
            (
                "PurchaseResearch".to_owned(),
                Some("Purchase Research".to_owned()),
                "0 0 9 * * 0".to_owned(),
                true
            ),
            (
                "MarketplaceSelling".to_owned(),
                Some("Marketplace Selling".to_owned()),
                "0 0 9 * * 0".to_owned(),
                true
            ),
            (
                "WorkspaceNotifications".to_owned(),
                Some("Workspace Notifications".to_owned()),
                "*/5 * * * *".to_owned(),
                false
            ),
        ]
    );
    assert_eq!(subsystem.mcp_tools.len(), 10);
    let names: Vec<&str> = subsystem.entities.iter().map(|e| e.name).collect();
    assert_eq!(
        names,
        [
            "workspace-subject",
            "workspace-artifact-revision",
            "workspace-message",
            "workspace-source",
            "workspace-action",
            "workspace-email-scope",
            "workspace-papercut",
            "workspace-notification",
        ]
    );
    assert_eq!(subsystem.managed_entities[0].slug, "workspace-subject");
    assert_eq!(
        subsystem.managed_entities[0].primary_key,
        ["workspaceId", "subjectId"]
    );
    assert_eq!(subsystem.email_handlers.len(), 1);
    assert_eq!(subsystem.email_handlers[0].name(), "Workspaces");
}
