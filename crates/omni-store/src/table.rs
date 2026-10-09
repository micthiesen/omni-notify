//! Relational tables beside `blobs` (mitools `Table`): `pets` and
//! `pet_weight_history`, created idempotently and accessed with
//! `INSERT OR IGNORE` / `INSERT OR REPLACE` / `SELECT * ... WHERE`.

use std::marker::PhantomData;

use rusqlite::params_from_iter;
use serde::{Deserialize, Serialize};

pub use rusqlite::Row;
pub use rusqlite::types::Value as SqlValue;

use crate::docstore::sqlite_err;
use crate::{Store, StoreError};

/// A row type with its schema. `DDL` statements must be idempotent
/// (`CREATE ... IF NOT EXISTS`); they run when the table handle is made.
pub trait TableRow: Sized + Send + Sync + 'static {
    const NAME: &'static str;
    /// Column names in insert order; `to_values` yields values in this order.
    const COLUMNS: &'static [&'static str];
    const DDL: &'static [&'static str];
    fn to_values(&self) -> Vec<SqlValue>;
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self>;
}

/// Typed access to one relational table.
pub struct Table<T> {
    store: Store,
    _row: PhantomData<fn() -> T>,
}

impl<T> Clone for Table<T> {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            _row: PhantomData,
        }
    }
}

impl<T: TableRow> Table<T> {
    /// `Table.make`: ensures the table and its indexes exist.
    pub(crate) async fn make(store: Store) -> Result<Self, StoreError> {
        store
            .with_connection(|conn| {
                for statement in T::DDL {
                    conn.execute_batch(statement).map_err(sqlite_err)?;
                }
                Ok(())
            })
            .await?;
        Ok(Self {
            store,
            _row: PhantomData,
        })
    }

    /// The underlying store.
    pub fn store(&self) -> &Store {
        &self.store
    }

    fn insert_sql(verb: &str) -> String {
        let placeholders = vec!["?"; T::COLUMNS.len()].join(", ");
        format!(
            "{verb} INTO {} ({}) VALUES ({placeholders})",
            T::NAME,
            T::COLUMNS.join(", ")
        )
    }

    /// `INSERT OR IGNORE`; `true` if a row was inserted.
    pub async fn insert(&self, row: &T) -> Result<bool, StoreError> {
        let sql = Self::insert_sql("INSERT OR IGNORE");
        let values = row.to_values();
        self.store
            .with_connection(move |conn| {
                conn.prepare_cached(&sql)
                    .and_then(|mut stmt| stmt.execute(params_from_iter(values)))
                    .map(|changes| changes > 0)
                    .map_err(sqlite_err)
            })
            .await
    }

    /// `INSERT OR REPLACE`.
    pub async fn upsert(&self, row: &T) -> Result<(), StoreError> {
        let sql = Self::insert_sql("INSERT OR REPLACE");
        let values = row.to_values();
        self.store
            .with_connection(move |conn| {
                conn.prepare_cached(&sql)
                    .and_then(|mut stmt| stmt.execute(params_from_iter(values)))
                    .map(|_| ())
                    .map_err(sqlite_err)
            })
            .await
    }

    /// `SELECT * FROM <table> WHERE <where_clause>` with positional parameters.
    pub async fn query(
        &self,
        where_clause: &str,
        params: Vec<SqlValue>,
    ) -> Result<Vec<T>, StoreError> {
        self.select(
            format!("SELECT * FROM {} WHERE {where_clause}", T::NAME),
            params,
        )
        .await
    }

    /// Every row.
    pub async fn all(&self) -> Result<Vec<T>, StoreError> {
        self.select(format!("SELECT * FROM {}", T::NAME), Vec::new())
            .await
    }

    async fn select(&self, sql: String, params: Vec<SqlValue>) -> Result<Vec<T>, StoreError> {
        self.store
            .with_connection(move |conn| {
                conn.prepare_cached(&sql)
                    .and_then(|mut stmt| {
                        stmt.query_map(params_from_iter(params), |row| T::from_row(row))?
                            .collect()
                    })
                    .map_err(sqlite_err)
            })
            .await
    }

    /// Deletes every row.
    pub async fn clear(&self) -> Result<(), StoreError> {
        let sql = format!("DELETE FROM {}", T::NAME);
        self.store
            .with_connection(move |conn| conn.execute(&sql, []).map(|_| ()).map_err(sqlite_err))
            .await
    }
}

/// The pet tracker tables (`src/pet-tracker/persistence.ts`), with the DDL
/// mitools `Table.make` generates for them.
pub mod pets {
    use super::{Deserialize, Row, Serialize, SqlValue, TableRow};

    /// `pets` row.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct PetRow {
        pub pet_id: String,
        pub name: String,
        pub current_weight: f64,
        /// ISO timestamp text.
        pub updated_at: String,
    }

    impl TableRow for PetRow {
        const NAME: &'static str = "pets";
        const COLUMNS: &'static [&'static str] =
            &["pet_id", "name", "current_weight", "updated_at"];
        const DDL: &'static [&'static str] = &[
            "CREATE TABLE IF NOT EXISTS pets (pet_id TEXT, name TEXT NOT NULL, \
             current_weight REAL NOT NULL, updated_at TEXT NOT NULL, PRIMARY KEY (pet_id))",
        ];

        fn to_values(&self) -> Vec<SqlValue> {
            vec![
                SqlValue::Text(self.pet_id.clone()),
                SqlValue::Text(self.name.clone()),
                SqlValue::Real(self.current_weight),
                SqlValue::Text(self.updated_at.clone()),
            ]
        }

        fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
            Ok(Self {
                pet_id: row.get("pet_id")?,
                name: row.get("name")?,
                current_weight: row.get("current_weight")?,
                updated_at: row.get("updated_at")?,
            })
        }
    }

    /// `pet_weight_history` row.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct WeightHistoryRow {
        pub pet_id: String,
        /// ISO timestamp text.
        pub timestamp: String,
        pub weight: f64,
    }

    impl TableRow for WeightHistoryRow {
        const NAME: &'static str = "pet_weight_history";
        const COLUMNS: &'static [&'static str] = &["pet_id", "timestamp", "weight"];
        const DDL: &'static [&'static str] = &[
            "CREATE TABLE IF NOT EXISTS pet_weight_history (pet_id TEXT NOT NULL, \
             timestamp TEXT NOT NULL, weight REAL NOT NULL, PRIMARY KEY (pet_id, timestamp))",
            "CREATE INDEX IF NOT EXISTS idx_pet_weight_history_pet_id_timestamp \
             ON pet_weight_history (pet_id, timestamp)",
        ];

        fn to_values(&self) -> Vec<SqlValue> {
            vec![
                SqlValue::Text(self.pet_id.clone()),
                SqlValue::Text(self.timestamp.clone()),
                SqlValue::Real(self.weight),
            ]
        }

        fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
            Ok(Self {
                pet_id: row.get("pet_id")?,
                timestamp: row.get("timestamp")?,
                weight: row.get("weight")?,
            })
        }
    }
}
