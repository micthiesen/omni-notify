//! The SQLite document store (see docs/architecture.md, "Data compatibility").
//!
//! The `blobs` table and its key encoding are a contract with the rows already
//! stored: keys are derived exactly as before, and every stored CBOR payload
//! must keep decoding.

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
