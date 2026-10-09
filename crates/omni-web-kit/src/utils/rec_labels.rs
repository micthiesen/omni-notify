//! Recommendation status, watchlist and feedback labels.

use omni_api::media::{RecommendationFeedback, RecommendationStatus, WatchlistResult};
use omni_api::podcasts::{PodcastFeedback, PodcastRecommendationStatus};

pub fn rec_status_label(status: RecommendationStatus) -> &'static str {
    match status {
        RecommendationStatus::Pending => "Pending",
        RecommendationStatus::Notified => "Notified",
        RecommendationStatus::Watched => "Watched",
        RecommendationStatus::Abandoned => "Abandoned",
        RecommendationStatus::Ignored => "Ignored",
        RecommendationStatus::Failed => "Failed",
    }
}

pub const REC_STATUS_ORDER: [RecommendationStatus; 6] = [
    RecommendationStatus::Notified,
    RecommendationStatus::Pending,
    RecommendationStatus::Watched,
    RecommendationStatus::Abandoned,
    RecommendationStatus::Ignored,
    RecommendationStatus::Failed,
];

pub fn watchlist_label(result: WatchlistResult) -> &'static str {
    match result {
        WatchlistResult::Added => "Added to Watchlist",
        WatchlistResult::AlreadyExists => "Already on Watchlist",
        WatchlistResult::Available => "Available in Plex",
        WatchlistResult::Error => "Watchlist Error",
    }
}

pub const REC_FEEDBACK_ACTIONS: [(RecommendationFeedback, &str); 3] = [
    (RecommendationFeedback::GoodPick, "Good Pick"),
    (RecommendationFeedback::NotForMe, "Not for Me"),
    (RecommendationFeedback::AlreadyWatched, "Already Watched"),
];

pub fn podcast_status_label(status: PodcastRecommendationStatus) -> &'static str {
    match status {
        PodcastRecommendationStatus::Pending => "Pending",
        PodcastRecommendationStatus::Notified => "Notified",
        PodcastRecommendationStatus::Listened => "Listened",
        PodcastRecommendationStatus::Abandoned => "Abandoned",
        PodcastRecommendationStatus::Ignored => "Ignored",
        PodcastRecommendationStatus::Failed => "Failed",
    }
}

/// The serialized value (CSS suffixes and filter keys).
pub fn podcast_status_str(status: PodcastRecommendationStatus) -> &'static str {
    match status {
        PodcastRecommendationStatus::Pending => "pending",
        PodcastRecommendationStatus::Notified => "notified",
        PodcastRecommendationStatus::Listened => "listened",
        PodcastRecommendationStatus::Abandoned => "abandoned",
        PodcastRecommendationStatus::Ignored => "ignored",
        PodcastRecommendationStatus::Failed => "failed",
    }
}

pub const PODCAST_STATUS_ORDER: [PodcastRecommendationStatus; 6] = [
    PodcastRecommendationStatus::Notified,
    PodcastRecommendationStatus::Pending,
    PodcastRecommendationStatus::Listened,
    PodcastRecommendationStatus::Abandoned,
    PodcastRecommendationStatus::Ignored,
    PodcastRecommendationStatus::Failed,
];

pub const PODCAST_FEEDBACK_ACTIONS: [(PodcastFeedback, &str); 2] = [
    (PodcastFeedback::GoodPick, "Good Pick"),
    (PodcastFeedback::NotForMe, "Not for Me"),
];
