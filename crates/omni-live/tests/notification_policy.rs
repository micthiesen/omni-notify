//! Port of `src/live-check/notificationPolicy.spec.ts`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_api::streamers::StreamerTier::{Background, Primary};
use omni_live::notification_policy::{
    NotificationPermissions, ViewerRecordScope, live_notifications_enabled,
    notification_permissions,
};

#[test]
fn defaults_to_enabled_when_live_notifications_is_undefined() {
    assert!(live_notifications_enabled(None, Primary));
}

#[test]
fn is_enabled_when_live_notifications_is_explicitly_true() {
    assert!(live_notifications_enabled(Some(true), Primary));
}

#[test]
fn is_disabled_when_live_notifications_is_false() {
    assert!(!live_notifications_enabled(Some(false), Primary));
}

#[test]
fn permits_all_live_activity_notifications_for_a_default_streamer() {
    assert_eq!(
        notification_permissions(None, Primary, true),
        NotificationPermissions {
            went_live: true,
            title_change: true,
            went_offline: true,
            viewer_records: ViewerRecordScope::All,
        }
    );
}

#[test]
fn mutes_went_live_title_change_and_went_offline_when_live_notifications_is_false() {
    assert_eq!(
        notification_permissions(Some(false), Primary, true),
        NotificationPermissions {
            went_live: false,
            title_change: false,
            went_offline: false,
            viewer_records: ViewerRecordScope::All,
        }
    );
}

#[test]
fn still_permits_viewer_record_notifications_all_windows_for_muted_streamers() {
    assert_eq!(
        notification_permissions(Some(false), Primary, true).viewer_records,
        ViewerRecordScope::All
    );
}

#[test]
fn suppresses_went_offline_when_offline_notifications_is_disabled_globally() {
    let permissions = notification_permissions(None, Primary, false);
    assert!(!permissions.went_offline);
    assert!(permissions.went_live);
    assert!(permissions.title_change);
    assert_eq!(permissions.viewer_records, ViewerRecordScope::All);
}

#[test]
fn mutes_all_live_activity_notifications_for_the_background_tier() {
    assert_eq!(
        notification_permissions(None, Background, true),
        NotificationPermissions {
            went_live: false,
            title_change: false,
            went_offline: false,
            viewer_records: ViewerRecordScope::AllTimeOnly,
        }
    );
}

#[test]
fn restricts_viewer_records_to_all_time_only_for_the_background_tier() {
    assert_eq!(
        notification_permissions(None, Background, true).viewer_records,
        ViewerRecordScope::AllTimeOnly
    );
}

#[test]
fn keeps_all_window_viewer_records_for_the_primary_tier() {
    assert_eq!(
        notification_permissions(None, Primary, true).viewer_records,
        ViewerRecordScope::All
    );
}

#[test]
fn is_disabled_for_the_background_tier_even_without_live_notifications_set() {
    assert!(!live_notifications_enabled(None, Background));
}

#[test]
fn is_enabled_for_the_primary_tier_by_default() {
    assert!(live_notifications_enabled(None, Primary));
}
