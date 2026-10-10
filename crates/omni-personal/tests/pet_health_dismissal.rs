//! Dismissing pet health findings: a dismissal hides the finding for the
//! current episode only, a new episode or a repeat push raises it again, and
//! it never changes the pushes themselves.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use futures::future::BoxFuture;
use omni_api::pets::PetHealthKind;
use omni_core::clock::TestClock;
use omni_mcp_kit::{ToolContext, ToolOutput};
use omni_personal::pets::alerts::{HealthLedger, HealthNotifier};
use omni_personal::pets::health::{Assessment, DAY_MS, Notice, Signal};
use omni_personal::pets::persistence::{PetRow, PetStore, WeightHistoryRow};
use omni_personal::reset_alerts::NotifyError;
use omni_testkit::{TestApp, TestStore, test_clock};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

const NOW: i64 = 1_791_000_000_000;
const DROP_90D: PetHealthKind = PetHealthKind::WeightDrop90d;

#[derive(Default)]
struct FakeNotifier {
    sent: Mutex<Vec<Notice>>,
}

impl HealthNotifier for FakeNotifier {
    fn notify<'a>(&'a self, notice: &'a Notice) -> BoxFuture<'a, Result<(), NotifyError>> {
        self.sent.lock().unwrap().push(notice.clone());
        Box::pin(async { Ok(()) })
    }
}

fn assessment(signal: Signal) -> Assessment {
    Assessment {
        pet_id: "PET-1".into(),
        kind: DROP_90D,
        signal,
    }
}

fn tripped(value: f64) -> Assessment {
    assessment(Signal::Tripped {
        value,
        title: format!("down {value}"),
        message: format!("Sandy {value}"),
    })
}

fn clear() -> Assessment {
    assessment(Signal::Clear { recovery: None })
}

async fn is_dismissed(ledger: &HealthLedger) -> bool {
    ledger
        .dismissed()
        .await
        .unwrap()
        .contains_key(&("PET-1".to_owned(), DROP_90D))
}

#[tokio::test(start_paused = true)]
async fn a_dismissal_lasts_until_the_episode_clears_without_pushover() {
    let clock: Arc<TestClock> = test_clock(NOW);
    let store = TestStore::new(clock).await;
    let ledger = HealthLedger::new(store.store.clone(), None);
    ledger.apply(&[tripped(6.0)], NOW).await.unwrap();
    assert!(ledger.all().await.unwrap().is_empty());

    ledger.dismiss("PET-1", DROP_90D, NOW).await.unwrap();
    assert_eq!(
        ledger
            .dismissed()
            .await
            .unwrap()
            .get(&("PET-1".to_owned(), DROP_90D)),
        Some(&NOW)
    );
    // Later passes of the same episode keep it dismissed.
    for day in 1..40 {
        let pass = [tripped(6.0 + f64::from(day) / 10.0)];
        ledger
            .apply(&pass, NOW + i64::from(day) * DAY_MS)
            .await
            .unwrap();
        assert_eq!(ledger.clear_ended_dismissals(&pass).await.unwrap(), 0);
    }
    assert!(is_dismissed(&ledger).await);
    // Between thresholds the episode holds.
    let hold = [assessment(Signal::Hold)];
    assert_eq!(ledger.clear_ended_dismissals(&hold).await.unwrap(), 0);
    assert!(is_dismissed(&ledger).await);

    // The episode ends: the next one starts undismissed.
    assert_eq!(ledger.clear_ended_dismissals(&[clear()]).await.unwrap(), 1);
    assert!(!is_dismissed(&ledger).await);
    ledger
        .clear_ended_dismissals(&[tripped(6.0)])
        .await
        .unwrap();
    assert!(!is_dismissed(&ledger).await);
}

#[tokio::test(start_paused = true)]
async fn a_repeat_push_or_a_new_episode_re_raises_a_dismissed_finding() {
    let clock: Arc<TestClock> = test_clock(NOW);
    let store = TestStore::new(clock).await;
    let notifier = Arc::new(FakeNotifier::default());
    let ledger = HealthLedger::new(
        store.store.clone(),
        Some(notifier.clone() as Arc<dyn HealthNotifier>),
    );
    assert_eq!(ledger.apply(&[tripped(6.0)], NOW).await.unwrap(), 1);
    ledger
        .dismiss("PET-1", DROP_90D, NOW + DAY_MS)
        .await
        .unwrap();
    assert!(is_dismissed(&ledger).await);

    // Four weeks on but not 2 points worse: no push, still dismissed.
    assert_eq!(
        ledger
            .apply(&[tripped(7.5)], NOW + 29 * DAY_MS)
            .await
            .unwrap(),
        0
    );
    assert!(is_dismissed(&ledger).await);
    // Worse by 2 points: the repeat push goes out as it would have, and the
    // finding needs attention again.
    assert_eq!(
        ledger
            .apply(&[tripped(8.5)], NOW + 30 * DAY_MS)
            .await
            .unwrap(),
        1
    );
    assert!(!is_dismissed(&ledger).await);
    assert_eq!(
        ledger
            .clear_ended_dismissals(&[tripped(8.5)])
            .await
            .unwrap(),
        1
    );

    // Dismissed again, then the episode ends and a new one trips before any
    // PetTracker cleanup ran: the new episode is not dismissed.
    ledger
        .dismiss("PET-1", DROP_90D, NOW + 31 * DAY_MS)
        .await
        .unwrap();
    ledger.apply(&[clear()], NOW + 40 * DAY_MS).await.unwrap();
    assert!(is_dismissed(&ledger).await);
    ledger
        .apply(&[tripped(5.5)], NOW + 41 * DAY_MS)
        .await
        .unwrap();
    assert!(!is_dismissed(&ledger).await);

    let titles: Vec<String> = notifier
        .sent
        .lock()
        .unwrap()
        .iter()
        .map(|n| n.title().to_owned())
        .collect();
    assert_eq!(titles, ["down 6", "down 8.5", "down 5.5"]);
}

