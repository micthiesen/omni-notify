//! App notifications over APNs: the alert payload and the
//! [`AppPushRoute`] that `omni_alerts::Pushover` tries before Pushover.
//!
//! The route sends to every device with alerts enabled and an APNs token.
//! One accepted push delivers the message. When every device answered with a
//! rejection (after one retry of 429/5xx) the message is not routed and
//! Pushover sends it. A push without an APNs answer has an unknown outcome:
//! it is never retried and the message is uncertain, so it is neither
//! resent nor sent through Pushover.
//!
//! ERROR-log alerts travel this route, so nothing here logs at ERROR.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use futures::future::BoxFuture;
use omni_alerts::{AppDelivery, AppPushRoute, PushoverChannel, PushoverMessage};
use omni_core::clock::SharedClock;
use omni_store::Store;
use serde_json::{Map, Value, json};

use crate::apns::{ApnsPush, ApnsPushResult, ApnsSender};
use crate::devices::{IosDevice, clear_device_token, list_devices};

const LOG: &str = "IOSAlerts";
/// APNs rejects alert payloads over 4 KB.
pub const MAX_ALERT_PAYLOAD_BYTES: usize = 4096;
const RETRY_DELAY: Duration = Duration::from_millis(250);
/// Devices pushed concurrently.
const CONCURRENCY: usize = 4;
const ELLIPSIS: char = '\u{2026}';
/// `apns-expiration` offsets: live alerts go stale fast.
const LIVE_ALERT_TTL_SECS: i64 = 60 * 60;
const ALERT_TTL_SECS: i64 = 24 * 60 * 60;

/// `omni.channel` for a Pushover channel.
pub fn channel_name(channel: PushoverChannel) -> &'static str {
    match channel {
        PushoverChannel::General => "general",
        PushoverChannel::Live => "live",
        PushoverChannel::Calendar => "calendar",
        PushoverChannel::Recs => "recs",
        PushoverChannel::Podcast => "podcast",
        PushoverChannel::PressPods => "presspods",
    }
}

fn non_empty(value: Option<&String>) -> Option<&str> {
    value.map(String::as_str).filter(|v| !v.is_empty())
}

fn payload_value(
    channel: PushoverChannel,
    message: &PushoverMessage,
    body: &str,
    title: Option<&str>,
) -> Value {
    let url = non_empty(message.url.as_ref());
    let mut alert = Map::new();
    if let Some(title) = title {
        alert.insert("title".to_owned(), Value::from(title));
    }
    alert.insert("body".to_owned(), Value::from(body));
    let thread_id = non_empty(message.thread_id.as_ref()).unwrap_or(channel_name(channel));
    let sender = message.sender.as_ref().map(|sender| {
        json!({
            "id": sender.id,
            "name": sender.name,
            "avatarUrl": sender.avatar_url,
        })
    });
    json!({
        "aps": {
            "alert": alert,
            "sound": "default",
            "thread-id": thread_id,
            "mutable-content": 1,
            "category": if url.is_some() { "OMNI_LINK" } else { "OMNI_PLAIN" },
        },
        "omni": {
            "channel": channel_name(channel),
            "url": url,
            "urlTitle": url.and(non_empty(message.url_title.as_ref())),
            "sender": sender,
            "refreshWidgets": message.refresh_widgets,
        }
    })
}

fn encoded(value: &Value) -> String {
    // Serializing a `Value` cannot fail.
    serde_json::to_string(value).unwrap_or_default()
}

/// The longest prefix of `text` (on a char boundary, with an ellipsis when
/// cut) for which `fits` holds; `fits` must be monotonic in length.
fn truncate_to_fit(text: &str, fits: impl Fn(&str) -> bool) -> String {
    if fits(text) {
        return text.to_owned();
    }
    if text.is_empty() {
        return String::new();
    }
    let boundaries: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let candidate = |count: usize| {
        let mut cut = text[..boundaries[count]].trim_end().to_owned();
        cut.push(ELLIPSIS);
        cut
    };
    // Largest char count `n` in 0..len whose candidate fits.
    let (mut low, mut high) = (0usize, boundaries.len());
    if !fits(&candidate(0)) {
        return String::new();
    }
    while high - low > 1 {
        let mid = low + (high - low) / 2;
        if fits(&candidate(mid)) {
            low = mid;
        } else {
            high = mid;
        }
    }
    candidate(low)
}

/// The `alert` push payload for `message`, at most
/// [`MAX_ALERT_PAYLOAD_BYTES`]: the body is cut first (UTF-8 safe), then the
/// title; a payload still too large drops the sender and link.
pub fn alert_payload(channel: PushoverChannel, message: &PushoverMessage) -> String {
    let title = non_empty(message.title.as_ref());
    let fits_with = |m: &PushoverMessage, body: &str, title: Option<&str>| {
        encoded(&payload_value(channel, m, body, title)).len() <= MAX_ALERT_PAYLOAD_BYTES
    };
    let mut m = message.clone();
    let mut body = truncate_to_fit(&message.message, |b| fits_with(&m, b, title));
    let mut title = title.map(str::to_owned);
    if !fits_with(&m, &body, title.as_deref()) {
        title = title.map(|t| truncate_to_fit(&t, |t| fits_with(&m, &body, Some(t))));
    }
    if !fits_with(&m, &body, title.as_deref()) {
        m.sender = None;
        m.url = None;
        m.url_title = None;
        m.thread_id = None;
        m.refresh_widgets.clear();
        body = truncate_to_fit(&message.message, |b| fits_with(&m, b, title.as_deref()));
        if !fits_with(&m, &body, title.as_deref()) {
            title = title.map(|t| truncate_to_fit(&t, |t| fits_with(&m, &body, Some(t))));
        }
    }
    encoded(&payload_value(channel, &m, &body, title.as_deref()))
}

