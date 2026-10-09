//! Port of `src/arr-recovery/client.spec.ts` against a wiremock Sonarr/Radarr,
//! plus a record-mode case (mutations are captured, never sent).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_arr::SideEffects;
use omni_arr::arr_recovery::client::{ArrClientConfig, HttpArrClient};
use omni_arr::arr_recovery::{
    ArrClient, ArrKind, ImportFile, Language, QueueItem, Target, TargetEpisode,
};
use omni_http::SideEffectMode;
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn quality() -> Value {
    json!({
        "quality": { "id": 7, "name": "Bluray-1080p", "source": "bluray" },
        "revision": { "version": 1, "real": 0, "isRepack": false },
        "retained": "raw-value",
    })
}

fn import_file() -> ImportFile {
    ImportFile {
        folder_name: Some("Show".into()),
        id: 1,
        path: "/downloads/Show/Show.S01E01.mkv".into(),
        name: "Show.S01E01".into(),
        size: 1_000.0,
        series_id: Some(9),
        movie_id: None,
        season_number: Some(1),
        episode_ids: vec![101],
        quality: quality().as_object().unwrap().clone(),
        languages: Some(vec![Language {
            id: 1,
            name: "English".into(),
        }]),
        release_group: Some("GROUP".into()),
        indexer_flags: Some(0),
        release_type: Some("singleEpisode".into()),
        rejections: vec![],
    }
}

fn episode(id: i64, number: i64, title: &str) -> TargetEpisode {
    TargetEpisode {
        id,
        season_number: 1,
        episode_number: number,
        title: title.into(),
        has_file: false,
        monitored: true,
    }
}

fn target() -> Target {
    Target {
        id: 9,
        title: "Show".into(),
        year: 2020,
        monitored: true,
        has_file: false,
        path: "/tv/Show".into(),
        episode_ids: vec![101],
        episodes: vec![episode(101, 1, "Pilot")],
        alternate_titles: vec![],
    }
}

fn client_with(
    server: &MockServer,
    kind: ArrKind,
    base: &str,
    api_key: &str,
    mode: SideEffectMode,
) -> (HttpArrClient, SideEffects) {
    let side_effects = SideEffects::new(mode);
    let client = HttpArrClient::new(ArrClientConfig {
        kind,
        url: format!("{}{base}", server.uri()),
        api_key: api_key.into(),
        http: omni_testkit::no_network(),
        side_effects: side_effects.clone(),
        local_files: false,
    })
    .unwrap();
    (client, side_effects)
}

fn client(server: &MockServer, kind: ArrKind) -> HttpArrClient {
    client_with(server, kind, "", "secret", SideEffectMode::Live).0
}

async fn respond(server: &MockServer, http_method: &str, at: &str, body: Value) {
    Mock::given(method(http_method))
        .and(path(at))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

fn queue_item(id: i64, episode_id: i64) -> QueueItem {
    QueueItem {
        id,
        download_id: "pack".into(),
        title: "Pack".into(),
        status: "completed".into(),
        tracked_download_status: "warning".into(),
        tracked_download_state: "importBlocked".into(),
        status_messages: vec![],
        size: 1.0,
        sizeleft: 0.0,
        output_path: None,
        added: None,
        series_id: Some(9),
        episode_id: Some(episode_id),
        movie_id: None,
        protocol: None,
        download_client: None,
    }
}

#[tokio::test]
async fn reads_every_queue_page_and_sends_bounded_authenticated_requests() {
    let server = MockServer::start().await;
    let mut first: Vec<Value> = (0..249)
        .map(|index| {
            json!({
                "id": index + 1,
                "downloadId": format!("download-{index}"),
                "title": format!("Release {index}"),
                "status": "completed",
                "trackedDownloadStatus": "warning",
                "trackedDownloadState": "importBlocked",
                "statusMessages": [{ "title": "Import", "messages": ["Blocked"] }],
                "size": 100,
                "sizeleft": 0,
                "outputPath": format!("/downloads/{index}"),
                "seriesId": 9,
                "episodeId": index + 100,
                "protocol": "usenet",
                "downloadClient": "NZBGet",
            })
        })
        .collect();
    first.push(json!({
        "id": 250, "downloadId": null, "title": "Pending release", "status": "delay",
        "trackedDownloadStatus": null, "trackedDownloadState": null, "statusMessages": null,
        "size": 100, "sizeleft": 100, "outputPath": null, "seriesId": 9, "episodeId": 900,
        "protocol": "usenet", "downloadClient": "NZBGet",
    }));
    Mock::given(method("GET"))
        .and(path("/base/api/v3/queue"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "page": 1, "pageSize": 250, "totalRecords": 251, "records": first }),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/base/api/v3/queue"))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "page": 2, "pageSize": 250, "totalRecords": 251,
            "records": [{ "id": 251, "downloadId": "last", "title": "Last", "status": "completed", "size": 1, "sizeLeft": 0 }],
        })))
        .mount(&server)
        .await;
    let (client, _) = client_with(
        &server,
        ArrKind::Sonarr,
        "/base",
        "secret",
        SideEffectMode::Live,
    );

    let items = client.queue().await.unwrap();

    assert_eq!(items.len(), 250);
    assert_eq!(items[0].tracked_download_state, "importBlocked");
    assert_eq!(items[0].sizeleft, 0.0);
    assert_eq!(items[0].episode_id, Some(100));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0]
            .url
            .as_str()
            .contains("/base/api/v3/queue?page=1&pageSize=250")
    );
    assert_eq!(requests[0].headers.get("X-Api-Key").unwrap(), "secret");
    assert_eq!(
        requests[0].headers.get("User-Agent").unwrap(),
        "OpenAI File Downloader, XaiImageApiFetch/1.0"
    );
}

