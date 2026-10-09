//! Recommendation candidate gathering.
#![allow(clippy::expect_used)]

use omni_media::candidates::{
    MAX_SOURCE_SHARE, SourceBucket, WatchSeed, assemble_pool, rank_genres,
};
use omni_media::tmdb::types::TmdbTitle;
use omni_media::types::{CandidateSource, MediaType};

fn make_title(tmdb_id: i64, media_type: MediaType) -> TmdbTitle {
    TmdbTitle {
        tmdb_id,
        media_type,
        title: format!("Title {tmdb_id}"),
        vote_average: 7.0,
        vote_count: 500.0,
        popularity: 5.0,
        original_language: Some("en".to_owned()),
        ..TmdbTitle::default()
    }
}

fn bucket(source: CandidateSource, ids: impl IntoIterator<Item = i64>) -> SourceBucket {
    SourceBucket {
        source,
        titles: ids
            .into_iter()
            .map(|id| make_title(id, MediaType::Movie))
            .collect(),
    }
}

#[test]
fn dedupes_across_buckets_first_source_wins() {
    let pool = assemble_pool(
        &[
            bucket(CandidateSource::Similar, [1, 2, 3]),
            bucket(CandidateSource::Trending, [2, 3, 4]),
        ],
        20,
    );
    assert_eq!(
        pool.iter().map(|c| c.tmdb_id).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    assert_eq!(
        pool.iter().find(|c| c.tmdb_id == 2).map(|c| c.source),
        Some(CandidateSource::Similar)
    );
}

#[test]
fn dedupes_same_tmdb_id_across_media_types_as_distinct_candidates() {
    let pool = assemble_pool(
        &[SourceBucket {
            source: CandidateSource::Similar,
            titles: vec![
                make_title(1, MediaType::Movie),
                make_title(1, MediaType::Tv),
            ],
        }],
        20,
    );
    assert_eq!(pool.len(), 2);
}

#[test]
fn caps_each_source_at_the_max_share() {
    let target = 30usize;
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let cap = (target as f64 * MAX_SOURCE_SHARE).ceil() as usize;
    let pool = assemble_pool(&[bucket(CandidateSource::Similar, 1..=50)], target);
    assert!(pool.len() <= cap);
}

#[test]
fn reserves_room_for_the_novelty_bucket() {
    let target = 30;
    let pool = assemble_pool(
        &[
            bucket(CandidateSource::Similar, 100..110),
            bucket(CandidateSource::Discover, 200..210),
            bucket(CandidateSource::Trending, 300..310),
            bucket(CandidateSource::Novelty, 400..410),
        ],
        target,
    );
    let novelty = pool
        .iter()
        .filter(|c| c.source == CandidateSource::Novelty)
        .count();
    assert!(novelty > 0);
    assert!(pool.len() <= target);
}

#[test]
fn never_exceeds_the_target_size() {
    let pool = assemble_pool(
        &[
            bucket(CandidateSource::Similar, 1..40),
            bucket(CandidateSource::Discover, 100..140),
            bucket(CandidateSource::Trending, 200..240),
            bucket(CandidateSource::Novelty, 300..340),
        ],
        50,
    );
    assert!(pool.len() <= 50);
}

#[test]
fn keeps_only_titles_whose_original_language_is_english() {
    let english = make_title(1, MediaType::Movie);
    let french = TmdbTitle {
        original_language: Some("fr".to_owned()),
        ..make_title(2, MediaType::Movie)
    };
    let unknown = TmdbTitle {
        original_language: None,
        ..make_title(3, MediaType::Movie)
    };
    let pool = assemble_pool(
        &[SourceBucket {
            source: CandidateSource::Trending,
            titles: vec![english, french, unknown],
        }],
        20,
    );
    assert_eq!(pool.iter().map(|c| c.tmdb_id).collect::<Vec<_>>(), vec![1]);
    assert_eq!(pool[0].original_language.as_deref(), Some("en"));
}

fn seed(genre_ids: &[i64]) -> WatchSeed {
    WatchSeed {
        canonical_id: "tmdb:movie:1".to_owned(),
        tmdb_id: 1,
        media_type: MediaType::Movie,
        genre_ids: genre_ids.to_vec(),
    }
}

#[test]
fn ranks_genres_by_frequency_across_seeds() {
    let ranked = rank_genres(&[seed(&[18, 35]), seed(&[18]), seed(&[18, 878]), seed(&[878])]);
    assert_eq!(ranked[..3], [18, 878, 35]);
}
