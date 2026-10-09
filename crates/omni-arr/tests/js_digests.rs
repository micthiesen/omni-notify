//! JS-derived identities match values computed by the TypeScript code
//! (`observationFingerprint`, `issueRevision`) on 2026-10-09, so persisted
//! fingerprints and revisions written before the Rust port stay comparable.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_arr::arr_recovery::QueueItem;
use omni_arr::arr_recovery::StatusMessage;
use omni_arr::arr_recovery::policy::observation_fingerprint;
use omni_arr::observer::ObserverIssue;
use omni_arr::observer_repair::service::issue_revision;
use serde_json::json;

fn q() -> QueueItem {
    QueueItem {
        id: 10,
        download_id: "download-1".into(),
        title: "House.of.Cards.US.S01E01.1080p.WEB-DL".into(),
        status: "Completed".into(),
        tracked_download_status: "Warning".into(),
        tracked_download_state: "importPending".into(),
        status_messages: vec![
            StatusMessage {
                title: " Import failed ".into(),
                messages: ["b second", " a first ", "", "Ünïcode é", "Zeta"]
                    .map(String::from)
                    .to_vec(),
            },
            StatusMessage {
                title: "Another".into(),
                messages: vec!["x".into()],
            },
        ],
        size: 1_234_567.5,
        sizeleft: 0.0,
        output_path: Some("/downloads/House".into()),
        added: None,
        series_id: Some(42),
        episode_id: None,
        movie_id: None,
        protocol: None,
        download_client: None,
    }
}

#[test]
fn observation_fingerprints_match_typescript() {
    assert_eq!(
        observation_fingerprint(&[q()]),
        "cb3cf50b7e885a51465bbf54c89298867c0ffbd91e87b502fc0b74e25851a155"
    );
    let second = QueueItem {
        id: 11,
        download_id: "download-2".into(),
        episode_id: Some(7),
        added: Some("2026-01-01T00:00:00Z".into()),
        output_path: None,
        size: 1e21,
        ..q()
    };
    assert_eq!(
        observation_fingerprint(&[q(), second]),
        "b3e7d8a666a33a7f760620bfdab218b7aa7b2e10028fcf9215f5283abcd84b5f"
    );
    let movie = QueueItem {
        series_id: None,
        movie_id: Some(5),
        status_messages: vec![],
        ..q()
    };
    assert_eq!(
        observation_fingerprint(&[movie]),
        "d196ec0457db9d6ca0568abc453b55bed57a6c038285982c2fdec2468c5cd86f"
    );
}

#[test]
fn observation_fingerprints_trim_with_javascript_whitespace() {
    // JS `trim` removes U+FEFF and line separators but keeps U+0085.
    let item = QueueItem {
        download_id: "download-ws".into(),
        title: "T".into(),
        status: "completed".into(),
        tracked_download_status: "warning".into(),
        tracked_download_state: "importBlocked".into(),
        status_messages: vec![StatusMessage {
            title: "\u{FEFF}Import failed\u{3000}".into(),
            messages: ["\u{FEFF} b \u{2028}", "\u{0085}x\u{0085}", "\u{FEFF}"]
                .map(String::from)
                .to_vec(),
        }],
        size: 1.0,
        output_path: Some("/d".into()),
        series_id: None,
        ..q()
    };
    assert_eq!(
        observation_fingerprint(&[item]),
        "c062d9722d65a8a362758a54197bd299cedd1ec7a9aff981086b9965020a5b5d"
    );
}

fn issue(overrides: serde_json::Value) -> ObserverIssue {
    let mut base = json!({
        "id": 12, "issueType": 1, "status": 1, "problemSeason": 3, "problemEpisode": 0,
        "updatedAt": "2026-01-01T00:00:00.000Z",
        "media": { "id": 44, "mediaType": "tv", "tmdbId": 123, "tvdbId": null, "status": 5 },
        "comments": [
            { "id": 1, "message": "Freezes \"badly\" é 😀", "user": null },
            { "id": 2, "message": "[Omni repair 12/abc] done", "user": null }
        ]
    });
    let object = base.as_object_mut().unwrap();
    for (key, value) in overrides.as_object().unwrap() {
        if value == "<undefined>" {
            object.remove(key);
        } else {
            object.insert(key.clone(), value.clone());
        }
    }
    serde_json::from_value(base).unwrap()
}

#[test]
fn issue_revisions_match_typescript() {
    assert_eq!(
        issue_revision(&issue(json!({}))),
        "5a681f8483233d3876bce9ba"
    );
    assert_eq!(
        issue_revision(&issue(
            json!({ "problemSeason": "<undefined>", "problemEpisode": null })
        )),
        "f2dc05632b75f4e96c919643"
    );
    assert_eq!(
        issue_revision(&issue(json!({ "media": null, "comments": null }))),
        "37b1607fa824634fae8623f1"
    );
    assert_eq!(
        issue_revision(&issue(
            json!({ "comments": "<undefined>", "media": { "mediaType": "movie" } })
        )),
        "19619e9a293ebc97b879b9ec"
    );
}
