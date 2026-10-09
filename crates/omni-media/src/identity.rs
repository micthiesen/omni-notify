//! Canonical TMDB identity for media-server items (`src/recommendations/identity.ts`).

use std::sync::LazyLock;

use icu_normalizer::DecomposingNormalizerBorrowed;
use omni_store::entity::{EntityOps as _, EntityWrite as _, UpsertOpts};
use omni_store::{DocOps as _, Store, StoreError};
use regex::Regex;

use crate::persistence::{IdentityAliasData, ResolutionPath};
use crate::tmdb::{Catalog, FindSource};
use crate::types::{ExternalIds, MediaItem, MediaType, make_canonical_id};

/// Below this confidence a resolution is treated as unresolved.
pub const RESOLUTION_CONFIDENCE_THRESHOLD: f64 = 0.8;

#[derive(Clone, Debug, PartialEq)]
pub struct Resolution {
    pub canonical_id: Option<String>,
    pub confidence: f64,
    pub resolution_path: ResolutionPath,
}

impl Resolution {
    fn unresolved(confidence: f64) -> Self {
        Self {
            canonical_id: None,
            confidence,
            resolution_path: ResolutionPath::Unresolved,
        }
    }

    /// The canonical id when confidently resolved.
    pub fn confident_id(&self) -> Option<&str> {
        self.canonical_id
            .as_deref()
            .filter(|_| self.confidence >= RESOLUTION_CONFIDENCE_THRESHOLD)
    }
}

static GUID_RE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"(?:^|\.)(imdb|tmdb|themoviedb|tvdb|thetvdb)://([a-zA-Z0-9]+)").ok()
});

/// External ids embedded in a server-native GUID (`tmdb://123`,
/// `imdb://tt123`, `tvdb://123`, legacy `com.plexapp.agents.imdb://tt…?lang=en`).
pub fn parse_guid_external_ids(guid: &str) -> ExternalIds {
    let mut ids = ExternalIds::default();
    let Some(captures) = GUID_RE.as_ref().and_then(|re| re.captures(guid)) else {
        return ids;
    };
    let (Some(source), Some(value)) = (captures.get(1), captures.get(2)) else {
        return ids;
    };
    let value = value.as_str();
    match source.as_str() {
        "imdb" if value.starts_with("tt") => ids.imdb = Some(value.to_owned()),
        "tmdb" | "themoviedb" => ids.tmdb = integer(value),
        "tvdb" | "thetvdb" => ids.tvdb = integer(value),
        _ => {}
    }
    ids
}

/// `Number(value)` when it is an integer.
fn integer(value: &str) -> Option<i64> {
    let number = omni_core::js::string_to_number(value);
    #[allow(clippy::cast_possible_truncation)]
    (number.is_finite() && number.fract() == 0.0).then_some(number as i64)
}

const LOG: &str = "Main:RecsTask";

/// Resolves `item` to its canonical TMDB id. Every network resolution
/// (including failures) is cached by GUID; without network access an item
/// without a direct TMDB id stays unresolved and uncached so a later full
/// pass can still fill it in.
pub async fn resolve_identity(
    store: &Store,
    catalog: &dyn Catalog,
    item: &MediaItem,
    allow_network: bool,
) -> Result<Resolution, StoreError> {
    let guid = item.guid.clone();
    if let Some(cached) = store
        .read(move |docs| docs.get::<IdentityAliasData>(&guid))
        .await?
    {
        return Ok(Resolution {
            canonical_id: cached.canonical_id,
            confidence: cached.confidence,
            resolution_path: cached.resolution_path,
        });
    }

    let external = parse_guid_external_ids(&item.guid).overlay(item.external_ids.as_ref());

    if let Some(tmdb) = external.tmdb {
        let resolution = Resolution {
            canonical_id: Some(make_canonical_id(item.media_type, tmdb)),
            confidence: 1.0,
            resolution_path: ResolutionPath::ExternalId,
        };
        cache_resolution(store, item, &resolution).await?;
        return Ok(resolution);
    }

    if !allow_network {
        return Ok(Resolution::unresolved(0.0));
    }

    let resolution = resolve_via_network(catalog, item, &external).await;
    cache_resolution(store, item, &resolution).await?;
    Ok(resolution)
}

async fn cache_resolution(
    store: &Store,
    item: &MediaItem,
    resolution: &Resolution,
) -> Result<(), StoreError> {
    let mut alias = IdentityAliasData {
        guid: item.guid.clone(),
        canonical_id: resolution.canonical_id.clone(),
        confidence: resolution.confidence,
        resolution_path: resolution.resolution_path,
        title: item.title.clone(),
        resolved_at: 0,
        extra: Default::default(),
    };
    store
        .write(move |tx| {
            alias.resolved_at = tx.now_ms();
            tx.upsert(&alias, UpsertOpts::default())
        })
        .await
}

