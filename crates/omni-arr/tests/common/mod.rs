//! Fixtures shared by the Arr recovery integration tests.
#![allow(dead_code)]

use omni_arr::arr_recovery::persistence::{ActionPhase, NotificationState, RecoveryAction};
use omni_arr::arr_recovery::{Decision, DecisionSource, ImportFile, Target};

pub fn movie_target() -> Target {
    Target {
        id: 10,
        title: "Example Movie".into(),
        year: 2026,
        monitored: true,
        has_file: false,
        path: "/movies/Example Movie".into(),
        episode_ids: vec![],
        episodes: vec![],
        alternate_titles: vec![],
    }
}

pub fn movie_file() -> ImportFile {
    ImportFile {
        folder_name: None,
        id: 20,
        path: "/downloads/example/Example.Movie.2026.mkv".into(),
        name: "Example.Movie.2026.mkv".into(),
        size: 1_000.0,
        series_id: None,
        movie_id: Some(10),
        season_number: None,
        episode_ids: vec![],
        quality: serde_json::Map::new(),
        languages: None,
        release_group: None,
        indexer_flags: None,
        release_type: None,
        rejections: vec![],
    }
}

pub fn action(
    download_id: &str,
    target: Target,
    decision: Decision,
    phase: ActionPhase,
    created_at: i64,
) -> RecoveryAction {
    RecoveryAction {
        download_id: download_id.into(),
        title: "Example.Movie.2026".into(),
        target,
        files: vec![],
        output_path: format!("/downloads/{download_id}"),
        decision,
        phase,
        created_at,
        updated_at: created_at,
        command_id: None,
        error: None,
        notification: NotificationState::Sent,
    }
}

pub fn replacement(reason: &str, source: DecisionSource) -> Decision {
    Decision::Remove {
        reason: reason.into(),
        source,
        replace: true,
    }
}
