//! Port of `src/observer-repair/arr.spec.ts` (planning is pure; inspection and
//! mutation run against wiremock), plus a record-mode case.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_arr::SideEffects;
use omni_arr::observer_repair::arr::{
    ArrInspection, ArrRepairClient, ArrRepairClientConfig, ArrRepairInstruction, ArrRepairKind,
    ArrRepairMedia, ArrRepairScope, EventType, HistoryData, Id, RepairEpisode, RepairFile,
    RepairHistory, RepairInstructionAction,
};
use omni_http::SideEffectMode;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn episode(id: i64, number: i64, has_file: bool, file_id: i64) -> RepairEpisode {
    RepairEpisode {
        id: Id(id),
        series_id: Id(7),
        season_number: 2,
        episode_number: number,
        has_file,
        monitored: true,
        episode_file_id: file_id,
    }
}

fn history(id: i64, event: &str, episode_id: i64) -> RepairHistory {
    RepairHistory {
        id: Id(id),
        series_id: Some(Id(7)),
        movie_id: None,
        episode_id: Some(Id(episode_id)),
        source_title: "Show.S02E14.1080p-GRP".into(),
        download_id: Some("download-1".into()),
        event_type: EventType::Name(event.into()),
        data: HistoryData {
            file_id: Some("20".into()),
            imported_path: None,
        },
    }
}

fn inspected() -> ArrInspection {
    ArrInspection {
        kind: ArrRepairKind::Sonarr,
        id: 7,
        title: "Show".into(),
        monitored: true,
        episodes: vec![episode(10, 14, true, 20)],
        files: vec![RepairFile {
            id: Id(20),
            series_id: Some(Id(7)),
            movie_id: None,
            path: "/tv/Show/S02E14.mkv".into(),
            scene_name: Some("Show.S02E14.1080p-GRP".into()),
        }],
        history: vec![
            history(30, "downloadFolderImported", 10),
            history(31, "grabbed", 10),
        ],
        queue: vec![],
    }
}

fn client_for(url: String, mode: SideEffectMode) -> (ArrRepairClient, SideEffects) {
    let side_effects = SideEffects::new(mode);
    let client = ArrRepairClient::new(ArrRepairClientConfig {
        kind: ArrRepairKind::Sonarr,
        url,
        api_key: "key".into(),
        http: omni_testkit::no_network(),
        side_effects: side_effects.clone(),
    })
    .unwrap();
    (client, side_effects)
}

fn client() -> ArrRepairClient {
    client_for("http://127.0.0.1:9".into(), SideEffectMode::Live).0
}

fn instruction(action: RepairInstructionAction, episodes: &[i64]) -> ArrRepairInstruction {
    ArrRepairInstruction {
        action,
        season: Some(2),
        episodes: episodes.to_vec(),
    }
}

// describe("ArrRepairClient planning")

#[test]
fn plans_an_exact_imported_file_and_matching_grabbed_release() {
    let scope = client()
        .plan(
            &inspected(),
            &instruction(RepairInstructionAction::Replace, &[14]),
        )
        .unwrap();
    assert_eq!(scope.episode_ids, vec![10]);
    assert_eq!(scope.file_ids, vec![20]);
    assert_eq!(scope.history_ids, vec![31]);
    assert_eq!(scope.releases, vec!["Show.S02E14.1080p-GRP".to_owned()]);
}

#[test]
fn rejects_a_shared_file_containing_episodes_outside_the_requested_scope() {
    let shared = ArrInspection {
        episodes: vec![episode(10, 14, true, 20), episode(11, 15, true, 20)],
        ..inspected()
    };
    let error = client()
        .plan(
            &shared,
            &instruction(RepairInstructionAction::Replace, &[14]),
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("outside the requested scope"),
        "{error}"
    );
}

#[test]
fn rejects_a_season_release_whose_grab_covers_episodes_outside_the_requested_scope() {
    let pack = ArrInspection {
        history: vec![
            history(30, "downloadFolderImported", 10),
            history(31, "grabbed", 11),
        ],
        ..inspected()
    };
    let error = client()
        .plan(&pack, &instruction(RepairInstructionAction::Replace, &[14]))
        .unwrap_err();
    assert!(error.to_string().contains("outside scope"), "{error}");
}

