//! The app push route: tried before Pushover, Pushover only when the app
//! does not route the message, and an uncertain app delivery never falls back.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex, RwLock};

use futures::future::BoxFuture;
use omni_alerts::{
    AlertGate, AlertLayer, AppDelivery, AppPushRoute, PushOutcome, Pushover, PushoverChannel,
    PushoverMessage,
};
use omni_core::clock::TestClock;
use omni_http::{HttpClient, HttpConfig, SideEffectMode};
use tracing_subscriber::layer::SubscriberExt as _;

struct ScriptedRoute {
    reply: AppDelivery,
    seen: Mutex<Vec<(PushoverChannel, PushoverMessage)>>,
}

impl ScriptedRoute {
    fn new(reply: AppDelivery) -> Arc<Self> {
        Arc::new(Self {
            reply,
            seen: Mutex::default(),
        })
    }

    fn seen(&self) -> Vec<(PushoverChannel, PushoverMessage)> {
        self.seen.lock().unwrap().clone()
    }
}

impl AppPushRoute for ScriptedRoute {
    fn deliver<'a>(
        &'a self,
        channel: PushoverChannel,
        message: &'a PushoverMessage,
    ) -> BoxFuture<'a, AppDelivery> {
        self.seen.lock().unwrap().push((channel, message.clone()));
        let reply = self.reply.clone();
        Box::pin(async move { reply })
    }
}

fn recording(user: Option<&str>) -> Pushover {
    Pushover::with_credentials(
        HttpClient::new(HttpConfig {
            connect_timeout: None,
            offline: true,
        })
        .unwrap(),
        user.map(str::to_owned),
        [(PushoverChannel::General, "general".to_owned())],
        SideEffectMode::Record,
    )
}

fn message() -> PushoverMessage {
    PushoverMessage {
        message: "body".to_owned(),
        title: Some("Title".to_owned()),
        ..PushoverMessage::default()
    }
}

#[tokio::test]
async fn a_delivered_app_push_skips_pushover() {
    let pushover = recording(Some("user"));
    let route = ScriptedRoute::new(AppDelivery::Delivered);
    pushover.set_app_route(route.clone()).unwrap();
    let outcome = pushover
        .send(PushoverChannel::Calendar, message())
        .await
        .unwrap();
    assert_eq!(outcome, PushOutcome::Sent);
    assert!(pushover.recorded().is_empty());
    assert_eq!(route.seen()[0].0, PushoverChannel::Calendar);
}

#[tokio::test]
async fn an_unrouted_message_falls_back_to_pushover_unchanged() {
    let pushover = recording(Some("user"));
    pushover
        .set_app_route(ScriptedRoute::new(AppDelivery::NotRouted))
        .unwrap();
    let outcome = pushover
        .send(PushoverChannel::General, message())
        .await
        .unwrap();
    assert_eq!(outcome, PushOutcome::Recorded);
    assert_eq!(pushover.recorded()[0].message, message());
    // No Calendar token: unrouted keeps the Pushover outcome.
    assert_eq!(
        pushover
            .send(PushoverChannel::Calendar, message())
            .await
            .unwrap(),
        PushOutcome::SkippedNoToken
    );
}

#[tokio::test]
async fn an_uncertain_app_push_is_an_uncertain_error_without_fallback() {
    let pushover = recording(Some("user"));
    pushover
        .set_app_route(ScriptedRoute::new(AppDelivery::Uncertain(
            "timed out".into(),
        )))
        .unwrap();
    let error = pushover
        .send(PushoverChannel::General, message())
        .await
        .unwrap_err();
    assert_eq!(error.status, None);
    assert!(!error.is_definite_rejection());
    assert!(error.to_string().contains("timed out"), "{error}");
    assert!(pushover.recorded().is_empty());
}

#[tokio::test]
async fn the_route_is_tried_without_pushover_credentials_and_for_streamer_tokens() {
    let pushover = recording(None);
    let route = ScriptedRoute::new(AppDelivery::Delivered);
    pushover.set_app_route(route.clone()).unwrap();
    assert!(pushover.has_app_route());
    assert_eq!(
        pushover
            .send(PushoverChannel::Recs, message())
            .await
            .unwrap(),
        PushOutcome::Sent
    );
    assert_eq!(
        pushover
            .send_with_token("streamer-app", message())
            .await
            .unwrap(),
        PushOutcome::Sent
    );
    let channels: Vec<_> = route.seen().into_iter().map(|(ch, _)| ch).collect();
    assert_eq!(channels, vec![PushoverChannel::Recs, PushoverChannel::Live]);
    assert!(pushover.set_app_route(route).is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn error_log_alerts_take_the_app_route() {
    let pushover = recording(Some("user"));
    let route = ScriptedRoute::new(AppDelivery::Delivered);
    pushover.set_app_route(route.clone()).unwrap();
    let gates: Arc<RwLock<Vec<Arc<dyn AlertGate>>>> = Arc::default();
    let (layer, worker) = AlertLayer::new(pushover.clone(), gates, TestClock::new(0));
    let worker = tokio::spawn(worker.run());
    {
        let subscriber = tracing_subscriber::registry().with(layer);
        let _guard = tracing::subscriber::set_default(subscriber);
        tracing::error!(target: "Test", "Boom");
    }
    worker.await.unwrap();
    let seen = route.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, PushoverChannel::General);
    assert_eq!(seen[0].1.title.as_deref(), Some("Error: Boom"));
    assert!(pushover.recorded().is_empty());
}
