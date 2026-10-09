//! Shared components (`frontend/src/components/*` except the WP16 pair).

pub mod activity_feed;
pub mod badges;
pub mod email_log_modal;
pub mod image_with_fallback;
pub mod live_now;
pub mod log_viewer;
pub mod mcp_badges;
pub mod nav_bar;
pub mod on_deck;
pub mod platform_icon;
pub mod section_nav;
pub mod show_more;
pub mod stat_strip;
pub mod status_filter_chips;
pub mod task_card;
pub mod toast;

pub use activity_feed::ActivityFeed;
pub use badges::{StatusDot, TriggerBadge};
pub use email_log_modal::EmailLogModal;
pub use image_with_fallback::ImageWithFallback;
pub use live_now::{DggPresenceTag, LiveNow};
pub use log_viewer::{LogLines, LogViewer};
pub use mcp_badges::{CallStatusPill, PolicyBadge};
pub use nav_bar::NavBar;
pub use on_deck::OnDeck;
pub use platform_icon::PlatformIcon;
pub use section_nav::SectionNav;
pub use show_more::{ShowMore, ShowMoreButton, use_show_more};
pub use stat_strip::StatStrip;
pub use status_filter_chips::StatusFilterChips;
pub use task_card::TaskCard;
pub use toast::{Toast, ToastHandle, ToastKind, ToastState, use_toast};
