//! Notification delivery seam (Pushover in production, fakes in tests).

use futures::future::BoxFuture;
use omni_alerts::{NotificationSender, PushOutcome, Pushover, PushoverChannel, PushoverMessage};

use crate::error::NotifyError;
use crate::platform::NotificationUrlFields;

/// The streamer a notification is about (the app shows it as the sender).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveSender {
    pub streamer_id: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
}

/// One live notification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveMessage {
    pub title: String,
    pub message: String,
    pub url: Option<NotificationUrlFields>,
    /// Set for streamer notifications (went-live, title change, went-offline,
    /// viewer records); `None` for fleet alerts.
    pub sender: Option<LiveSender>,
    /// `live:<streamerId>` for streamer notifications.
    pub thread_id: Option<String>,
}

impl LiveMessage {
    /// A fleet notification with no sender.
    pub fn plain(title: String, message: String, url: Option<NotificationUrlFields>) -> Self {
        Self {
            title,
            message,
            url,
            sender: None,
            thread_id: None,
        }
    }

    /// A notification about one streamer, threaded per streamer.
    pub fn about(
        sender: LiveSender,
        title: String,
        message: String,
        url: Option<NotificationUrlFields>,
    ) -> Self {
        Self {
            title,
            message,
            url,
            thread_id: Some(format!("live:{}", sender.streamer_id)),
            sender: Some(sender),
        }
    }
}

/// The app widget reloaded by streamer notifications.
pub const ON_AIR_WIDGET: &str = "on-air";

/// Pushover fields for `message`, including the app-only sender, thread and
/// widget refresh hints.
pub fn pushover_message(message: LiveMessage) -> PushoverMessage {
    let (url, url_title) = match message.url {
        Some(fields) => (Some(fields.url), Some(fields.url_title)),
        None => (None, None),
    };
    let refresh_widgets = if message.sender.is_some() {
        vec![ON_AIR_WIDGET.to_owned()]
    } else {
        Vec::new()
    };
    PushoverMessage {
        message: message.message,
        title: Some(message.title),
        url,
        url_title,
        thread_id: message.thread_id,
        sender: message.sender.map(|sender| NotificationSender {
            id: sender.streamer_id,
            name: sender.display_name,
            avatar_url: sender.avatar_url,
        }),
        refresh_widgets,
        ..PushoverMessage::default()
    }
}

/// Sends live notifications. `token: None` uses the default application
/// token (`PUSHOVER_TOKEN`).
pub trait LiveNotifier: Send + Sync {
    fn send<'a>(
        &'a self,
        token: Option<&'a str>,
        message: LiveMessage,
    ) -> BoxFuture<'a, Result<(), NotifyError>>;
}

/// Pushover delivery; `SideEffectMode::Record` is honored by [`Pushover`].
#[derive(Clone)]
pub struct PushoverNotifier {
    pushover: Pushover,
}

impl PushoverNotifier {
    pub fn new(pushover: Pushover) -> Self {
        Self { pushover }
    }
}

impl LiveNotifier for PushoverNotifier {
    fn send<'a>(
        &'a self,
        token: Option<&'a str>,
        message: LiveMessage,
    ) -> BoxFuture<'a, Result<(), NotifyError>> {
        Box::pin(async move {
            let push = pushover_message(message);
            let outcome = match token {
                Some(token) => self.pushover.send_with_token(token, push).await,
                None => self.pushover.send(PushoverChannel::General, push).await,
            };
            match outcome {
                Ok(PushOutcome::SkippedNoToken | PushOutcome::Disabled) => {
                    tracing::debug!(target: "LiveCheckTask", "Pushover not configured; notification skipped");
                    Ok(())
                }
                Ok(PushOutcome::Sent | PushOutcome::Recorded) => Ok(()),
                Err(error) => Err(NotifyError::new(error.to_string())),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamer_messages_carry_the_sender_thread_and_on_air_refresh() {
        let message = LiveMessage::about(
            LiveSender {
                streamer_id: "destiny".into(),
                display_name: "Destiny".into(),
                avatar_url: Some("https://files.kick.com/d.webp".into()),
            },
            "Destiny is LIVE!".into(),
            "title".into(),
            Some(NotificationUrlFields {
                url: "https://kick.com/destiny".into(),
                url_title: "Watch on Kick".into(),
            }),
        );
        let push = pushover_message(message);
        assert_eq!(push.title.as_deref(), Some("Destiny is LIVE!"));
        assert_eq!(push.url.as_deref(), Some("https://kick.com/destiny"));
        assert_eq!(push.url_title.as_deref(), Some("Watch on Kick"));
        assert_eq!(push.thread_id.as_deref(), Some("live:destiny"));
        assert_eq!(
            push.sender,
            Some(NotificationSender {
                id: "destiny".into(),
                name: "Destiny".into(),
                avatar_url: Some("https://files.kick.com/d.webp".into()),
            })
        );
        assert_eq!(push.refresh_widgets, ["on-air"]);
    }

    #[test]
    fn fleet_messages_have_no_app_hints() {
        let push = pushover_message(LiveMessage::plain("Down".into(), "x".into(), None));
        assert_eq!(push.thread_id, None);
        assert_eq!(push.sender, None);
        assert!(push.refresh_widgets.is_empty());
        assert_eq!(push.url, None);
    }
}
