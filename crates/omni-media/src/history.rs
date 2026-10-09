//! Ground-truth watch history for taste inputs.

use crate::js::{math_round, number, to_date_stamp};
use crate::outcomes::WATCHED_COMPLETION_THRESHOLD;
use crate::types::{InProgressItem, MediaType, WatchedItem};

const DIGEST_LIMIT: usize = 40;

/// Whether a watch counts as completed: by completion when known, otherwise
/// a movie with at least one view.
pub fn is_completed(media_type: MediaType, completion: Option<f64>, view_count: f64) -> bool {
    match completion {
        Some(completion) => completion >= WATCHED_COMPLETION_THRESHOLD,
        None => media_type == MediaType::Movie && view_count >= 1.0,
    }
}

/// Completed watches, newest first.
pub fn completed_watches(watched: &[WatchedItem]) -> Vec<WatchedItem> {
    #[allow(clippy::cast_precision_loss)]
    let mut completed: Vec<WatchedItem> = watched
        .iter()
        .filter(|w| is_completed(w.item.media_type, w.completion, w.view_count as f64))
        .cloned()
        .collect();
    completed.sort_by_key(|w| std::cmp::Reverse(w.viewed_at));
    completed
}

fn year_suffix(year: Option<i64>) -> String {
    year.filter(|y| *y != 0)
        .map(|y| format!(" ({y})"))
        .unwrap_or_default()
}

/// The compact taste digest injected into model prompts, derived only from
/// ground-truth watch history (never from recommendation outcome labels).
pub fn format_history_digest(watched: &[WatchedItem], in_progress: &[InProgressItem]) -> String {
    let completed: Vec<WatchedItem> = completed_watches(watched)
        .into_iter()
        .take(DIGEST_LIMIT)
        .collect();
    let mut lines: Vec<String> = Vec::new();
    if completed.is_empty() {
        lines.push("No completed watch history available.".to_owned());
    } else {
        lines.push("Recently watched (newest first):".to_owned());
        for item in &completed {
            let rewatch = if item.view_count > 1 {
                format!(" — rewatched {}x", item.view_count)
            } else {
                String::new()
            };
            lines.push(format!(
                "- {}{} [{}] — {}{rewatch}",
                item.item.title,
                year_suffix(item.item.year),
                item.item.media_type.as_str(),
                to_date_stamp(item.viewed_at)
            ));
        }
    }
    if !in_progress.is_empty() {
        lines.push(String::new());
        lines.push("Currently watching:".to_owned());
        for item in in_progress.iter().take(10) {
            lines.push(format!(
                "- {}{} [{}] — {}% through",
                item.item.title,
                year_suffix(item.item.year),
                item.item.media_type.as_str(),
                number(math_round(item.progress * 100.0))
            ));
        }
    }
    lines.join("\n")
}
