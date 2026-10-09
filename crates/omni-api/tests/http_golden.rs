//! Every DTO against the HTTP fixtures captured from the production TS
//! service (`cargo xtask capture-golden --base http://omni.boris`, read-only
//! GETs). Each fixture body must decode into its
//! DTO and re-encode to the same JSON value: same keys, same nulls, same
//! numbers (JS has one number type, so `1` and `1.0` are equal).

#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/http")
}

/// `{route, status, body}`.
fn fixture(slug: &str) -> (String, u16, Value) {
    let path = fixture_dir().join(format!("{slug}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let value: Value = serde_json::from_str(&text).unwrap();
    (
        value["route"].as_str().unwrap().to_owned(),
        u16::try_from(value["status"].as_u64().unwrap()).unwrap(),
        value["body"].clone(),
    )
}

/// JS number semantics: integral floats compare equal to integers.
fn normalize(value: Value) -> Value {
    match value {
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 9.007_199_254_740_992e15 =>
            {
                #[allow(clippy::cast_possible_truncation)]
                Value::from(f as i64)
            }
            _ => Value::Number(n),
        },
        Value::Array(items) => Value::Array(items.into_iter().map(normalize).collect()),
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, normalize(v))).collect())
        }
        other => other,
    }
}

/// The first path where `a` and `b` differ.
fn first_difference(path: &str, a: &Value, b: &Value) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for key in x.keys().chain(y.keys()) {
                match (x.get(key), y.get(key)) {
                    (Some(v), Some(w)) => {
                        if let Some(d) = first_difference(&format!("{path}.{key}"), v, w) {
                            return Some(d);
                        }
                    }
                    (v, w) => {
                        return Some(format!(
                            "{path}.{key}: fixture {} vs dto {}",
                            v.map_or("<absent>".to_owned(), Value::to_string),
                            w.map_or("<absent>".to_owned(), Value::to_string)
                        ));
                    }
                }
            }
            None
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Some(format!("{path}: length {} vs {}", x.len(), y.len()));
            }
            x.iter()
                .zip(y)
                .enumerate()
                .find_map(|(i, (v, w))| first_difference(&format!("{path}[{i}]"), v, w))
        }
        _ if a == b => None,
        _ => Some(format!("{path}: fixture {a} vs dto {b}")),
    }
}

fn round_trips<T: DeserializeOwned + Serialize>(slug: &str, status: u16) {
    let (route, captured, body) = fixture(slug);
    assert_eq!(captured, status, "{route}");
    let typed: T = serde_json::from_value(body.clone())
        .unwrap_or_else(|e| panic!("{route} does not decode: {e}"));
    let back = normalize(serde_json::to_value(&typed).unwrap());
    let body = normalize(body);
    if let Some(diff) = first_difference("$", &body, &back) {
        panic!("{route} does not round-trip: {diff}");
    }
}

