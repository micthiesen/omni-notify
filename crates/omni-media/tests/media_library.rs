//! The Plex media library snapshot. Failures travel through the real client,
//! so the unavailable reason carries the operation prefix
//! (`<operation> failed: <cause>`).
#![allow(clippy::expect_used)]

use std::sync::Arc;

use futures::future::BoxFuture;
use omni_media::media_library::{MediaLibrary, PlexLibrary};
use omni_media::plex::{PlexClient, PlexGet, PlexParams};
use omni_media::types::{FetchResult, MediaItem, MediaType};
use serde_json::{Value, json};

struct Scripted(Result<Value, String>);

impl PlexGet for Scripted {
    fn get<'a>(
        &'a self,
        path: &'a str,
        _params: PlexParams,
    ) -> BoxFuture<'a, Result<Value, String>> {
        let result = match (&self.0, path) {
            (Ok(_), "/library/sections") => {
                Ok(json!({"MediaContainer": {"Directory": [{"key": "1", "type": "movie"}]}}))
            }
            (result, _) => result.clone(),
        };
        Box::pin(async move { result })
    }
}

fn library(result: Result<Value, String>) -> PlexLibrary {
    PlexLibrary::new(PlexClient::new(Arc::new(Scripted(result)), None))
}

#[tokio::test]
async fn wraps_successful_plex_responses() {
    let lib = library(Ok(json!({"MediaContainer": {"totalSize": 1, "Metadata": [
        {"type": "movie", "guid": "plex://movie/1", "title": "One"}
    ]}})));
    let FetchResult::Ok(history) = lib.watch_history().await else {
        panic!("history unavailable");
    };
    assert_eq!(history[0].item.guid, "plex://movie/1");
    let FetchResult::Ok(in_progress) = lib.in_progress().await else {
        panic!("in progress unavailable");
    };
    assert!(in_progress.is_empty());
    assert_eq!(
        lib.library_index().await,
        FetchResult::Ok(vec![MediaItem {
            guid: "plex://movie/1".to_owned(),
            title: "One".to_owned(),
            year: None,
            media_type: MediaType::Movie,
            external_ids: None,
            title_slug: None,
        }])
    );
}

#[tokio::test]
async fn reports_plex_failures_as_unavailable_instead_of_empty_state() {
    let lib = library(Err("Plex timed out".to_owned()));
    assert_eq!(
        lib.watch_history().await,
        FetchResult::Unavailable {
            reason: "Plex watch history failed: Plex timed out".to_owned()
        }
    );
}

#[tokio::test]
async fn reports_missing_configuration_as_unavailable() {
    let lib = PlexLibrary::from_config(None, Some("token"), None, |_, _| unreachable_get());
    assert_eq!(
        lib.library_index().await,
        FetchResult::Unavailable {
            reason: "PLEX_URL is not configured".to_owned()
        }
    );
}

fn unreachable_get() -> Arc<dyn PlexGet> {
    Arc::new(Scripted(Err("unused".to_owned())))
}
