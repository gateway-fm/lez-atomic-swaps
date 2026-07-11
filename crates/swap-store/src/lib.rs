//! Persistent swap repository.

use std::{path::Path, time::Duration};

use lez_swap_core::{SwapCoordinator, SwapId};
use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;

const SCHEMA_VERSION: i64 = 1;

/// Persistent-store failure.
#[derive(Debug, Error)]
pub enum StoreError {
    /// `SQLite` operation failed.
    #[error("SQLite swap-store operation failed")]
    Sqlite(#[from] rusqlite::Error),
    /// Durable state could not be encoded or decoded.
    #[error("swap state serialization failed")]
    Serialization(#[from] serde_json::Error),
}

/// Single-process `SQLite` repository for durable swap aggregates.
#[derive(Debug)]
pub struct SqliteSwapStore {
    connection: Connection,
}

impl SqliteSwapStore {
    /// Opens or creates a store and applies the current schema.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` cannot open, configure, or migrate the database.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS swaps (
                id             TEXT PRIMARY KEY NOT NULL,
                schema_version INTEGER NOT NULL,
                state_json     TEXT NOT NULL
            ) STRICT;
            ",
        )?;
        Ok(Self { connection })
    }

    /// Atomically inserts or replaces one complete swap aggregate.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when encoding or writing the aggregate fails.
    pub fn save(&self, swap: &SwapCoordinator) -> Result<(), StoreError> {
        let state_json = serde_json::to_string(swap)?;
        self.connection.execute(
            "
            INSERT INTO swaps (id, schema_version, state_json)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(id) DO UPDATE SET
                schema_version = excluded.schema_version,
                state_json = excluded.state_json
            ",
            params![swap.id().as_str(), SCHEMA_VERSION, state_json],
        )?;
        Ok(())
    }

    /// Loads a swap by stable ID.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when reading or decoding the stored aggregate fails.
    pub fn load(&self, id: &SwapId) -> Result<Option<SwapCoordinator>, StoreError> {
        let encoded = self
            .connection
            .query_row(
                "SELECT state_json FROM swaps WHERE id = ?1 AND schema_version = ?2",
                params![id.as_str(), SCHEMA_VERSION],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        encoded
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(StoreError::from)
    }
}
