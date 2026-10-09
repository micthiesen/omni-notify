//! Omni Notify domain pages.
//!
//! `omni-web`'s router mounts exactly these exports; their names and props
//! are this crate's contract with the router:
//!
//! | Export | Route | Props |
//! |---|---|---|
//! | [`MediaPage`] | `/media` (and `/recommendations`) | none |
//! | [`MediaDetailPage`] | `/media/:id` | `id` |
//! | [`PodcastsPage`] | `/podcasts` | none |
//! | [`PodcastDetailPage`] | `/podcasts/:id` | `id` |
//! | [`FeedbackPage`] | `/feedback/(recommendations\|podcasts)/:id` | `kind`, `id` |
//! | [`PodsPage`] | `/pods` | none |
//! | [`PodsDetailPage`] | `/pods/:id` | `id` |
//! | [`WorkspacesPage`] | `/workspaces[/:w[/:s]]` | `workspace_id`, `subject_id` |
//! | [`PetsPage`], [`RemindersPage`], [`BriefingsPage`], [`McpPage`], [`ClaudePage`] | `/pets`, `/reminders`, `/briefings`, `/mcp-activity`, `/claude` | none |
//!
//! Path parameters arrive already percent-decoded. Each detail page is
//! re-created when its id changes; `WorkspacesPage` stays mounted and receives
//! its ids as signals (as the React page received new props).

mod briefings;
mod claude;
mod common;
mod feedback;
mod mcp;
mod media;
pub mod pets;
mod podcasts;
mod pods;
mod pods_detail;
mod rec_ui;
mod recommendation_runs;
pub mod reminders;
mod research_nav;
mod taste_brain;
mod workspaces;

pub use briefings::BriefingsPage;
pub use claude::ClaudePage;
pub use feedback::{FeedbackKind, FeedbackPage};
pub use mcp::McpPage;
pub use media::{MediaDetailPage, MediaPage};
pub use pets::PetsPage;
pub use podcasts::{PodcastDetailPage, PodcastsPage};
pub use pods::{PodsPage, format_audio_duration};
pub use pods_detail::PodsDetailPage;
pub use recommendation_runs::RecommendationRuns;
pub use reminders::RemindersPage;
pub use taste_brain::{TasteBrain, TasteBrainProfile, TasteClaimView};
pub use workspaces::WorkspacesPage;
