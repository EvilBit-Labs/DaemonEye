//! Storage error types for the event store and the legacy `DatabaseManager`.
//!
//! Split out of `storage.rs` (T3 · U1) so the growing event-store engine keeps
//! each module within the project's source-size guidance. The variant set is a
//! superset of the original `DatabaseManager` errors plus event-store-specific
//! conditions (postcard codec failures, schema-version mismatch, the read-only
//! degraded terminal state, and bucket lifecycle errors).

use std::path::PathBuf;
use thiserror::Error;

/// Database operation errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StorageError {
    /// Platform-agnostic friendly errors for directory issues
    #[error("Database directory '{dir}' does not exist")]
    MissingDirectory {
        /// Directory that was expected to exist.
        dir: PathBuf,
        /// Underlying I/O error, when one was produced.
        #[source]
        source: Option<std::io::Error>,
    },

    /// The configured database path's parent exists but is not a directory.
    #[error("Database directory '{path}' is not a directory")]
    NotADirectory {
        /// Offending path.
        path: PathBuf,
    },

    /// Permission was denied while accessing the database directory.
    #[error("Permission denied accessing database directory '{path}'")]
    DirectoryPermissionDenied {
        /// Offending path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The database file could not be opened or created.
    #[error("Failed to open/create database at '{path}'")]
    DatabaseCreationFailed {
        /// Offending path.
        path: PathBuf,
        /// Underlying redb error.
        #[source]
        source: redb::DatabaseError,
    },

    /// Database error (redb).
    #[error("Database error: {0}")]
    DatabaseError(#[from] redb::DatabaseError),

    /// Storage error (redb).
    #[error("Storage error: {0}")]
    StorageError(#[from] redb::StorageError),

    /// Table error (redb).
    #[error("Table error: {0}")]
    TableError(#[from] redb::TableError),

    /// Transaction error (redb).
    #[error("Transaction error: {0}")]
    TransactionError(#[from] redb::TransactionError),

    /// Commit error (redb).
    #[error("Commit error: {0}")]
    CommitError(#[from] redb::CommitError),

    /// Savepoint error (redb) — used by the resumable schema rebuild.
    #[error("Savepoint error: {0}")]
    SavepointError(#[from] redb::SavepointError),

    /// JSON serialization error (legacy `DatabaseManager` paths).
    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    /// Postcard codec failure (event-store value/key encoding).
    #[error("Postcard codec error: {0}")]
    Codec(#[from] postcard::Error),

    /// The on-disk `schema_version` does not match the running binary.
    #[error("Schema version mismatch: store has {found}, binary expects {expected}")]
    SchemaVersionMismatch {
        /// Version recorded in the store.
        found: u32,
        /// Version the running binary understands.
        expected: u32,
    },

    /// The store entered read-only degraded mode after repeated rebuild failures.
    #[error("Event store is in read-only degraded mode: {reason}")]
    DegradedReadOnly {
        /// Human-readable reason for the degraded state.
        reason: String,
    },

    /// A time-bucket table could not be created, opened, or dropped.
    #[error("Bucket '{bucket}' operation failed: {message}")]
    Bucket {
        /// Bucket identifier (e.g. `processes.events@2026-06-14T01`).
        bucket: String,
        /// Description of what went wrong.
        message: String,
    },

    /// Arithmetic on key/offset/bucket math overflowed.
    #[error("Arithmetic overflow in {context}")]
    Overflow {
        /// Where the overflow occurred (key packing, bucket id, retention math).
        context: String,
    },

    /// I/O error against a specific path.
    #[error("I/O error accessing '{path}'")]
    IoError {
        /// Offending path.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },

    /// A requested table was not found.
    #[error("Table not found: {table}")]
    TableNotFound {
        /// Table name.
        table: String,
    },

    /// A requested record was not found.
    #[error("Record not found: {id}")]
    RecordNotFound {
        /// Record identifier.
        id: String,
    },
}
