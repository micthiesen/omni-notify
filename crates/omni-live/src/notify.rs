//! Notification delivery seam (Pushover in production, fakes in tests).

use futures::future::BoxFuture;
use omni_alerts::{PushOutcome, Pushover, PushoverChannel, PushoverMessage};

use crate::error::NotifyError;
use crate::platform::NotificationUrlFields;

/// One live notification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveMessage {
    pub title: String,
    pub message: String,
    pub url: Option<NotificationUrlFields>,
}

/// Sends live notifications. `token: None` uses the default application
/// token (`PUSHOVER_TOKEN`), as mitools `notify` does without a token.
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
            let (url, url_title) = match message.url {
                Some(fields) => (Some(fields.url), Some(fields.url_title)),
                None => (None, None),
            };
            let push = PushoverMessage {
                message: message.message,
                title: Some(message.title),
                url,
                url_title,
                ..PushoverMessage::default()
            };
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