async fn resolve_via_network(
    catalog: &dyn Catalog,
    item: &MediaItem,
    external: &ExternalIds,
) -> Resolution {
    let find = match (&external.imdb, external.tvdb) {
        (Some(imdb), _) => Some((imdb.clone(), FindSource::Imdb)),
        (None, Some(tvdb)) => Some((tvdb.to_string(), FindSource::Tvdb)),
        (None, None) => None,
    };
    if let Some((id, source)) = find {
        let matches = match catalog.find_by_external_id(&id, source).await {
            Ok(found) => found
                .into_iter()
                .filter(|t| t.media_type == item.media_type)
                .collect(),
            Err(error) => {
                tracing::warn!(
                    target: LOG,
                    error = error.effect_message(),
                    "TMDB find failed for \"{}\" ({}={id})",
                    item.title,
                    source.as_str()
                );
                Vec::new()
            }
        };
        if let [only] = matches.as_slice() {
            return Resolution {
                canonical_id: Some(make_canonical_id(item.media_type, only.tmdb_id)),
                confidence: 0.98,
                resolution_path: ResolutionPath::TmdbFind,
            };
        }
    }

    let results = match catalog
        .search_titles(&item.title, item.media_type, item.year)
        .await
    {
        Ok(results) => results,
        Err(error) => {
            tracing::warn!(
                target: LOG,
                error = error.effect_message(),
                "TMDB search failed for \"{}\"",
                item.title
            );
            Vec::new()
        }
    };
    let candidates: Vec<SearchResult> = results
        .iter()
        .map(|r| SearchResult {
            tmdb_id: r.tmdb_id,
            title: r.title.clone(),
            year: r.year,
            vote_count: r.vote_count,
        })
        .collect();
    score_search_results(&item.title, item.year, item.media_type, &candidates)
        .unwrap_or_else(|| Resolution::unresolved(0.0))
}

/// A search hit as `scoreSearchResults` sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchResult {
    pub tmdb_id: i64,
    pub title: String,
    pub year: Option<i64>,
    pub vote_count: f64,
}

/// Title (and, when known, year ±1) matching; several survivors resolve only
/// when one has at least ten times the votes of the runner-up. `None` when no
/// title matches.
pub fn score_search_results(
    title: &str,
    year: Option<i64>,
    media_type: MediaType,
    results: &[SearchResult],
) -> Option<Resolution> {
    let target = normalize_title(title);
    let title_matches: Vec<&SearchResult> = results
        .iter()
        .filter(|r| normalize_title(&r.title) == target)
        .collect();
    let year = year.filter(|y| *y != 0);
    let year_matches: Vec<&SearchResult> = match year {
        Some(year) => title_matches
            .into_iter()
            .filter(|r| r.year.is_some_and(|ry| (ry - year).abs() <= 1))
            .collect(),
        None => title_matches,
    };
    match year_matches.as_slice() {
        [] => None,
        [only] => Some(Resolution {
            canonical_id: Some(make_canonical_id(media_type, only.tmdb_id)),
            confidence: if year.is_some() { 0.95 } else { 0.85 },
            resolution_path: ResolutionPath::TmdbSearch,
        }),
        several => {
            let mut sorted: Vec<&&SearchResult> = several.iter().collect();
            sorted.sort_by(|a, b| b.vote_count.total_cmp(&a.vote_count));
            let (first, second) = (sorted[0], sorted[1]);
            if first.vote_count >= 10.0 * second.vote_count.max(1.0) {
                Some(Resolution {
                    canonical_id: Some(make_canonical_id(media_type, first.tmdb_id)),
                    confidence: 0.85,
                    resolution_path: ResolutionPath::TmdbSearch,
                })
            } else {
                Some(Resolution::unresolved(0.5))
            }
        }
    }
}

/// Lowercase, NFKD, strip combining diacritics (U+0300–U+036F), collapse
/// non-alphanumerics to single spaces, trim.
pub fn normalize_title(title: &str) -> String {
    let lowered = title.to_lowercase();
    let decomposed = DecomposingNormalizerBorrowed::new_nfkd().normalize(&lowered);
    let mut out = String::with_capacity(decomposed.len());
    let mut pending_space = false;
    for c in decomposed
        .chars()
        .filter(|c| !('\u{0300}'..='\u{036f}').contains(c))
    {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(c);
        } else {
            pending_space = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_title_strips_accents_and_punctuation() {
        assert_eq!(normalize_title("Amélie"), "amelie");
        assert_eq!(
            normalize_title("  The Thing: Part II! "),
            "the thing part ii"
        );
        assert_eq!(normalize_title("WALL·E"), "wall e");
    }
}
