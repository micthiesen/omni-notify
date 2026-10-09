//! Viewer-surge detection and relevance scoring.
//!
//! A surge compares the current count against a baseline drawn from samples
//! 5-20 minutes old. The baseline must be flat (not still climbing after
//! go-live), a platform surge must also reach the typical session peak, a
//! sparse baseline (after a restart or primary switch) suppresses, and two
//! consecutive candidate observations are required. State is in memory.

use std::collections::HashMap;

use omni_core::js::{math_round, number_to_string};

use crate::js_math::{js_max, js_min};
use crate::observation::{Streamer, StreamerTier};
use crate::types::{SemanticMetadata, StreamSession, ViewerTrend};

#[derive(Clone, Copy, Debug, PartialEq)]
struct ViewerSample {
    at: i64,
    viewers: Option<f64>,
    dgg_viewers: Option<f64>,
}

const SAMPLE_WINDOW_MS: i64 = 20 * 60 * 1000;
const MIN_BASELINE_AGE_MS: i64 = 5 * 60 * 1000;
const MIN_SESSION_AGE_MS: i64 = 20 * 60 * 1000;
const MIN_BASELINE_SAMPLES: usize = 8;
const MAX_BASELINE_CLIMB: f64 = 0.15;
const MIN_HALF_SAMPLES: usize = 3;
const SURGE_CONFIRMATION_OBSERVATIONS: u32 = 2;
const VIEWER_SURGE_PERCENT: f64 = 50.0;
const MIN_VIEWER_SURGE_GAIN: f64 = 100.0;
const DGG_SURGE_PERCENT: f64 = 100.0;
const MIN_DGG_SURGE_GAIN: f64 = 30.0;

const TYPICAL_PEAK_SESSIONS: usize = 10;
const MIN_TYPICAL_PEAK_SESSIONS: usize = 3;
const MIN_TYPICAL_PEAK_SESSION_MS: f64 = 30.0 * 60.0 * 1000.0;
const TYPICAL_PEAK_MAX_AGE_MS: f64 = 30.0 * 24.0 * 60.0 * 60.0 * 1000.0;

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    }
}

fn percent_change(current: f64, baseline: f64) -> f64 {
    if baseline <= 0.0 {
        return 0.0;
    }
    ((current - baseline) / baseline) * 100.0
}

/// Median summed-viewer peak of the streamer's recent full sessions, or `None`
/// without enough history. Short restarts are excluded.
pub fn typical_session_peak(sessions: &[StreamSession], now: i64) -> Option<f64> {
    #[allow(clippy::cast_precision_loss)]
    let cutoff = now as f64 - TYPICAL_PEAK_MAX_AGE_MS;
    let qualifying: Vec<f64> = sessions
        .iter()
        .filter(|s| s.duration_ms >= MIN_TYPICAL_PEAK_SESSION_MS && s.ended_at >= cutoff)
        .map(|s| s.peak_viewers)
        .collect();
    let recent = &qualifying[qualifying.len().saturating_sub(TYPICAL_PEAK_SESSIONS)..];
    (recent.len() >= MIN_TYPICAL_PEAK_SESSIONS).then(|| median(recent))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Flat,
    Climbing,
    Sparse,
}

#[derive(Clone, Copy, Debug)]
struct Baseline {
    level: f64,
    samples: usize,
    shape: Shape,
}

/// `(at, value)` samples.
type Points = Vec<(i64, f64)>;

fn measure_baseline(samples: &[(i64, f64)], window_start: i64, window_end: i64) -> Baseline {
    #[allow(clippy::cast_precision_loss)]
    let midpoint = (window_start as f64 + window_end as f64) / 2.0;
    #[allow(clippy::cast_precision_loss)]
    let (older, newer): (Points, Points) =
        samples.iter().partition(|(at, _)| (*at as f64) < midpoint);
    let values = |items: &[(i64, f64)]| items.iter().map(|(_, v)| *v).collect::<Vec<_>>();
    let older_level = median(&values(&older));
    let newer_level = median(&values(&newer));
    let shape = if older.len() < MIN_HALF_SAMPLES || newer.len() < MIN_HALF_SAMPLES {
        Shape::Sparse
    } else if newer_level > older_level * (1.0 + MAX_BASELINE_CLIMB) {
        Shape::Climbing
    } else {
        Shape::Flat
    };
    Baseline {
        level: median(&values(samples)),
        samples: samples.len(),
        shape,
    }
}

/// One observation of a live streamer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnomalyInput<'a> {
    pub streamer_id: &'a str,
    /// The primary binding's viewers, so a second binding going live is not a jump.
    pub viewers: Option<f64>,
    pub dgg_viewers: Option<f64>,
    pub session_started_at: i64,
    /// Identifies the primary binding; a change restarts the viewer baseline.
    pub source_key: Option<String>,
    /// Summed viewers across live bindings, compared against `typical_peak`.
    pub total_viewers: Option<f64>,
    /// Typical summed session peak; a platform surge must reach it.
    pub typical_peak: Option<f64>,
    pub now: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Streaks {
    viewers: u32,
    dgg: u32,
}

