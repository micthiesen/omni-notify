//! The `--preview` fixture data decodes through every typed model and shows
//! up in the dashboard routes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::http::StatusCode;
use omni_testkit::TestApp;

#[tokio::test]
async fn seeds_typed_fixture_data_the_routes_serve() {
    let app = TestApp::new().await;
    let mut wired = omni_notify::wiring::wire(&app.ctx, 0).await.unwrap();
    let now = app.ctx.clock.now_ms();
    omni_notify::preview::seed(&app.ctx, now).await.unwrap();
    for task in omni_notify::preview::fake_tasks().unwrap() {
        app.ctx.tasks.track(task).unwrap();
    }
    let (router, _) = omni_notify::boot::app_router(
        &app.ctx,
        &mut wired.subsystems,
        std::path::Path::new("/nonexistent"),
    );
    let (status, snapshot) = app.get_json(&router, "/api/snapshot").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(snapshot["tasks"].as_array().unwrap().len(), 6);
    let runs = snapshot["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 18);
    assert!(runs.iter().any(|run| run["status"] == "degraded"));
    assert_eq!(snapshot["onDeck"].as_array().unwrap().len(), 3);
    let (_, recs) = app.get_json(&router, "/api/recommendations").await;
    assert_eq!(recs["recommendations"].as_array().unwrap().len(), 4);
    let (_, pets) = app.get_json(&router, "/api/pets").await;
    assert_eq!(pets[0]["name"], "Mochi");
    assert_eq!(pets[0]["weightHistory"].as_array().unwrap().len(), 91);
    let (_, briefings) = app.get_json(&router, "/api/briefings").await;
    assert!(briefings.to_string().contains("Chip exports tighten again"));
    let (_, activity) = app.get_json(&router, "/api/email-activity").await;
    assert_eq!(activity["activities"].as_array().unwrap().len(), 1);
}

#[test]
fn channels_config_builds_four_streamers() {
    let config = omni_notify::preview::channels_json();
    assert_eq!(config.as_object().unwrap().len(), 4);
    assert_eq!(config["LoopStation"]["tier"], "background");
}