/// One device's result.
enum DeviceOutcome {
    Sent,
    Rejected(String),
    Uncertain(String),
}

/// The [`AppPushRoute`] over registered alert devices.
pub struct AppAlertRoute {
    store: Store,
    apns: Option<Arc<dyn ApnsSender>>,
    clock: SharedClock,
}

impl AppAlertRoute {
    pub fn new(store: Store, apns: Option<Arc<dyn ApnsSender>>, clock: SharedClock) -> Self {
        Self { store, apns, clock }
    }

    /// `apns-expiration` for `channel`: a go-live is stale after an hour,
    /// anything else is worth a day of store-and-forward.
    fn expiration(&self, channel: PushoverChannel) -> i64 {
        let ttl = match channel {
            PushoverChannel::Live => LIVE_ALERT_TTL_SECS,
            _ => ALERT_TTL_SECS,
        };
        self.clock.now_ms().div_euclid(1_000) + ttl
    }

    async fn route(&self, channel: PushoverChannel, message: &PushoverMessage) -> AppDelivery {
        let Some(apns) = self.apns.as_ref() else {
            return AppDelivery::NotRouted;
        };
        let devices = match list_devices(&self.store).await {
            Ok(devices) => devices,
            Err(error) => {
                tracing::warn!(target: LOG, "App alert devices unreadable, using Pushover: {error}");
                return AppDelivery::NotRouted;
            }
        };
        let devices: Vec<IosDevice> = devices
            .into_iter()
            .filter(IosDevice::receives_alerts)
            .collect();
        if devices.is_empty() {
            return AppDelivery::NotRouted;
        }
        let payload = alert_payload(channel, message);
        let expiration = self.expiration(channel);
        let outcomes: Vec<DeviceOutcome> = futures::stream::iter(devices)
            .map(|device| {
                let payload = payload.clone();
                async move { self.push(apns.as_ref(), &device, payload, expiration).await }
            })
            .buffer_unordered(CONCURRENCY)
            .collect()
            .await;
        combine(outcomes)
    }

    async fn push(
        &self,
        apns: &dyn ApnsSender,
        device: &IosDevice,
        payload: String,
        expiration: i64,
    ) -> DeviceOutcome {
        let Some(token) = device.apns_token.as_deref() else {
            return DeviceOutcome::Rejected("no APNs token".to_owned());
        };
        let push = ApnsPush::alert(token, device.environment, payload, expiration);
        for attempt in 0..2 {
            if attempt > 0 {
                tokio::time::sleep(RETRY_DELAY).await;
            }
            match apns.send_push(&push).await {
                Ok(ApnsPushResult::Sent) => return DeviceOutcome::Sent,
                Ok(ApnsPushResult::InvalidToken { reason }) => {
                    if let Err(error) =
                        clear_device_token(&self.store, &device.device_id, token).await
                    {
                        tracing::warn!(target: LOG, "Failed to clear a stale alert token: {error}");
                    } else {
                        tracing::info!(target: LOG, "Cleared a stale alert token for {}: {reason}", device.name);
                    }
                    return DeviceOutcome::Rejected(format!("{}: {reason}", device.name));
                }
                Ok(ApnsPushResult::Failed { status, reason }) => {
                    let retryable = status == 429 || status >= 500;
                    if !retryable || attempt > 0 {
                        return DeviceOutcome::Rejected(format!(
                            "{}: {status} {reason}",
                            device.name
                        ));
                    }
                }
                // Nothing left Omni (bad key, URL, guard): certainly not delivered.
                Err(error) if error.not_sent => {
                    return DeviceOutcome::Rejected(format!("{}: {error}", device.name));
                }
                // No answer: the device may have the alert. Never resend.
                Err(error) => return DeviceOutcome::Uncertain(format!("{}: {error}", device.name)),
            }
        }
        DeviceOutcome::Rejected(format!("{}: retry exhausted", device.name))
    }
}

fn combine(outcomes: Vec<DeviceOutcome>) -> AppDelivery {
    let mut rejected = Vec::new();
    let mut uncertain = Vec::new();
    for outcome in outcomes {
        match outcome {
            DeviceOutcome::Sent => return AppDelivery::Delivered,
            DeviceOutcome::Rejected(reason) => rejected.push(reason),
            DeviceOutcome::Uncertain(reason) => uncertain.push(reason),
        }
    }
    if !uncertain.is_empty() {
        let detail = uncertain.join("; ");
        tracing::warn!(target: LOG, "App alert outcome unknown, not resending: {detail}");
        return AppDelivery::Uncertain(detail);
    }
    tracing::warn!(target: LOG, "Every alert device rejected the push, using Pushover: {}", rejected.join("; "));
    AppDelivery::NotRouted
}

impl AppPushRoute for AppAlertRoute {
    fn deliver<'a>(
        &'a self,
        channel: PushoverChannel,
        message: &'a PushoverMessage,
    ) -> BoxFuture<'a, AppDelivery> {
        Box::pin(self.route(channel, message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_keeps_char_boundaries_and_marks_the_cut() {
        let text = "ééééé";
        let cut = truncate_to_fit(text, |t| t.len() <= 7);
        assert_eq!(cut, "éé\u{2026}");
        assert_eq!(truncate_to_fit("short", |_| true), "short");
        assert_eq!(truncate_to_fit("x", |t| t.is_empty()), "");
        assert_eq!(truncate_to_fit("", |_| false), "");
    }
}
