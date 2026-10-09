//! Candidate pool assembly.

use std::collections::HashSet;

use futures::StreamExt;

use crate::error::IntegrationError;
use crate::tmdb::types::{TmdbTitle, TmdbTitleDetails};
use crate::tmdb::{Catalog, DiscoverOptions};
use crate::types::{Candidate, CandidateSource, MediaType, make_canonical_id};

pub const TARGET_POOL_SIZE: usize = 80;
/// No single source bucket may exceed this share of the pool.
pub const MAX_SOURCE_SHARE: f64 = 1.0 / 3.0;
const SEED_LIMIT: usize = 8;
const NOVELTY_SHARE: f64 = 0.15;
const REQUIRED_ORIGINAL_LANGUAGE: &str = "en";
const SIMILAR_CONCURRENCY: usize = 4;
const DETAIL_CONCURRENCY: usize = 6;
const LOG: &str = "Main:RecsTask";

/// A recent completed watch that seeds recommendations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchSeed {
    pub canonical_id: String,
    pub tmdb_id: i64,
    pub media_type: MediaType,
    pub genre_ids: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceBucket {
    pub source: CandidateSource,
    pub titles: Vec<TmdbTitle>,
}

/// A pooled candidate before enrichment (genre ids, no library flag).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PooledCandidate {
    pub canonical_id: String,
    pub tmdb_id: i64,
    pub media_type: MediaType,
    pub title: String,
    pub year: Option<i64>,
    pub overview: String,
    pub genre_ids: Vec<i64>,
    pub vote_average: f64,
    pub vote_count: f64,
    pub popularity: f64,
    pub poster_path: Option<String>,
    pub original_language: Option<String>,
    pub source: CandidateSource,
}

pub fn is_eligible_original_language(title: &TmdbTitle) -> bool {
    title.original_language.as_deref() == Some(REQUIRED_ORIGINAL_LANGUAGE)
}

fn english_only(titles: Vec<TmdbTitle>) -> Vec<TmdbTitle> {
    titles
        .into_iter()
        .filter(is_eligible_original_language)
        .collect()
}

/// Round-robin merge of several lists.
fn interleave<T>(lists: Vec<Vec<T>>) -> Vec<T> {
    let mut iters: Vec<std::vec::IntoIter<T>> = lists.into_iter().map(Vec::into_iter).collect();
    let mut out = Vec::new();
    loop {
        let mut any = false;
        for iter in &mut iters {
            if let Some(item) = iter.next() {
                out.push(item);
                any = true;
            }
        }
        if !any {
            return out;
        }
    }
}

/// Genre ids by how many seeds carry them (ties keep first appearance).
pub fn rank_genres(seeds: &[WatchSeed]) -> Vec<i64> {
    let mut counts: Vec<(i64, usize)> = Vec::new();
    for seed in seeds {
        for genre in &seed.genre_ids {
            match counts.iter_mut().find(|(id, _)| id == genre) {
                Some((_, count)) => *count += 1,
                None => counts.push((*genre, 1)),
            }
        }
    }
    counts.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    counts.into_iter().map(|(id, _)| id).collect()
}

/// TMDB buckets: recommendations seeded by recent completed watches, discover
/// on the top genres, weekly trending, and novelty outside the top genres.
/// Each fetch failure degrades to an empty contribution (logged).
pub async fn fetch_candidate_buckets(
    catalog: &dyn Catalog,
    seeds: &[WatchSeed],
) -> Vec<SourceBucket> {
    let recent: Vec<WatchSeed> = seeds.iter().take(SEED_LIMIT).cloned().collect();
    let top_genres: Vec<i64> = rank_genres(seeds).into_iter().take(3).collect();
    let trending = async {
        catalog.trending().await.unwrap_or_else(|error| {
            tracing::warn!(target: LOG, error = error.cause_message(), "TMDB trending fetch failed");
            Vec::new()
        })
    };
    let (similar, discover, trending, novelty) = futures::join!(
        fetch_similar(catalog, &recent),
        fetch_discover(catalog, &top_genres),
        trending,
        fetch_novelty(catalog, &top_genres)
    );
    vec![
        SourceBucket {
            source: CandidateSource::Similar,
            titles: english_only(similar),
        },
        SourceBucket {
            source: CandidateSource::Discover,
            titles: english_only(discover),
        },
        SourceBucket {
            source: CandidateSource::Trending,
            titles: english_only(trending),
        },
        SourceBucket {
            source: CandidateSource::Novelty,
            titles: english_only(novelty),
        },
    ]
}

