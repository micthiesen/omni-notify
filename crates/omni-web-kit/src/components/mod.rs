//! Shared components. `docs/design-system.md` defines their behavior;
//! `style/components.css` their look.

pub mod avatar;
pub mod badges;
pub mod button;
pub mod controls;
pub mod email_inspector;
pub mod icon;
pub mod image_with_fallback;
pub mod inspector;
pub mod log_viewer;
pub mod mcp_badges;
pub mod on_deck;
pub mod platform_icon;
pub mod readout;
pub mod run_log;
pub mod show_more;
pub mod states;
pub mod streamers;
pub mod surface;
pub mod task_display;
pub mod toast;
pub mod tone;

pub use avatar::{Avatar, Poster, Presence};
pub use badges::{CountBadge, Delta, LiveTag, Status, StatusDot, Tag, TriggerBadge};
pub use button::{Button, ButtonLink, ButtonSize, ButtonVariant, ConfirmButton, RunButton};
pub use controls::{Chip, Kbd, SearchField, SegOption, Segmented};
pub use email_inspector::EmailInspector;
pub use icon::{Glyph, Icon, IconSize};
pub use image_with_fallback::ImageWithFallback;
pub use inspector::{Inspector, Modal};
pub use log_viewer::{LogLines, LogViewer, LogWell};
pub use mcp_badges::{CallStatusPill, PolicyBadge};
pub use on_deck::{OnDeck, PosterCard};
pub use platform_icon::PlatformIcon;
pub use readout::{
    CellKind, Meter, Readout, ReadoutBand, ReadoutSize, RunCell, RunStrip, Sparkline, TickNum,
    TimeLane,
};
pub use run_log::{RunLog, RunRow};
pub use show_more::{ShowMore, ShowMoreButton, use_show_more};
pub use states::{EmptyState, ErrorState, InlineNote, Skeleton, SkeletonKind, SkeletonRows};
pub use streamers::DggPresenceTag;
pub use surface::{Disclosure, PageHead, Panel, PanelHead, Section};
pub use toast::{
    Toast, ToastHandle, ToastKind, ToastRegion, ToastState, provide_toasts, use_toast,
};
pub use tone::{StatusKind, Tone};
