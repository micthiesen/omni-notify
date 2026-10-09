//! Viewer metrics: daily peak buckets, all-time records and record
//! notifications (`metrics/*`).

mod persistence;
mod service;
mod windows;

pub use persistence::{
    PlatformViewerMetrics, ViewerMetrics, get_platform_viewer_metrics, get_viewer_metrics,
    record_platform_viewer_count,
};
pub use service::{ConfirmedPeak, PendingPeak, ViewerMetricsService, ViewerObservation};
pub use windows::{
    DailyBucket, MetricWindow, WindowConfig, calculate_window_max, prune_buckets, to_date_stamp,
    update_daily_bucket,
};
