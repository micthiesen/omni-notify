//! Frontend-side wire types for endpoints whose `omni-api` module is still
//! empty (`snapshot`, `data`, both owned by WP14). Field names and nullability
//! follow `frontend/src/api.ts` and `src/server.ts` exactly.

use omni_api::media::OnDeckItem;
use omni_api::runs::Run;
use omni_api::streamers::StreamerView;
use omni_api::tasks::TaskInfo;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// `GET /api/snapshot` and every `snapshot` frame of `/api/events`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub tasks: Vec<TaskInfo>,
    pub streamers: Vec<StreamerView>,
    pub runs: Vec<Run>,
    pub on_deck: Vec<OnDeckItem>,
}

/// One managed entity in the data manager.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataEntity {
    pub slug: String,
    pub label: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    pub primary_key: Vec<String>,
    pub count: f64,
    pub storage_bytes: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataStorageSummary {
    pub database_size_bytes: f64,
    pub entity_storage_bytes: f64,
}

/// `GET /api/data/entities`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DataEntitiesResponse {
    pub entities: Vec<DataEntity>,
    pub storage: DataStorageSummary,
}

/// One stored row as JSON (`DataRow`).
pub type DataRow = Map<String, Value>;

/// `GET /api/data/entities/:slug`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DataRowsResponse {
    pub summary: DataEntity,
    pub rows: Vec<DataRow>,
}

/// `{ "deleted": true }` and `{ "deleted": boolean }` replies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deleted {
    pub deleted: bool,
}

/// `{ "runId": string }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunId {
    pub run_id: String,
}