#[tokio::test]
async fn previews_a_targeted_sonarr_import_and_retains_the_full_quality_object() {
    let server = MockServer::start().await;
    respond(
        &server,
        "GET",
        "/api/v3/manualimport",
        json!([{
            "id": 12, "path": "/downloads/Show/Show.S01E01.mkv", "folderName": "Show",
            "name": "Show.S01E01", "size": 1000, "series": { "id": 9, "ignored": true },
            "seasonNumber": 1, "episodes": [{ "id": 101, "title": "Pilot" }], "quality": quality(),
            "languages": [{ "id": 1, "name": "English" }], "releaseGroup": "GROUP",
            "indexerFlags": 0, "releaseType": "singleEpisode", "rejections": [],
        }]),
    )
    .await;

    let files = client(&server, ArrKind::Sonarr)
        .preview("download-id")
        .await
        .unwrap();

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].folder_name.as_deref(), Some("Show"));
    assert_eq!(files[0].series_id, Some(9));
    assert_eq!(files[0].episode_ids, vec![101]);
    assert_eq!(Value::Object(files[0].quality.clone()), quality());
    let request = &server.received_requests().await.unwrap()[0];
    let query: Vec<(String, String)> = request.url.query_pairs().into_owned().collect();
    assert_eq!(
        query,
        vec![("downloadId".to_owned(), "download-id".to_owned())]
    );
}

#[tokio::test]
async fn loads_every_targeted_episode_in_a_sonarr_queue_group() {
    let server = MockServer::start().await;
    respond(
        &server,
        "GET",
        "/api/v3/series/9",
        json!({ "id": 9, "title": "Show", "year": 2020, "monitored": true, "path": "/tv/Show", "alternateTitles": [{ "title": "Alias" }] }),
    )
    .await;
    respond(
        &server,
        "GET",
        "/api/v3/episode",
        json!([
            { "id": 101, "seasonNumber": 1, "episodeNumber": 1, "title": "One", "hasFile": true, "monitored": true, "episodeFileId": 51 },
            { "id": 102, "seasonNumber": 1, "episodeNumber": 2, "title": "Two", "hasFile": false, "monitored": true, "episodeFileId": 0 },
            { "id": 999, "seasonNumber": 2, "episodeNumber": 1, "title": "Other", "hasFile": false, "monitored": true },
        ]),
    )
    .await;

    let result = client(&server, ArrKind::Sonarr)
        .target(&[queue_item(1, 101), queue_item(2, 102)])
        .await
        .unwrap();

    assert_eq!(result.episode_ids, vec![101, 102]);
    assert_eq!(
        result.episodes.iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![101, 102]
    );
    assert!(!result.has_file);
    assert_eq!(result.alternate_titles, vec!["Alias".to_owned()]);
}

#[tokio::test]
async fn filters_grab_history_by_the_exact_download_id() {
    let server = MockServer::start().await;
    respond(
        &server,
        "GET",
        "/api/v3/history",
        json!({ "page": 1, "pageSize": 250, "totalRecords": 2, "records": [
            { "downloadId": "wanted", "sourceTitle": "Release", "seriesId": 9, "episodeId": 101, "eventType": "grabbed", "date": "2026-09-12T01:00:00Z" },
            { "downloadId": "other", "sourceTitle": "Other", "seriesId": 9, "episodeId": 102, "eventType": "grabbed", "date": "2026-09-12T00:00:00Z" },
        ]}),
    )
    .await;

    let grabs = client(&server, ArrKind::Sonarr)
        .history("wanted")
        .await
        .unwrap();

    assert_eq!(grabs.len(), 1);
    let request = &server.received_requests().await.unwrap()[0];
    let query: std::collections::HashMap<String, String> =
        request.url.query_pairs().into_owned().collect();
    assert_eq!(query["downloadId"], "wanted");
    assert_eq!(query["eventType"], "1");
}

