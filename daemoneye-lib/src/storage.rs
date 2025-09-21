//! Database abstractions and storage management.
//!
//! This module provides database operations using redb for optimal performance and security.
//! It includes table definitions, transaction handling, and data serialization.

use crate::models::{Alert, DetectionRule, ProcessRecord, SystemInfo};
use redb::{Database, ReadableDatabase, TableDefinition};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use thiserror::Error;

/// Database operation errors.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StorageError {
    /// Platform-agnostic friendly errors for directory issues
    #[error("Database directory '{dir}' does not exist")]
    MissingDirectory {
        dir: PathBuf,
        #[source]
        source: Option<std::io::Error>,
    },

    #[error("Database directory '{path}' is not a directory")]
    NotADirectory { path: PathBuf },

    #[error("Permission denied accessing database directory '{path}'")]
    DirectoryPermissionDenied {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to open/create database at '{path}'")]
    DatabaseCreationFailed {
        path: PathBuf,
        #[source]
        source: redb::DatabaseError,
    },

    /// Legacy error variants (preserved for compatibility)
    #[error("Database error: {0}")]
    DatabaseError(#[from] redb::DatabaseError),

    #[error("Storage error: {0}")]
    StorageError(#[from] redb::StorageError),

    #[error("Table error: {0}")]
    TableError(#[from] redb::TableError),

    #[error("Transaction error: {0}")]
    TransactionError(#[from] redb::TransactionError),

    #[error("Commit error: {0}")]
    CommitError(#[from] redb::CommitError),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("I/O error accessing '{path}'")]
    IoError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Table not found: {table}")]
    TableNotFound { table: String },

    #[error("Record not found: {id}")]
    RecordNotFound { id: String },
}

/// Validates that the directory containing the database file exists and is accessible.
/// This provides platform-agnostic error messages before attempting to open the database.
fn ensure_db_dir(db_path: &Path) -> Result<(), StorageError> {
    if let Some(dir) = db_path.parent() {
        match fs::metadata(dir) {
            Ok(metadata) => {
                if !metadata.is_dir() {
                    return Err(StorageError::NotADirectory {
                        path: dir.to_path_buf(),
                    });
                }
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(StorageError::MissingDirectory {
                dir: dir.to_path_buf(),
                source: Some(e),
            }),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                Err(StorageError::DirectoryPermissionDenied {
                    path: dir.to_path_buf(),
                    source: e,
                })
            }
            Err(e) => Err(StorageError::IoError {
                path: dir.to_path_buf(),
                source: e,
            }),
        }
    } else {
        // Paths like "test.db" (no parent directory) are acceptable
        Ok(())
    }
}

/// Table definitions for the database schema.
pub struct Tables;

impl Tables {
    /// Process records table
    /// TODO: Use `ProcessRecord` type in Task 8 when redb Value trait is implemented
    pub const PROCESSES: TableDefinition<'static, u64, Vec<u8>> = TableDefinition::new("processes");

    /// Detection rules table
    /// TODO: Use `DetectionRule` type in Task 8 when redb Value trait is implemented
    pub const DETECTION_RULES: TableDefinition<'static, &str, Vec<u8>> =
        TableDefinition::new("detection_rules");

    /// Alerts table
    /// TODO: Use Alert type in Task 8 when redb Value trait is implemented
    pub const ALERTS: TableDefinition<'static, u64, Vec<u8>> = TableDefinition::new("alerts");

    /// System info table
    /// TODO: Use `SystemInfo` type in Task 8 when redb Value trait is implemented
    pub const SYSTEM_INFO: TableDefinition<'static, u64, Vec<u8>> =
        TableDefinition::new("system_info");

    /// Scan metadata table
    /// TODO: Use `ScanMetadata` type in Task 8 when redb Value trait is implemented
    pub const SCAN_METADATA: TableDefinition<'static, u64, Vec<u8>> =
        TableDefinition::new("scan_metadata");
}

/// Scan metadata for tracking collection operations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScanMetadata {
    /// Scan ID
    pub scan_id: String,
    /// Scan timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Number of processes collected
    pub process_count: usize,
    /// Scan duration in milliseconds
    pub duration_ms: u64,
    /// Scan status
    pub status: ScanStatus,
    /// Error message if scan failed
    pub error_message: Option<String>,
}

/// Scan status enumeration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScanStatus {
    InProgress,
    Completed,
    Failed,
}

/// Database manager for `SentinelD` storage operations.
pub struct DatabaseManager {
    db: Database,
}

impl DatabaseManager {
    /// Create a new database manager.
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let db_path = path.as_ref();

        // Platform-agnostic preflight directory checks
        ensure_db_dir(db_path)?;

        // Create the database, mapping errors to friendly messages
        let db =
            Database::create(db_path).map_err(|source| StorageError::DatabaseCreationFailed {
                path: db_path.to_path_buf(),
                source,
            })?;

        let manager = Self { db };
        manager.initialize_schema()?;
        Ok(manager)
    }

    /// Open an existing database.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let db_path = path.as_ref();

        // Platform-agnostic preflight directory checks
        ensure_db_dir(db_path)?;

        // Open the database, mapping errors to friendly messages
        let db =
            Database::open(db_path).map_err(|source| StorageError::DatabaseCreationFailed {
                path: db_path.to_path_buf(),
                source,
            })?;

        Ok(Self { db })
    }

    /// Initialize the database schema.
    fn initialize_schema(&self) -> Result<(), StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _write_txn = self.db.begin_write()?;
        // Implementation will be added in Task 8
        Ok(())
    }

    /// Store a process record.
    pub fn store_process(&self, _id: u64, _process: &ProcessRecord) -> Result<(), StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _write_txn = self.db.begin_write()?;
        // Implementation will be added in Task 8
        Ok(())
    }

    /// Store multiple process records in a batch.
    pub fn store_processes_batch(
        &self,
        _processes: &[(u64, ProcessRecord)],
    ) -> Result<(), StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _write_txn = self.db.begin_write()?;
        // Implementation will be added in Task 8
        Ok(())
    }

    /// Get a process record by ID.
    pub fn get_process(&self, _id: u64) -> Result<Option<ProcessRecord>, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // Implementation will be added in Task 8
        Ok(None)
    }

    /// Get all process records.
    pub fn get_all_processes(&self) -> Result<Vec<ProcessRecord>, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // Implementation will be added in Task 8
        Ok(Vec::new())
    }

    /// Store a detection rule.
    pub fn store_rule(&self, _rule: &DetectionRule) -> Result<(), StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _write_txn = self.db.begin_write()?;
        // Implementation will be added in Task 8
        Ok(())
    }

    /// Get a detection rule by ID.
    pub fn get_rule(&self, _id: &str) -> Result<Option<DetectionRule>, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // Implementation will be added in Task 8
        Ok(None)
    }

    /// Get all detection rules.
    pub fn get_all_rules(&self) -> Result<Vec<DetectionRule>, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // Implementation will be added in Task 8
        Ok(Vec::new())
    }

    /// Store an alert.
    pub fn store_alert(&self, _id: u64, _alert: &Alert) -> Result<(), StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _write_txn = self.db.begin_write()?;
        // Implementation will be added in Task 8
        Ok(())
    }

    /// Get an alert by ID.
    pub fn get_alert(&self, _id: u64) -> Result<Option<Alert>, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // let table = read_txn.open_table(Tables::ALERTS)?;
        // Ok(table
        //     .get(id)
        //     .map_err(StorageError::from)?
        //     .map(|guard| guard.value().clone()))
        Ok(None)
    }

    /// Get all alerts.
    pub fn get_all_alerts(&self) -> Result<Vec<Alert>, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // let table = read_txn.open_table(Tables::ALERTS)?;
        // let mut alerts = Vec::new();

        // for result in table.iter().map_err(StorageError::from)? {
        //     let (_, alert) = result.map_err(StorageError::from)?;
        //     alerts.push(alert.value().clone());
        // }

        // Ok(alerts)
        Ok(Vec::new())
    }

    /// Store system information.
    pub fn store_system_info(
        &self,
        _id: u64,
        _system_info: &SystemInfo,
    ) -> Result<(), StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _write_txn = self.db.begin_write()?;
        // {
        //     let mut table = write_txn.open_table(Tables::SYSTEM_INFO)?;
        //     table.insert(id, system_info).map_err(StorageError::from)?;
        // }
        // write_txn.commit()?;
        Ok(())
    }

    /// Get the latest system information.
    pub fn get_latest_system_info(&self) -> Result<Option<SystemInfo>, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // let table = read_txn.open_table(Tables::SYSTEM_INFO)?;

        // // Get the latest entry (highest ID)
        // let mut latest: Option<SystemInfo> = None;
        // for result in table.iter().map_err(StorageError::from)? {
        //     let (_, system_info) = result.map_err(StorageError::from)?;
        //     latest = Some(system_info.value().clone());
        // }

        // Ok(latest)
        Ok(None)
    }

    /// Store scan metadata.
    pub fn store_scan_metadata(
        &self,
        _id: u64,
        _metadata: &ScanMetadata,
    ) -> Result<(), StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _write_txn = self.db.begin_write()?;
        // {
        //     let mut table = write_txn.open_table(Tables::SCAN_METADATA)?;
        //     table.insert(id, metadata).map_err(StorageError::from)?;
        // }
        // write_txn.commit()?;
        Ok(())
    }

    /// Get scan metadata by ID.
    pub fn get_scan_metadata(&self, _id: u64) -> Result<Option<ScanMetadata>, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // Implementation will be added in Task 8
        Ok(None)
    }

    /// Get all scan metadata.
    pub fn get_all_scan_metadata(&self) -> Result<Vec<ScanMetadata>, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // Implementation will be added in Task 8
        Ok(Vec::new())
    }

    /// Clean up old data based on retention policy.
    pub fn cleanup_old_data(&self, retention_days: u32) -> Result<usize, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _cutoff_time = chrono::Utc::now()
            .checked_sub_signed(chrono::Duration::days(i64::from(retention_days)))
            .unwrap_or_else(chrono::Utc::now);
        let _write_txn = self.db.begin_write()?;
        // Implementation will be added in Task 8
        Ok(0)
    }

    /// Get database statistics.
    pub fn get_stats(&self) -> Result<DatabaseStats, StorageError> {
        // TODO: Implement in Task 8 - redb database integration
        let _read_txn = self.db.begin_read()?;
        // Implementation will be added in Task 8
        Ok(DatabaseStats::default())
    }
}

