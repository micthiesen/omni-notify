//! The SQLite document store shared with the TypeScript service
//! (ARCHITECTURE.md sections 3.3 and 4).
//!
//! The `blobs` table, its key encoding and its CBOR payloads are a contract:
//! Rust writes must decode in node-cbor to the same JS values TS would write.

pub mod cbor;
mod docstore;
pub mod entity;
pub mod logs_gz;
pub mod table;

pub use cbor::JsValue;
pub use docstore::{DocMeta, DocOps, DocWrite, Docs, RawRow, Store, StoreOptions, Tx, like_prefix};
pub use entity::{Entity, EntityOps, EntityWrite};
pub use logs_gz::LogLine;

#[derive(thiserror::Error, Debug)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(String),
    #[error("corrupt row {pk}: {reason}")]
    CorruptRow { pk: String, reason: String },
    #[error("invalid key: {0}")]
    InvalidKey(String),
    #[error("decode {pk}: {source}")]
    Decode {
        pk: String,
        source: cbor::DecodeError,
    },
    #[error("encode {pk}: {source}")]
    Encode {
        pk: String,
        source: cbor::EncodeError,
    },
    #[error("validation failed for {entity}: {reason}")]
    Validation {
        entity: &'static str,
        reason: String,
    },
    #[error("store closed")]
    Closed,
}