#[derive(Debug, Default)]
pub struct ViewerAnomalyTracker {
    samples: HashMap<String, Vec<ViewerSample>>,
    surge_streaks: HashMap<String, Streaks>,
    sources: HashMap<String, String>,
}

fn format_count(value: Option<f64>) -> String {
    value.map_or_else(|| "null".to_owned(), number_to_string)
}

impl ViewerAnomalyTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&mut self, input: AnomalyInput<'_>) -> ViewerTrend {
        let now = input.now;
        let id = input.streamer_id;
        if let Some(source_key) = &input.source_key {
            if let Some(previous) = self.sources.get(id)
                && previous != source_key
            {
                // DGG presence does not depend on the primary binding, so keep it.
                if let Some(samples) = self.samples.get_mut(id) {
                    for sample in samples.iter_mut() {
                        sample.viewers = None;
                    }
                }
                if let Some(streaks) = self.surge_streaks.get_mut(id) {
                    streaks.viewers = 0;
                }
            }
            self.sources.insert(id.to_owned(), source_key.clone());
        }
        let mut history: Vec<ViewerSample> = self
            .samples
            .remove(id)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.at >= now - SAMPLE_WINDOW_MS)
            .collect();
        let window_start = now - SAMPLE_WINDOW_MS;
        let window_end = now - MIN_BASELINE_AGE_MS;
        let baseline_samples: Vec<ViewerSample> = history
            .iter()
            .filter(|s| s.at <= window_end)
            .copied()
            .collect();
        let viewer_points: Vec<(i64, f64)> = baseline_samples
            .iter()
            .filter_map(|s| s.viewers.map(|v| (s.at, v)))
            .collect();
        let dgg_points: Vec<(i64, f64)> = baseline_samples
            .iter()
            .filter_map(|s| s.dgg_viewers.map(|v| (s.at, v)))
            .collect();
        let viewer_baseline = measure_baseline(&viewer_points, window_start, window_end);
        let dgg_baseline = measure_baseline(&dgg_points, window_start, window_end);
        #[allow(clippy::cast_precision_loss)]
        let elapsed_minutes = baseline_samples.first().map_or(1.0, |oldest| {
            js_max(1.0, (now - oldest.at) as f64 / 60_000.0)
        });
        let oldest_viewer = baseline_samples.iter().find(|s| s.viewers.is_some());
        let viewers_per_minute = match (oldest_viewer, input.viewers) {
            (Some(oldest), Some(current)) => {
                (current - oldest.viewers.unwrap_or(0.0)) / elapsed_minutes
            }
            _ => 0.0,
        };
        let viewer_percent = input
            .viewers
            .map_or(0.0, |v| percent_change(v, viewer_baseline.level));
        let dgg_percent = match input.dgg_viewers {
            Some(dgg) if dgg_baseline.level > 0.0 => Some(percent_change(dgg, dgg_baseline.level)),
            _ => None,
        };
        let session_warmed = now - input.session_started_at >= MIN_SESSION_AGE_MS;
        let viewer_jump = session_warmed
            && viewer_baseline.samples >= MIN_BASELINE_SAMPLES
            && input.viewers.is_some_and(|v| {
                viewer_percent >= VIEWER_SURGE_PERCENT
                    && v - viewer_baseline.level >= MIN_VIEWER_SURGE_GAIN
            });
        let typical_peak = input.typical_peak;
        let below_typical_peak = typical_peak
            .is_some_and(|peak| input.total_viewers.or(input.viewers).unwrap_or(0.0) < peak);
        let viewer_surge_candidate =
            viewer_jump && viewer_baseline.shape == Shape::Flat && !below_typical_peak;
        let dgg_jump = session_warmed
            && dgg_baseline.samples >= MIN_BASELINE_SAMPLES
            && dgg_percent.is_some_and(|percent| percent >= DGG_SURGE_PERCENT)
            && input.dgg_viewers.unwrap_or(0.0) - dgg_baseline.level >= MIN_DGG_SURGE_GAIN;
        let dgg_surge_candidate = dgg_jump && dgg_baseline.shape == Shape::Flat;
        let previous = self.surge_streaks.get(id).copied().unwrap_or_default();
        let streaks = Streaks {
            viewers: if viewer_surge_candidate {
                previous.viewers + 1
            } else {
                0
            },
            dgg: if dgg_surge_candidate {
                previous.dgg + 1
            } else {
                0
            },
        };
        self.surge_streaks.insert(id.to_owned(), streaks);
        let viewer_surge = streaks.viewers >= SURGE_CONFIRMATION_OBSERVATIONS;
        let dgg_surge = streaks.dgg >= SURGE_CONFIRMATION_OBSERVATIONS;
        let candidate_observations = streaks.viewers.max(streaks.dgg);
        let anomalous = viewer_surge || dgg_surge;
        let mut reasons = Vec::new();
        if viewer_surge {
            reasons.push(format!(
                "viewers up {}% ({} vs {} baseline)",
                number_to_string(math_round(viewer_percent)),
                format_count(input.viewers),
                number_to_string(math_round(viewer_baseline.level)),
            ));
        }
        if dgg_surge {
            reasons.push(format!(
                "DGG audience up {}% ({} vs {} baseline)",
                number_to_string(math_round(dgg_percent.unwrap_or(0.0))),
                format_count(input.dgg_viewers),
                number_to_string(math_round(dgg_baseline.level)),
            ));
        }
        let suppression_reason = if !session_warmed {
            #[allow(clippy::cast_precision_loss)]
            let remaining =
                ((MIN_SESSION_AGE_MS - (now - input.session_started_at)) as f64 / 60_000.0).ceil();
            Some(format!(
                "Building a post-start baseline ({}m remaining)",
                number_to_string(js_max(1.0, remaining))
            ))
        } else if viewer_baseline.samples < MIN_BASELINE_SAMPLES
            && dgg_baseline.samples < MIN_BASELINE_SAMPLES
        {
            Some(format!(
                "Waiting for {MIN_BASELINE_SAMPLES} baseline samples"
            ))
        } else if (viewer_surge_candidate || dgg_surge_candidate) && !anomalous {
            Some("Confirming the viewer rise with another observation".to_owned())
        } else if !anomalous && (viewer_jump || dgg_jump) {
            let mut shapes = Vec::new();
            if viewer_jump {
                shapes.push(viewer_baseline.shape);
            }
            if dgg_jump {
                shapes.push(dgg_baseline.shape);
            }
            if shapes.contains(&Shape::Climbing) {
                Some("Audience is still ramping up".to_owned())
            } else if shapes.contains(&Shape::Sparse) {
                Some("Waiting for a longer baseline".to_owned())
            } else if viewer_jump && below_typical_peak {
                typical_peak.map(|peak| {
                    format!(
                        "Below the typical session peak of {}",
                        number_to_string(math_round(peak))
                    )
                })
            } else {
                None
            }
        } else {
            None
        };
        history.push(ViewerSample {
            at: now,
            viewers: input.viewers,
            dgg_viewers: input.dgg_viewers,
        });
        self.samples.insert(id.to_owned(), history);
        #[allow(clippy::cast_precision_loss)]
        ViewerTrend {
            percent_change: viewer_percent,
            viewers_per_minute,
            dgg_percent_change: dgg_percent,
            anomalous,
            reason: (!reasons.is_empty()).then(|| reasons.join("; ")),
            current_viewers: Some(input.viewers),
            baseline_viewers: Some((viewer_baseline.samples > 0).then_some(viewer_baseline.level)),
            current_dgg_viewers: Some(input.dgg_viewers),
            baseline_dgg_viewers: Some((dgg_baseline.samples > 0).then_some(dgg_baseline.level)),
            baseline_samples: Some(viewer_baseline.samples.max(dgg_baseline.samples) as f64),
            typical_peak_viewers: Some(typical_peak),
            candidate_observations: Some(f64::from(candidate_observations)),
            suppression_reason: Some(suppression_reason),
            updated_at: now,
            extra: Default::default(),
        }
    }

    pub fn clear(&mut self, streamer_id: &str) {
        self.samples.remove(streamer_id);
        self.surge_streaks.remove(streamer_id);
        self.sources.remove(streamer_id);
    }
}