#[tokio::test]
async fn posts_the_exact_manual_import_and_removal_controls() {
    let server = MockServer::start().await;
    respond(
        &server,
        "POST",
        "/api/v3/command",
        json!({ "id": 41, "status": "queued" }),
    )
    .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v3/queue/77"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let client = client(&server, ArrKind::Sonarr);

    assert_eq!(
        client
            .import_files("download-id", &[import_file()])
            .await
            .unwrap(),
        41
    );
    client.remove(77, true).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(
        body,
        json!({
            "name": "ManualImport",
            "importMode": "auto",
            "files": [{
                "path": "/downloads/Show/Show.S01E01.mkv",
                "folderName": "Show",
                "seriesId": 9,
                "episodeIds": [101],
                "quality": quality(),
                "languages": [{ "id": 1, "name": "English" }],
                "releaseGroup": "GROUP",
                "indexerFlags": 0,
                "releaseType": "singleEpisode",
                "downloadId": "download-id",
            }],
        })
    );
    assert_eq!(requests[1].url.path(), "/api/v3/queue/77");
    let query: Vec<(String, String)> = requests[1].url.query_pairs().into_owned().collect();
    assert_eq!(
        query,
        vec![
            ("removeFromClient".to_owned(), "true".to_owned()),
            ("blocklist".to_owned(), "true".to_owned()),
            ("skipRedownload".to_owned(), "true".to_owned()),
        ]
    );
}

async fn sonarr_files(server: &MockServer, episodes: Value, files: Value) {
    respond(server, "GET", "/api/v3/episode", episodes).await;
    respond(server, "GET", "/api/v3/episodefile", files).await;
}

