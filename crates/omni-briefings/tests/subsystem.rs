//! The briefings subsystem as app wiring receives it.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_testkit::TestApp;

#[tokio::test]
async fn builds_without_briefings_path() {
    let app = TestApp::new().await;
    let subsystem = omni_briefings::subsystem(&app.ctx).unwrap();
    assert_eq!(subsystem.name, "briefings");
    assert!(subsystem.tasks.is_empty());
    let names: Vec<&str> = subsystem.entities.iter().map(|e| e.name).collect();
    assert_eq!(names, ["briefing-history", "briefing-delivery"]);
    assert_eq!(subsystem.managed_entities.len(), 1);
    assert_eq!(subsystem.managed_entities[0].primary_key, ["briefingName"]);
}
