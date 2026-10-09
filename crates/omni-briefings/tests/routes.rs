//! `GET /api/briefings` (`src/server.ts` 1641-1668) and the `BriefingsReader` port.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::http::StatusCode;
use omni_briefings::persistence::{
    BriefingNotificationData, add_notification, distribute_run_cost,
};
use omni_testkit::TestApp;
use serde_json::json;

fn note(title: &str, timestamp: i64, run_id: Option<&str>) -> BriefingNotificationData {
    let mut n = BriefingNotificationData::new(
        title.to_owned(),
        format!("{title} body"),
        format!("https://example.com/{title}"),
        timestamp,
    );
    n.run_id = run_id.map(str::to_owned);
    n
}

#[tokio::test]
async fn lists_briefings_newest_first_with_null_defaults() {
    let app = TestApp::new().await;
    let store = &app.ctx.store;
    add_notification(store, "Old", note("o1", 100, None))
        .await
        .unwrap();
    add_notification(store, "New", note("n1", 200, Some("New:r1")))
        .await
        .unwrap();
    add_notification(store, "New", note("n2", 300, Some("New:r2")))
        .await
        .unwrap();
    distribute_run_cost(store, "New", Some("New:r2"), Some(1.5))
        .await
        .unwrap();
    let router = omni_briefings::router(store.clone());
    let (status, body) = app.get_json(&router, "/api/briefings").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"briefings": [
            {"name": "New", "notifications": [
                {"title": "n2", "message": "n2 body", "url": "https://example.com/n2", "timestamp": 300, "runId": "New:r2", "costCents": 1.5},
                {"title": "n1", "message": "n1 body", "url": "https://example.com/n1", "timestamp": 200, "runId": "New:r1", "costCents": null}
            ]},
            {"name": "Old", "notifications": [
                {"title": "o1", "message": "o1 body", "url": "https://example.com/o1", "timestamp": 100, "runId": null, "costCents": null}
            ]}
        ]})
    );

    let reader = omni_briefings::briefings_reader(store.clone());
    let histories = reader.histories().await.unwrap();
    assert_eq!(histories.len(), 2);
    assert_eq!(histories[0]["briefingName"], "New");
    assert_eq!(histories[0]["notifications"][0]["costCents"], 1.5);
    let decoded: omni_api::briefings::BriefingHistory =
        serde_json::from_value(histories[1].clone()).unwrap();
    assert_eq!(decoded.notifications[0].run_id, None);
}