#[tokio::test]
async fn verifies_sonarr_imports_by_episode_file_identity_and_source_scene_name() {
    let server = MockServer::start().await;
    sonarr_files(
        &server,
        json!([{ "id": 101, "seasonNumber": 1, "episodeNumber": 1, "title": "Pilot", "hasFile": true, "monitored": true, "episodeFileId": 51 }]),
        json!([{ "id": 51, "path": "/tv/Show/Season 01/01 - Pilot.mkv", "relativePath": "Season 01/01 - Pilot.mkv", "sceneName": "Show.S01E01" }]),
    )
    .await;
    let client = client(&server, ArrKind::Sonarr);

    assert!(
        client
            .verify_imported(&target(), &[import_file()])
            .await
            .unwrap()
    );
    let different = ImportFile {
        name: "Different.Release".into(),
        ..import_file()
    };
    assert!(
        !client
            .verify_imported(&target(), &[different])
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn verifies_obfuscated_imports_against_the_release_folder_and_exact_byte_size() {
    let file = ImportFile {
        name: "NqFGW2VSR2C49AkyiFgnB6G".into(),
        folder_name: Some("Show.S01E01.1080p-GROUP".into()),
        size: 1234.0,
        ..import_file()
    };
    for (size, expected) in [(1234, true), (4321, false)] {
        let server = MockServer::start().await;
        sonarr_files(
            &server,
            json!([{ "id": 101, "seasonNumber": 1, "episodeNumber": 1, "title": "Pilot", "hasFile": true, "monitored": true, "episodeFileId": 51 }]),
            json!([{ "id": 51, "path": "/tv/Show/Season 01/01 - Pilot.mkv", "relativePath": "Season 01/01 - Pilot.mkv", "sceneName": "Show.S01E01.1080p-GROUP", "size": size }]),
        )
        .await;
        let verified = client(&server, ArrKind::Sonarr)
            .verify_imported(&target(), std::slice::from_ref(&file))
            .await
            .unwrap();
        assert_eq!(verified, expected, "size {size}");
    }
}

#[tokio::test]
async fn ties_every_sonarr_source_to_the_file_id_of_its_own_episodes() {
    let server = MockServer::start().await;
    sonarr_files(
        &server,
        json!([
            { "id": 101, "seasonNumber": 1, "episodeNumber": 1, "title": "One", "hasFile": true, "monitored": true, "episodeFileId": 52 },
            { "id": 102, "seasonNumber": 1, "episodeNumber": 2, "title": "Two", "hasFile": true, "monitored": true, "episodeFileId": 51 },
        ]),
        json!([
            { "id": 51, "path": "/tv/Show/One.mkv", "relativePath": "One.mkv", "sceneName": "One" },
            { "id": 52, "path": "/tv/Show/Two.mkv", "relativePath": "Two.mkv", "sceneName": "Two" },
        ]),
    )
    .await;
    let two_episode_target = Target {
        episode_ids: vec![101, 102],
        episodes: vec![episode(101, 1, "Pilot"), episode(102, 2, "Two")],
        ..target()
    };
    let files = [
        ImportFile {
            name: "One".into(),
            episode_ids: vec![101],
            ..import_file()
        },
        ImportFile {
            id: 2,
            name: "Two".into(),
            episode_ids: vec![102],
            ..import_file()
        },
    ];

    assert!(
        !client(&server, ArrKind::Sonarr)
            .verify_imported(&two_episode_target, &files)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn verifies_radarr_imports_against_the_original_or_scene_filename() {
    let server = MockServer::start().await;
    respond(
        &server,
        "GET",
        "/api/v3/movie/4",
        json!({ "id": 4, "title": "Movie", "year": 2024, "monitored": true, "path": "/movies/Movie", "hasFile": true, "movieFileId": 71 }),
    )
    .await;
    respond(
        &server,
        "GET",
        "/api/v3/moviefile",
        json!([{ "id": 71, "movieId": 4, "path": "/movies/Movie/Movie (2024).mkv", "relativePath": "Movie (2024).mkv", "sceneName": "Movie.2024-GROUP", "originalFilePath": "Movie.2024-GROUP.mkv" }]),
    )
    .await;
    let movie_target = Target {
        id: 4,
        title: "Movie".into(),
        episode_ids: vec![],
        episodes: vec![],
        ..target()
    };
    let movie_file = ImportFile {
        path: "/downloads/Movie.2024-GROUP.mkv".into(),
        name: "Movie.2024-GROUP".into(),
        series_id: None,
        movie_id: Some(4),
        episode_ids: vec![],
        ..import_file()
    };

    assert!(
        client(&server, ArrKind::Radarr)
            .verify_imported(&movie_target, &[movie_file])
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn only_confirms_removal_when_the_parent_listing_proves_it_was_readable() {
    let cases = [
        (
            json!({ "directories": [{ "path": "/downloads/another" }], "files": [] }),
            true,
        ),
        (json!({ "directories": [], "files": [] }), false),
        (
            json!({ "directories": [{ "path": "/downloads/removed/" }], "files": [] }),
            false,
        ),
    ];
    for (index, (listing, expected)) in cases.into_iter().enumerate() {
        let server = MockServer::start().await;
        respond(&server, "GET", "/api/v3/filesystem", listing).await;
        let removed = client(&server, ArrKind::Sonarr)
            .verify_removed("/downloads/removed")
            .await
            .unwrap();
        assert_eq!(removed, expected, "case {index}");
        if index == 0 {
            let request = &server.received_requests().await.unwrap()[0];
            let query: std::collections::HashMap<String, String> =
                request.url.query_pairs().into_owned().collect();
            assert_eq!(query["path"], "/downloads");
        }
    }
}

#[tokio::test]
async fn fails_with_a_typed_error_without_exposing_the_api_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({ "error": "no" })))
        .mount(&server)
        .await;
    let (client, _) = client_with(
        &server,
        ArrKind::Sonarr,
        "",
        "top-secret",
        SideEffectMode::Live,
    );

    let failure = client.queue().await.unwrap_err();

    assert_eq!(failure.operation, "read queue");
    assert!(failure.to_string().contains("HTTP 500"));
    assert!(!failure.to_string().contains("top-secret"));
}

#[tokio::test]
async fn record_mode_captures_mutations_without_sending_them() {
    let server = MockServer::start().await;
    let (client, side_effects) = client_with(
        &server,
        ArrKind::Radarr,
        "",
        "secret",
        SideEffectMode::Record,
    );

    assert!(client.import_files("d", &[import_file()]).await.is_err());
    assert!(client.remove(5, false).await.is_err());
    assert!(client.search(&target()).await.is_err());

    assert!(server.received_requests().await.unwrap().is_empty());
    let recorded = side_effects.recorded();
    assert_eq!(
        recorded
            .iter()
            .map(|m| m.method.as_str())
            .collect::<Vec<_>>(),
        vec!["POST", "DELETE", "POST"]
    );
    assert_eq!(
        recorded[2].body,
        Some(json!({ "name": "MoviesSearch", "movieIds": [9] }))
    );
}