/// Three readings a day for 100 days ending an hour before the test epoch
/// (2026-01-01T00:00Z): 14 lb until 60 days ago, 13 lb since, a 7% drop over
/// 90 days that does not trip the two-week rule.
async fn seed_decline(app: &TestApp) {
    let pets = PetStore::open(&app.ctx.store).await.unwrap();
    pets.upsert_pet(&PetRow {
        pet_id: "PET-1".into(),
        name: "Sandy".into(),
        current_weight: 13.0,
        updated_at: "2025-12-31T23:00:00.000Z".into(),
    })
    .await
    .unwrap();
    let end = jiff::civil::date(2025, 12, 31).at(23, 0, 0, 0);
    for i in 0..300 {
        let at = end - jiff::SignedDuration::from_hours(8 * i);
        pets.insert_weight_reading(&WeightHistoryRow {
            pet_id: "PET-1".into(),
            timestamp: at.to_string(),
            weight: if i > 180 { 14.0 } else { 13.0 },
        })
        .await
        .unwrap();
    }
}

fn finding<'a>(health: &'a Value, kind: &str) -> &'a Value {
    health["pets"][0]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == kind)
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn the_pets_page_dismisses_and_restores_a_tripped_finding() {
    let app = TestApp::new().await;
    seed_decline(&app).await;
    let subsystem = omni_personal::subsystem(&app.ctx).await.unwrap();
    let router = app.router(&subsystem);

    let (status, health) = app.get_json(&router, "/api/pets/health").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        finding(&health, "weight-drop-90d")
            .get("dismissedAt")
            .is_none()
    );

    let request = json!({"petId": "PET-1", "kind": "weight-drop-90d"});
    let (status, body) = app
        .post_json(&router, "/api/pets/health/dismiss", &request)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"petId": "PET-1", "kind": "weight-drop-90d", "dismissedAt": "2026-01-01T00:00:00.000Z"})
    );
    let (_, health) = app.get_json(&router, "/api/pets/health").await;
    assert_eq!(
        finding(&health, "weight-drop-90d")["dismissedAt"],
        "2026-01-01T00:00:00.000Z"
    );

    // Agents see the same state (and the output schema accepts it).
    let tool = subsystem
        .mcp_tools
        .iter()
        .find(|t| t.meta.name == "pets_read")
        .unwrap();
    let cx = ToolContext {
        call_id: "1".into(),
        cancel: CancellationToken::new(),
    };
    let ToolOutput::Structured(trend) = tool
        .handler
        .call(json!({"resource": "trend", "weeks": 2}), cx)
        .await
        .unwrap()
    else {
        panic!("structured output expected");
    };
    let trend = Value::Object(trend);
    assert_eq!(
        finding(&trend, "weight-drop-90d")["dismissedAt"],
        "2026-01-01T00:00:00.000Z"
    );

    // Only a tripped per-pet rule can be dismissed.
    for (body, expected) in [
        (
            json!({"petId": "PET-1", "kind": "weight-drop-2w"}),
            StatusCode::CONFLICT,
        ),
        (
            json!({"petId": "PET-2", "kind": "weight-drop-90d"}),
            StatusCode::NOT_FOUND,
        ),
        (
            json!({"petId": "*", "kind": "data-gap"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"petId": "PET-1", "kind": "weight"}),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (status, _) = app
            .post_json(&router, "/api/pets/health/dismiss", &body)
            .await;
        assert_eq!(status, expected, "{body}");
    }

    let (status, body) = app
        .post_json(&router, "/api/pets/health/restore", &request)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["dismissedAt"], Value::Null);
    let (_, health) = app.get_json(&router, "/api/pets/health").await;
    assert!(
        finding(&health, "weight-drop-90d")
            .get("dismissedAt")
            .is_none()
    );
    // Dismissing never pushes.
    assert!(app.pushes.all().is_empty());
}