async fn fetch_similar(catalog: &dyn Catalog, seeds: &[WatchSeed]) -> Vec<TmdbTitle> {
    let results: Vec<Vec<TmdbTitle>> = futures::stream::iter(
        seeds
            .iter()
            .map(|seed| async move {
                match catalog
                    .recommendations_for(seed.media_type, seed.tmdb_id)
                    .await
                {
                    Ok(titles) => titles,
                    Err(error) => {
                        tracing::warn!(
                            target: LOG,
                            error = error.cause_message(),
                            "TMDB recommendations fetch failed for {}",
                            seed.canonical_id
                        );
                        Vec::new()
                    }
                }
            })
            .collect::<Vec<_>>(),
    )
    .buffered(SIMILAR_CONCURRENCY)
    .collect()
    .await;
    interleave(
        results
            .into_iter()
            .map(|titles| english_only(titles).into_iter().take(12).collect())
            .collect(),
    )
}

async fn discover_both(
    catalog: &dyn Catalog,
    options: &DiscoverOptions,
    label: &'static str,
) -> Vec<TmdbTitle> {
    let fetch = |media_type: MediaType| async move {
        match catalog.discover(media_type, options).await {
            Ok(titles) => titles,
            Err(error) => {
                tracing::warn!(
                    target: LOG,
                    error = error.cause_message(),
                    "{label} ({})",
                    media_type.as_str()
                );
                Vec::new()
            }
        }
    };
    let (movies, series) = futures::join!(fetch(MediaType::Movie), fetch(MediaType::Tv));
    interleave(vec![movies, series])
}

async fn fetch_discover(catalog: &dyn Catalog, top_genres: &[i64]) -> Vec<TmdbTitle> {
    if top_genres.is_empty() {
        return Vec::new();
    }
    let options = DiscoverOptions {
        with_genres: Some(top_genres.to_vec()),
        with_original_language: Some(REQUIRED_ORIGINAL_LANGUAGE.to_owned()),
        ..DiscoverOptions::default()
    };
    discover_both(catalog, &options, "TMDB discover failed").await
}

async fn fetch_novelty(catalog: &dyn Catalog, top_genres: &[i64]) -> Vec<TmdbTitle> {
    let options = DiscoverOptions {
        without_genres: Some(top_genres.to_vec()),
        with_original_language: Some(REQUIRED_ORIGINAL_LANGUAGE.to_owned()),
        min_vote_count: Some(1000),
        ..DiscoverOptions::default()
    };
    discover_both(catalog, &options, "TMDB novelty discover failed").await
}