#[test]
fn searches_only_missing_episodes_while_preserving_present_episodes() {
    let partial = ArrInspection {
        episodes: vec![episode(10, 14, true, 20), episode(11, 15, false, 0)],
        ..inspected()
    };
    let scope = client()
        .plan(
            &partial,
            &instruction(RepairInstructionAction::SearchMissing, &[]),
        )
        .unwrap();
    assert_eq!(scope.episode_ids, vec![11]);
    assert!(scope.file_ids.is_empty());
}

#[test]
fn rejects_nonexistent_specific_episodes() {
    assert!(
        client()
            .plan(
                &inspected(),
                &instruction(RepairInstructionAction::SearchMissing, &[99])
            )
            .is_err()
    );
}

// describe("ArrRepairClient inspection and mutation")

#[tokio::test]
async fn uses_the_series_endpoint_when_both_external_ids_are_present() {
    let server = MockServer::start().await;
    let routes = [
        (
            "/api/v3/series",
            json!([{ "id": 7, "title": "Show", "tvdbId": 123, "tmdbId": 456, "monitored": true }]),
        ),
        (
            "/api/v3/episode",
            json!([{ "id": 10, "seriesId": 7, "seasonNumber": 2, "episodeNumber": 14, "hasFile": true, "monitored": true, "episodeFileId": 20 }]),
        ),
        (
            "/api/v3/episodefile",
            json!([{ "id": 20, "seriesId": 7, "path": "/x", "sceneName": "Show.S02E14" }]),
        ),
        (
            "/api/v3/history/series",
            json!([{ "id": 30, "seriesId": 7, "episodeId": 10, "sourceTitle": "Show.S02E14.1080p-GRP", "downloadId": "download-1", "eventType": "downloadFolderImported", "data": { "fileId": "20" } }]),
        ),
        ("/api/v3/queue", json!({ "totalRecords": 0, "records": [] })),
    ];
    for (at, body) in routes {
        Mock::given(path(at))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
    }
    let (client, _) = client_for(server.uri(), SideEffectMode::Live);

    let inspection = client
        .inspect(&ArrRepairMedia {
            tmdb_id: Some(456),
            tvdb_id: Some(123),
            media_type: Some("tv".into()),
        })
        .await
        .unwrap();

    assert_eq!(inspection.id, 7);
    let requests = server.received_requests().await.unwrap();
    assert!(
        requests[0]
            .url
            .as_str()
            .contains("/api/v3/series?tvdbId=123")
    );
}

fn scope() -> ArrRepairScope {
    ArrRepairScope {
        kind: ArrRepairKind::Sonarr,
        id: 7,
        title: "Show".into(),
        description: String::new(),
        episode_ids: vec![10],
        file_ids: vec![20],
        history_ids: vec![30],
        releases: vec!["Show.S02E14.1080p-GRP".into()],
    }
}

#[tokio::test]
async fn blocklists_verifies_deletes_verifies_then_starts_search() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v3/command"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 99 })))
        .mount(&server)
        .await;
    Mock::given(path("/api/v3/blocklist"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "records": [{ "sourceTitle": "Show.S02E14.1080p-GRP", "seriesId": 7 }] }),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/episodefile"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v3/history/failed/30"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v3/episodefile/20"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let (client, _) = client_for(server.uri(), SideEffectMode::Live);

    assert_eq!(client.replace(&scope()).await.unwrap(), 99);

    let calls: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.url.path().trim_start_matches("/api/v3/").to_owned())
        .collect();
    assert_eq!(
        calls,
        vec![
            "history/failed/30",
            "blocklist",
            "episodefile/20",
            "episodefile",
            "command"
        ]
    );
}

#[tokio::test]
async fn record_mode_stops_before_any_mutation_is_sent() {
    let server = MockServer::start().await;
    let (client, side_effects) = client_for(server.uri(), SideEffectMode::Record);

    assert!(client.replace(&scope()).await.is_err());

    assert!(server.received_requests().await.unwrap().is_empty());
    let recorded = side_effects.recorded();
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].url.ends_with("/api/v3/history/failed/30"));
}
