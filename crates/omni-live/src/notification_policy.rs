//! Which notification kinds may fire for a streamer (`notificationPolicy.ts`).

use omni_api::streamers::StreamerTier;

/// Whether viewer records may notify for every window or only all-time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewerRecordScope {
    All,
    AllTimeOnly,
}

/// The decision for one streamer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotificationPermissions {
    pub went_live: bool,
    pub title_change: bool,
    pub went_offline: bool,
    /// Never muted by `liveNotifications`; the background tier narrows it.
    pub viewer_records: ViewerRecordScope,
}

/// Whether live-activity notifications (went-live, title, went-offline) are
/// enabled: muted by `liveNotifications: false` or the background tier.
pub fn live_notifications_enabled(live_notifications: Option<bool>, tier: StreamerTier) -> bool {
    if tier == StreamerTier::Background {
        return false;
    }
    live_notifications != Some(false)
}

/// Pure decision; went-offline also requires `OFFLINE_NOTIFICATIONS`.
pub fn notification_permissions(
    live_notifications: Option<bool>,
    tier: StreamerTier,
    offline_notifications: bool,
) -> NotificationPermissions {
    let live_activity = live_notifications_enabled(live_notifications, tier);
    NotificationPermissions {
        went_live: live_activity,
        title_change: live_activity,
        went_offline: live_activity && offline_notifications,
        viewer_records: if tier == StreamerTier::Background {
            ViewerRecordScope::AllTimeOnly
        } else {
            ViewerRecordScope::All
        },
    }
}