/// Dedupes by canonical id (first source wins), caps each source at its
/// share, reserves the novelty share and caps the pool size.
pub fn assemble_pool(buckets: &[SourceBucket], target_size: usize) -> Vec<PooledCandidate> {
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let (per_source_cap, novelty_reserve) = (
        (target_size as f64 * MAX_SOURCE_SHARE).ceil() as usize,
        (target_size as f64 * NOVELTY_SHARE).floor() as usize,
    );
    let mut seen: HashSet<String> = HashSet::new();
    let mut pool: Vec<PooledCandidate> = Vec::new();

    let mut take = |bucket: &SourceBucket, cap: usize, pool: &mut Vec<PooledCandidate>| {
        let mut taken = 0usize;
        for title in &bucket.titles {
            if taken >= cap || pool.len() >= target_size {
                break;
            }
            if !is_eligible_original_language(title) {
                continue;
            }
            let canonical_id = make_canonical_id(title.media_type, title.tmdb_id);
            if !seen.insert(canonical_id.clone()) {
                continue;
            }
            pool.push(PooledCandidate {
                canonical_id,
                tmdb_id: title.tmdb_id,
                media_type: title.media_type,
                title: title.title.clone(),
                year: title.year,
                overview: title.overview.clone(),
                genre_ids: title.genre_ids.clone(),
                vote_average: title.vote_average,
                vote_count: title.vote_count,
                popularity: title.popularity,
                poster_path: title.poster_path.clone(),
                original_language: title.original_language.clone(),
                source: bucket.source,
            });
            taken += 1;
        }
    };

    let rest_budget = target_size.saturating_sub(novelty_reserve);
    for bucket in buckets
        .iter()
        .filter(|b| b.source != CandidateSource::Novelty)
    {
        let cap = per_source_cap.min(rest_budget.saturating_sub(pool.len()));
        take(bucket, cap, &mut pool);
    }
    for bucket in buckets
        .iter()
        .filter(|b| b.source == CandidateSource::Novelty)
    {
        take(bucket, per_source_cap.min(novelty_reserve), &mut pool);
    }
    pool
}

/// Attaches genre names, TMDB details (a failed detail fetch keeps the
/// candidate) and library presence.
pub async fn enrich_candidates(
    catalog: &dyn Catalog,
    pool: &[PooledCandidate],
    library_ids: &HashSet<String>,
) -> Result<Vec<Candidate>, IntegrationError> {
    let (movie_genres, tv_genres) = futures::join!(
        catalog.genre_map(MediaType::Movie),
        catalog.genre_map(MediaType::Tv)
    );
    let (movie_genres, tv_genres) = (movie_genres?, tv_genres?);
    let details: Vec<Option<TmdbTitleDetails>> = futures::stream::iter(
        pool.iter()
            .map(|candidate| async move {
                match catalog
                    .title_details(candidate.media_type, candidate.tmdb_id)
                    .await
                {
                    Ok(details) => Some(details),
                    Err(error) => {
                        tracing::warn!(
                            target: LOG,
                            error = error.cause_message(),
                            "TMDB details fetch failed for {}",
                            candidate.canonical_id
                        );
                        None
                    }
                }
            })
            .collect::<Vec<_>>(),
    )
    .buffered(DETAIL_CONCURRENCY)
    .collect()
    .await;
    Ok(pool
        .iter()
        .zip(details)
        .map(|(c, details)| {
            let genre_map = match c.media_type {
                MediaType::Movie => &movie_genres,
                MediaType::Tv => &tv_genres,
            };
            let mut candidate = Candidate {
                canonical_id: c.canonical_id.clone(),
                tmdb_id: c.tmdb_id,
                media_type: c.media_type,
                title: c.title.clone(),
                year: c.year,
                overview: c.overview.clone(),
                genres: c
                    .genre_ids
                    .iter()
                    .filter_map(|id| genre_map.get(id).cloned())
                    .collect(),
                vote_average: c.vote_average,
                vote_count: c.vote_count,
                popularity: c.popularity,
                poster_path: c.poster_path.clone(),
                original_language: c.original_language.clone(),
                source: c.source,
                in_library: library_ids.contains(&c.canonical_id),
                ..Candidate::default()
            };
            if let Some(details) = details {
                // `{...c, ...details}`: every details key overrides, absent ones included.
                candidate.runtime_minutes = details.runtime_minutes;
                candidate.season_count = details.season_count;
                candidate.episode_count = details.episode_count;
                candidate.series_status = details.series_status;
                candidate.original_language = details.original_language;
                candidate.origin_countries = Some(details.origin_countries);
                candidate.creators = Some(details.creators);
                candidate.cast = Some(details.cast);
                candidate.keywords = Some(details.keywords);
                candidate.certification = details.certification;
            }
            candidate
        })
        .collect())
}