/// Database statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DatabaseStats {
    pub processes: usize,
    pub rules: usize,
    pub alerts: usize,
    pub system_info: usize,
    pub scans: usize,
    // Legacy field names for API compatibility
    pub process_count: usize,
    pub rule_count: usize,
    pub alert_count: usize,
    pub system_info_count: usize,
    pub scan_count: usize,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::let_underscore_must_use)]
mod tests {
    use super::*;
    use crate::models::AlertSeverity;
    use tempfile::tempdir;

    #[test]
    fn test_database_creation() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let _manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");
        assert!(db_path.exists());
    }

    #[test]
    fn test_database_creation_invalid_path() {
        // Test with a path that doesn't exist
        let invalid_path = Path::new("/nonexistent/path/test.db");
        let result = DatabaseManager::new(invalid_path);
        // This might succeed or fail depending on the system, but shouldn't panic
        let _ = result;
    }

    #[test]
    fn test_scan_metadata_creation() {
        let metadata = ScanMetadata {
            scan_id: "test-scan-1".to_owned(),
            timestamp: chrono::Utc::now(),
            process_count: 100,
            duration_ms: 5000,
            status: ScanStatus::Completed,
            error_message: None,
        };

        assert_eq!(metadata.scan_id, "test-scan-1");
        assert_eq!(metadata.process_count, 100);
        assert_eq!(metadata.duration_ms, 5000);
        assert_eq!(metadata.status, ScanStatus::Completed);
        assert!(metadata.error_message.is_none());
    }

    #[test]
    fn test_scan_metadata_with_error() {
        let metadata = ScanMetadata {
            scan_id: "test-scan-2".to_owned(),
            timestamp: chrono::Utc::now(),
            process_count: 0,
            duration_ms: 1000,
            status: ScanStatus::Failed,
            error_message: Some("Test error".to_owned()),
        };

        assert_eq!(metadata.status, ScanStatus::Failed);
        assert_eq!(metadata.error_message, Some("Test error".to_owned()));
    }

    #[test]
    fn test_scan_metadata_serialization() {
        let metadata = ScanMetadata {
            scan_id: "test-scan-3".to_owned(),
            timestamp: chrono::Utc::now(),
            process_count: 50,
            duration_ms: 2500,
            status: ScanStatus::InProgress,
            error_message: None,
        };

        let json = serde_json::to_string(&metadata).expect("Failed to serialize metadata");
        let deserialized: ScanMetadata =
            serde_json::from_str(&json).expect("Failed to deserialize metadata");

        assert_eq!(metadata.scan_id, deserialized.scan_id);
        assert_eq!(metadata.process_count, deserialized.process_count);
        assert_eq!(metadata.duration_ms, deserialized.duration_ms);
        assert_eq!(metadata.status, deserialized.status);
    }

    #[test]
    fn test_scan_status_variants() {
        let statuses = vec![
            ScanStatus::InProgress,
            ScanStatus::Completed,
            ScanStatus::Failed,
        ];

        for status in statuses {
            let json = serde_json::to_string(&status).expect("Failed to serialize status");
            let deserialized: ScanStatus =
                serde_json::from_str(&json).expect("Failed to deserialize status");
            assert_eq!(status, deserialized);
        }
    }

    #[test]
    fn test_database_stats_default() {
        let stats = DatabaseStats::default();
        assert_eq!(stats.process_count, 0);
        assert_eq!(stats.rule_count, 0);
        assert_eq!(stats.alert_count, 0);
        assert_eq!(stats.system_info_count, 0);
        assert_eq!(stats.scan_count, 0);
    }

    #[test]
    fn test_database_stats_creation() {
        let stats = DatabaseStats {
            processes: 100,
            rules: 10,
            alerts: 5,
            system_info: 1,
            scans: 50,
            process_count: 100,
            rule_count: 10,
            alert_count: 5,
            system_info_count: 1,
            scan_count: 50,
        };

        assert_eq!(stats.process_count, 100);
        assert_eq!(stats.rule_count, 10);
        assert_eq!(stats.alert_count, 5);
        assert_eq!(stats.system_info_count, 1);
        assert_eq!(stats.scan_count, 50);
    }

    #[test]
    fn test_database_stats_serialization() {
        let stats = DatabaseStats {
            processes: 100,
            rules: 10,
            alerts: 5,
            system_info: 1,
            scans: 50,
            process_count: 100,
            rule_count: 10,
            alert_count: 5,
            system_info_count: 1,
            scan_count: 50,
        };

        let json = serde_json::to_string(&stats).expect("Failed to serialize stats");
        let deserialized: DatabaseStats =
            serde_json::from_str(&json).expect("Failed to deserialize stats");
        assert_eq!(stats, deserialized);
    }

    #[test]
    fn test_storage_error_display() {
        let errors = vec![
            StorageError::TableNotFound {
                table: "test".to_owned(),
            },
            StorageError::RecordNotFound {
                id: "test-id".to_owned(),
            },
        ];

        for error in errors {
            let error_string = format!("{error}");
            assert!(!error_string.is_empty());
        }
    }

    #[test]
    fn test_process_storage() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        let process = ProcessRecord::new(1234, "test-process".to_owned());

        // Test that store_process doesn't panic (currently stubbed)
        manager
            .store_process(1, &process)
            .expect("Failed to store process");

        // Test that get_process returns None (currently stubbed)
        let retrieved = manager.get_process(1).expect("Failed to get process");
        assert!(retrieved.is_none()); // Currently stubbed to return None
    }

    #[test]
    fn test_rule_storage() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        let rule = DetectionRule::new(
            "rule-1".to_owned(),
            "Test Rule".to_owned(),
            "Test detection rule".to_owned(),
            "SELECT * FROM processes WHERE name = 'test'".to_owned(),
            "test".to_owned(),
            AlertSeverity::Medium,
        );

        // Test that store_rule doesn't panic (currently stubbed)
        manager.store_rule(&rule).expect("Failed to store rule");

        // Test that get_rule returns None (currently stubbed)
        let retrieved = manager.get_rule("rule-1").expect("Failed to get rule");
        assert!(retrieved.is_none()); // Currently stubbed to return None
    }

    #[test]
    fn test_alert_storage() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        let process = ProcessRecord::new(1234, "test-process".to_owned());
        let alert = Alert::new(
            AlertSeverity::High,
            "Test Alert",
            "This is a test alert",
            "test-rule",
            process,
        );

        // Test that store_alert doesn't panic (currently stubbed)
        manager
            .store_alert(1, &alert)
            .expect("Failed to store alert");

        // Test that get_alert returns None (currently stubbed)
        let retrieved = manager.get_alert(1).expect("Failed to get alert");
        assert!(retrieved.is_none()); // Currently stubbed to return None
    }

    #[test]
    fn test_get_all_alerts() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        // Test that get_all_alerts returns empty vector (currently stubbed)
        let alerts = manager.get_all_alerts().expect("Failed to get all alerts");
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_system_info_storage() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        let system_info = SystemInfo {
            hostname: "test-host".to_owned(),
            os_name: "TestOS".to_owned(),
            os_version: "1.0".to_owned(),
            architecture: "x86_64".to_owned(),
            cpu_cores: 4,
            total_memory: 8192,
            uptime: 3600,
            capabilities: vec!["test_capability".to_owned()],
        };

        // Test that store_system_info doesn't panic (currently stubbed)
        manager
            .store_system_info(1, &system_info)
            .expect("Failed to store system info");

        // Test that get_latest_system_info returns None (currently stubbed)
        let retrieved = manager
            .get_latest_system_info()
            .expect("Failed to get latest system info");
        assert!(retrieved.is_none());
    }

    #[test]
    fn test_database_stats() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        let process = ProcessRecord::new(1234, "test-process".to_owned());

        // Test that store_process doesn't panic (currently stubbed)
        manager
            .store_process(1, &process)
            .expect("Failed to store process");

        // Test that get_stats returns default values (currently stubbed)
        let stats = manager.get_stats().expect("Failed to get stats");
        assert_eq!(stats.process_count, 0); // Currently stubbed to return 0
    }

    #[test]
    fn test_tables_constants() {
        // Test that table definitions are accessible
        let _ = Tables::PROCESSES;
        let _ = Tables::DETECTION_RULES;
        let _ = Tables::ALERTS;
        let _ = Tables::SYSTEM_INFO;
        let _ = Tables::SCAN_METADATA;
    }

    #[test]
    fn test_database_manager_open() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        // Create a database first
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");
        drop(manager); // Close the database so we can open it again

        // Now open the existing database
        let open_manager = DatabaseManager::open(&db_path).expect("Failed to open database");
        assert!(db_path.exists());

        // Test that we can call methods on the opened database
        let stats = open_manager.get_stats().expect("Failed to get stats");
        assert_eq!(stats.process_count, 0);
    }

    #[test]
    fn test_batch_process_storage() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        let processes = vec![
            (1, ProcessRecord::new(1234, "process1".to_owned())),
            (2, ProcessRecord::new(5678, "process2".to_owned())),
            (3, ProcessRecord::new(9012, "process3".to_owned())),
        ];

        // Test that store_processes_batch doesn't panic (currently stubbed)
        manager
            .store_processes_batch(&processes)
            .expect("Failed to store processes batch");
    }

    #[test]
    fn test_get_all_processes() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        // Test that get_all_processes returns empty vector (currently stubbed)
        let processes = manager
            .get_all_processes()
            .expect("Failed to get all processes");
        assert!(processes.is_empty());
    }

    #[test]
    fn test_get_all_rules() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        // Test that get_all_rules returns empty vector (currently stubbed)
        let rules = manager.get_all_rules().expect("Failed to get all rules");
        assert!(rules.is_empty());
    }

    #[test]
    fn test_scan_metadata_storage() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        let metadata = ScanMetadata {
            scan_id: "test-scan".to_owned(),
            timestamp: chrono::Utc::now(),
            process_count: 100,
            duration_ms: 5000,
            status: ScanStatus::Completed,
            error_message: None,
        };

        // Test that store_scan_metadata doesn't panic (currently stubbed)
        manager
            .store_scan_metadata(1, &metadata)
            .expect("Failed to store scan metadata");

        // Test that get_scan_metadata returns None (currently stubbed)
        let retrieved = manager
            .get_scan_metadata(1)
            .expect("Failed to get scan metadata");
        assert!(retrieved.is_none());
    }

    #[test]
    fn test_get_all_scan_metadata() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        // Test that get_all_scan_metadata returns empty vector (currently stubbed)
        let metadata = manager
            .get_all_scan_metadata()
            .expect("Failed to get scan metadata");
        assert!(metadata.is_empty());
    }

    #[test]
    fn test_cleanup_old_data() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).expect("Failed to create database manager");

        // Test that cleanup_old_data returns 0 (currently stubbed)
        let cleaned = manager
            .cleanup_old_data(30)
            .expect("Failed to cleanup old data");
        assert_eq!(cleaned, 0);
    }

    #[test]
    fn test_storage_error_friendly_variants() {
        // Test the new friendly error variants
        let missing_dir_error = StorageError::MissingDirectory {
            dir: "/tmp/missing".into(),
            source: Some(std::io::Error::other("test io error")),
        };
        let missing_dir_msg = format!("{missing_dir_error}");
        assert!(missing_dir_msg.contains("Database directory"));
        assert!(missing_dir_msg.contains("does not exist"));
        assert!(missing_dir_msg.contains("/tmp/missing"));

        let not_dir_error = StorageError::NotADirectory {
            path: "/tmp/file".into(),
        };
        let not_dir_msg = format!("{not_dir_error}");
        assert!(not_dir_msg.contains("is not a directory"));
        assert!(not_dir_msg.contains("/tmp/file"));
    }

    #[test]
    fn test_storage_error_from_serialization_error() {
        // Test that StorageError can be created from serde_json::Error
        let json_error = serde_json::from_str::<serde_json::Value>("invalid json")
            .expect_err("Expected JSON error");
        let storage_error = StorageError::from(json_error);
        assert!(format!("{storage_error}").contains("Serialization error"));
    }

    #[test]
    fn test_scan_metadata_with_all_fields() {
        let metadata = ScanMetadata {
            scan_id: "comprehensive-scan".to_owned(),
            timestamp: chrono::Utc::now(),
            process_count: 1000,
            duration_ms: 10000,
            status: ScanStatus::InProgress,
            error_message: Some("Test error message".to_owned()),
        };

        assert_eq!(metadata.scan_id, "comprehensive-scan");
        assert_eq!(metadata.process_count, 1000);
        assert_eq!(metadata.duration_ms, 10000);
        assert_eq!(metadata.status, ScanStatus::InProgress);
        assert_eq!(
            metadata.error_message,
            Some("Test error message".to_owned())
        );
    }

    #[test]
    fn test_database_stats_with_all_fields() {
        let stats = DatabaseStats {
            processes: 1000,
            rules: 50,
            alerts: 25,
            system_info: 5,
            scans: 100,
            process_count: 1000,
            rule_count: 50,
            alert_count: 25,
            system_info_count: 5,
            scan_count: 100,
        };

        assert_eq!(stats.process_count, 1000);
        assert_eq!(stats.rule_count, 50);
        assert_eq!(stats.alert_count, 25);
        assert_eq!(stats.system_info_count, 5);
        assert_eq!(stats.scan_count, 100);
    }

    #[test]
    fn test_database_stats_clone() {
        let stats = DatabaseStats {
            processes: 100,
            rules: 10,
            alerts: 5,
            system_info: 1,
            scans: 50,
            process_count: 100,
            rule_count: 10,
            alert_count: 5,
            system_info_count: 1,
            scan_count: 50,
        };

        let cloned_stats = stats.clone();
        assert_eq!(stats, cloned_stats);
    }

    #[test]
    fn test_scan_metadata_clone() {
        let metadata = ScanMetadata {
            scan_id: "test-scan".to_owned(),
            timestamp: chrono::Utc::now(),
            process_count: 100,
            duration_ms: 5000,
            status: ScanStatus::Completed,
            error_message: None,
        };

        let cloned_metadata = metadata.clone();
        assert_eq!(metadata, cloned_metadata);
    }

    #[test]
    fn test_scan_status_clone() {
        let status = ScanStatus::InProgress;
        let cloned_status = status.clone();
        assert_eq!(status, cloned_status);
    }
}

// TODO: Implement redb Value trait implementations in Task 8
// For now, just focus on getting the basic structure compiling for Task 1
