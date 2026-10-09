//! Voiceprint enrollment helpers (`src/tools/enroll-destiny-voice.ts`).

use std::cmp::Ordering;
use std::collections::HashSet;

use crate::speech::cosine_similarity;

const CLUSTER_SIMILARITY: f64 = 0.5;
const MAX_ENROLLMENT_WINDOWS: usize = 16;
/// Fewer consistent windows than this cannot make a voiceprint.
pub const MIN_ENROLLMENT_WINDOWS: usize = 6;

/// One window embedding and the clip it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct SourcedEmbedding {
    pub embedding: Vec<f32>,
    pub source_index: usize,
}

fn similarity(a: &[f32], b: &[f32]) -> f64 {
    let b: Vec<f64> = b.iter().map(|v| f64::from(*v)).collect();
    cosine_similarity(a, &b)
}

/// The windows around the embedding that agrees with the most clips (then the
/// most central one): up to 16 windows within 0.5 cosine of it, or nothing
/// when no window is shared by at least two clips.
pub fn select_cross_source_cluster(embeddings: &[SourcedEmbedding]) -> Vec<Vec<f32>> {
    #[allow(clippy::cast_precision_loss)]
    let count = embeddings.len() as f64;
    let mut ranked: Vec<(usize, usize, f64)> = embeddings
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let sources: HashSet<usize> = embeddings
                .iter()
                .filter(|item| {
                    similarity(&candidate.embedding, &item.embedding) >= CLUSTER_SIMILARITY
                })
                .map(|item| item.source_index)
                .collect();
            let centrality = embeddings
                .iter()
                .map(|item| similarity(&candidate.embedding, &item.embedding))
                .sum::<f64>()
                / count;
            (index, sources.len(), centrality)
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| b.2.partial_cmp(&a.2).unwrap_or(Ordering::Equal))
    });
    let Some(&(center_index, source_count, _)) = ranked.first() else {
        return Vec::new();
    };
    if source_count < 2 {
        return Vec::new();
    }
    let center = &embeddings[center_index].embedding;
    ranked
        .iter()
        .map(|(index, _, _)| &embeddings[*index].embedding)
        .filter(|embedding| similarity(center, embedding) >= CLUSTER_SIMILARITY)
        .take(MAX_ENROLLMENT_WINDOWS)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sourced(embedding: [f32; 2], source_index: usize) -> SourcedEmbedding {
        SourcedEmbedding {
            embedding: embedding.to_vec(),
            source_index,
        }
    }

    #[test]
    fn keeps_the_cluster_shared_across_clips() {
        let embeddings = vec![
            sourced([1.0, 0.0], 0),
            sourced([0.9, 0.1], 1),
            sourced([0.0, 1.0], 0),
            sourced([0.95, 0.05], 0),
        ];
        let selected = select_cross_source_cluster(&embeddings);
        assert_eq!(selected.len(), 3);
        assert!(selected.iter().all(|e| e[0] > 0.5));
    }

    #[test]
    fn rejects_windows_from_a_single_clip() {
        let embeddings = vec![sourced([1.0, 0.0], 0), sourced([0.0, 1.0], 1)];
        assert!(select_cross_source_cluster(&embeddings).is_empty());
    }
}