macro_rules! golden {
    ($($name:ident: $ty:ty = $slug:literal $(, $status:literal)?;)*) => {
        $(
            #[test]
            fn $name() {
                round_trips::<$ty>($slug, golden!(@status $($status)?));
            }
        )*
    };
    (@status) => { 200 };
    (@status $s:literal) => { $s };
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Health {
    status: String,
}

golden! {
    health: Health = "api_health";
    tasks: omni_api::tasks::TasksResponse = "api_tasks";
    task_runs: omni_api::runs::RunsResponse = "api_task-runs";
    task_runs_limited: omni_api::runs::RunsResponse = "api_task-runs__limit_5";
    task_runs_for_task: omni_api::runs::RunsResponse = "api_task-runs__task_PressPods";
    run_logs: omni_api::runs::RunLogsResponse =
        "api_task-runs_LiveCheckTask_a05dc9c0-4110-4268-bbb9-1ac83212876f_logs";
    run_logs_presspods: omni_api::runs::RunLogsResponse =
        "api_task-runs_PressPods_5fd43fe5-4e6e-40b6-9753-301436707d9f_logs";
    run_logs_unknown: omni_api::common::ApiErrorBody = "api_task-runs_nope_logs", 404;
    snapshot: omni_api::snapshot::Snapshot = "api_snapshot";
    costs: omni_api::costs::CostsResponse = "api_costs";
    costs_7: omni_api::costs::CostsResponse = "api_costs__days_7";
    costs_90: omni_api::costs::CostsResponse = "api_costs__days_90";
    costs_all: omni_api::costs::CostsResponse = "api_costs__days_all";
    costs_invalid: omni_api::common::ApiErrorBody = "api_costs__days_14", 400;
    data_entities: omni_api::data::EntitiesResponse = "api_data_entities";
    data_rows_cost_migration: omni_api::data::EntityRowsResponse = "api_data_entities_cost-migration";
    data_rows_schedule_state: omni_api::data::EntityRowsResponse =
        "api_data_entities_task-schedule-state";
    data_unknown: omni_api::common::ApiErrorBody = "api_data_entities_nope", 404;
    streamers: omni_api::streamers::StreamersResponse = "api_streamers";
    trigger_channels: omni_api::streamers::TriggerChannelsResponse = "api_trigger-channels";
    streamer_metrics_live: omni_api::streamers::StreamerMetricsResponse =
        "api_streamers_anythingelse_metrics";
    streamer_metrics_offline: omni_api::streamers::StreamerMetricsResponse =
        "api_streamers_darius_metrics";
    streamer_metrics_unknown: omni_api::common::ApiErrorBody = "api_streamers_nobody_metrics", 404;
    streamer_sessions_live: omni_api::streamers::StreamSessionsResponse =
        "api_streamers_anythingelse_sessions";
    streamer_sessions_offline: omni_api::streamers::StreamSessionsResponse =
        "api_streamers_darius_sessions";
    intelligence_details: omni_api::intelligence::IntelligenceDetailsResponse =
        "api_streamers_anythingelse_intelligence-details__limit_5";
    email_activity: omni_api::email::EmailActivitiesResponse = "api_email-activity";
    email_activity_logs: omni_api::email::EmailActivityLogsResponse =
        "api_email-activity_CalendarEvents_23_3Canomalyco_2Fsst_2Fissues_2F6575_2F6075578949_40github_com_3E_logs";
    email_feedback: omni_api::email::EmailFeedbackListResponse = "api_email-feedback";
    email_rules: omni_api::email::EmailRulesResponse = "api_email-rules";
    pets: Vec<omni_api::pets::Pet> = "api_pets";
    podcast_recommendations: omni_api::podcasts::PodcastRecommendationsResponse =
        "api_podcast-recommendations";
    podcast_recommendation: omni_api::podcasts::PodcastRecommendationResponse =
        "api_podcast-recommendations_6fcb0401-7b15-45a7-84c2-3ae7915f6d4b";
    presspods_episodes: omni_api::presspods::PressPodsListResponse = "api_press-pods_episodes";
    presspods_episode: omni_api::presspods::PressPodsEpisodeResponse =
        "api_press-pods_episodes_t_H1l9vKZANwpHusw7NNDw";
    recommendations: omni_api::media::RecommendationsResponse = "api_recommendations";
    recommendation: omni_api::media::RecommendationResponse =
        "api_recommendations_582207dc-59c9-40d4-99ce-4efa687a5e67";
    reminders_status_cross_origin: omni_api::common::ApiErrorBody = "api_reminders_status", 403;
    workspace_papercuts: omni_api::workspaces::WorkspacePapercutsResponse = "api_workspace-papercuts";
    workspaces: omni_api::workspaces::WorkspacesResponse = "api_workspaces";
    workspace_purchase: omni_api::workspaces::WorkspaceResponse = "api_workspaces_purchase-research";
    workspace_marketplace: omni_api::workspaces::WorkspaceResponse =
        "api_workspaces_marketplace-selling";
    workspace_subject: omni_api::workspaces::WorkspaceSubjectResponse =
        "api_workspaces_purchase-research_subjects_3324868c-9a97-4a32-93cf-1cc07a9d2df6";
    briefings: omni_api::briefings::BriefingsResponse = "api_briefings";
    mcp_activity: omni_api::mcp_activity::McpActivityResponse = "api_mcp_activity";
    claude_activity: omni_api::claude::ClaudeActivityResponse = "api_claude_activity";
    pods_rss_unauthorized: omni_api::common::ApiErrorBody = "pods_rss", 401;
}

/// Both taste-profile routes return the unrun Effect object in TS
/// (`{"profile": {"_id": "Effect", ...}}`), a defect the Rust port fixes
/// deliberately (WP07/WP08 reports): the fixtures pin the TS bytes, and the
/// DTO decodes the corrected shape instead.
#[test]
fn taste_profile_fixtures_record_the_ts_defect() {
    for slug in [
        "api_recommendations_taste-profile",
        "api_podcast-recommendations_taste-profile",
    ] {
        let (_, status, body) = fixture(slug);
        assert_eq!(status, 200);
        assert_eq!(body["profile"]["_id"], "Effect", "{slug}");
    }
    let empty: omni_api::media::TasteProfileResponse =
        serde_json::from_value(serde_json::json!({ "profile": null })).unwrap();
    assert!(empty.profile.is_none());
    let empty: omni_api::podcasts::PodcastTasteProfileResponse =
        serde_json::from_value(serde_json::json!({ "profile": null })).unwrap();
    assert!(empty.profile.is_none());
}

/// Every captured fixture has a DTO test above (non-JSON bodies excepted).
#[test]
fn every_json_fixture_is_covered() {
    let source = include_str!("http_golden.rs");
    for entry in std::fs::read_dir(fixture_dir()).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        let Some(slug) = name.strip_suffix(".json") else {
            continue;
        };
        if slug.ends_with(".meta") {
            continue;
        }
        assert!(
            source.contains(&format!("\"{slug}\"")),
            "fixture {name} has no DTO test"
        );
    }
}

#[test]
fn csv_export_fixture_keeps_its_shape() {
    let meta: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture_dir().join(
            "api_pets_PET-b4738d2e-9a37-4d70-b401-a86e56bfd180_export_csv__days_30.meta.json",
        ))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(meta["status"], 200);
    assert!(
        meta["contentType"]
            .as_str()
            .unwrap()
            .starts_with("text/csv")
    );
    let body = std::fs::read_to_string(
        fixture_dir()
            .join("api_pets_PET-b4738d2e-9a37-4d70-b401-a86e56bfd180_export_csv__days_30.body"),
    )
    .unwrap();
    assert!(body.starts_with("timestamp,weight_lbs\n"));
}
