//! Data manager: `/api/data/entities` and `/api/data/entities/:slug`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The key that marks an undecodable row in a listing (`MALFORMED_ROW_KEY`).
pub const MALFORMED_ROW_KEY: &str = "__dataManagerMalformed";

/// One managed entity (`ManagedEntitySummary`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedEntitySummary {
    pub slug: String,
    pub label: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    pub primary_key: Vec<String>,
    pub count: u64,
    /// Encoded CBOR payload bytes, excluding SQLite indexes and page overhead.
    pub storage_bytes: u64,
}

/// `ManagedDataSummary`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedDataSummary {
    /// SQLite page allocation, including all entity and relational table data.
    pub database_size_bytes: u64,
    /// Encoded payload bytes belonging to the managed entities.
    pub entity_storage_bytes: u64,
}

/// `GET /api/data/entities`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntitiesResponse {
    pub entities: Vec<ManagedEntitySummary>,
    pub storage: ManagedDataSummary,
}

/// One stored row as JSON (`DataRow`).
pub type DataRow = Map<String, Value>;

/// `GET /api/data/entities/:slug`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntityRowsResponse {
    pub summary: ManagedEntitySummary,
    pub rows: Vec<DataRow>,
}

/// `DELETE /api/data/entities/:slug` body: the row's primary key, or a
/// malformed row's `{ "__dataManagerMalformed": { rawKey, error } }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeleteRowRequest {
    pub key: Map<String, Value>,
}

/// `DELETE /api/data/entities/:slug` success body (`{ "deleted": true }`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteRowResponse {
    pub deleted: bool,
}

/// The metadata of a malformed row (`MalformedRowMetadata`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MalformedRowMetadata {
    pub raw_key: String,
    pub error: String,
}

/// Paths of the data-manager routes.
pub mod paths {
    pub const ENTITIES: &str = "/api/data/entities";

    /// `/api/data/entities/:slug`.
    pub fn entity(slug: &str) -> String {
        format!("{ENTITIES}/{}", crate::common::encode_uri_component(slug))
    }
}