/// Relevance score (0-100) and up to four reasons.
pub fn compute_relevance(
    streamer: &Streamer,
    semantic: Option<&SemanticMetadata>,
    trend: Option<&ViewerTrend>,
    destiny_confirmed: bool,
) -> (f64, Vec<String>) {
    let mut reasons = Vec::new();
    let primary = streamer.tier == StreamerTier::Primary;
    let mut score = if primary { 40.0 } else { 15.0 };
    if primary {
        reasons.push("primary channel".to_owned());
    }
    if let Some(semantic) = semantic {
        score += semantic.importance * 0.35;
        if semantic.importance >= 65.0 {
            reasons.push(semantic.reason.clone());
        }
    }
    if let Some(trend) = trend
        && trend.anomalous
    {
        score += 25.0;
        if let Some(reason) = &trend.reason {
            reasons.push(reason.clone());
        }
    }
    let dgg_viewers = streamer.dgg.and_then(|d| d.viewers).unwrap_or(0.0);
    if dgg_viewers > 0.0 {
        score += js_min(15.0, (dgg_viewers + 1.0).log10() * 5.0);
        if dgg_viewers >= 100.0 {
            reasons.push(format!("{} watching on DGG", number_to_string(dgg_viewers)));
        }
    }
    if destiny_confirmed {
        score += 40.0;
        reasons.push("Destiny detected as a live participant".to_owned());
    }
    reasons.truncate(4);
    (math_round(js_min(100.0, score)), reasons)
}
