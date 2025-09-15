//! Database abstractions and storage management.
//!
//! This module provides database operations using redb for optimal performance and security.
//! It includes table definitions, transaction handling, and data serialization.

use crate::models::{Alert, DetectionRule, ProcessRecord, SystemInfo};
use redb::{Database, ReadableDatabase, TableDefinition};
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

/// Database operation errors.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("Database error: {0}")]
    DatabaseError(#[from] redb::Error),

    #[error("Database error: {0}")]
    DatabaseError2(#[from] redb::DatabaseError),

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

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Table not found: {table}")]
    TableNotFound { table: String },

    #[error("Record not found: {id}")]
    RecordNotFound { id: String },
}

/// Table definitions for the database schema.
pub struct Tables;

impl Tables {
    /// Process records table
    /// TODO: Use ProcessRecord type in Task 8 when redb Value trait is implemented
    pub const PROCESSES: TableDefinition<'static, u64, Vec<u8>> = TableDefinition::new("processes");

    /// Detection rules table
    /// TODO: Use DetectionRule type in Task 8 when redb Value trait is implemented
    pub const DETECTION_RULES: TableDefinition<'static, &str, Vec<u8>> =
        TableDefinition::new("detection_rules");

    /// Alerts table
    /// TODO: Use Alert type in Task 8 when redb Value trait is implemented
    pub const ALERTS: TableDefinition<'static, u64, Vec<u8>> = TableDefinition::new("alerts");

    /// System info table
    /// TODO: Use SystemInfo type in Task 8 when redb Value trait is implemented
    pub const SYSTEM_INFO: TableDefinition<'static, u64, Vec<u8>> =
        TableDefinition::new("system_info");

    /// Scan metadata table
    /// TODO: Use ScanMetadata type in Task 8 when redb Value trait is implemented
    pub const SCAN_METADATA: TableDefinition<'static, u64, Vec<u8>> =
        TableDefinition::new("scan_metadata");
}

/// Scan metadata for tracking collection operations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ScanStatus {
    InProgress,
    Completed,
    Failed,
}

/// Database manager for SentinelD storage operations.
pub struct DatabaseManager {
    db: Database,
}

impl DatabaseManager {
    /// Create a new database manager.
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let db = Database::create(path).map_err(StorageError::from)?;
        let manager = Self { db };
        manager.initialize_schema()?;
        Ok(manager)
    }

    /// Open an existing database.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let db = Database::open(path).map_err(StorageError::from)?;
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
        let _cutoff_time = chrono::Utc::now() - chrono::Duration::days(retention_days as i64);
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
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DatabaseStats {
    pub process_count: usize,
    pub rule_count: usize,
    pub alert_count: usize,
    pub system_info_count: usize,
    pub scan_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AlertSeverity;
    use tempfile::tempdir;

    #[test]
    fn test_database_creation() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let _manager = DatabaseManager::new(&db_path).unwrap();
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
            scan_id: "test-scan-1".to_string(),
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
            scan_id: "test-scan-2".to_string(),
            timestamp: chrono::Utc::now(),
            process_count: 0,
            duration_ms: 1000,
            status: ScanStatus::Failed,
            error_message: Some("Test error".to_string()),
        };

        assert_eq!(metadata.status, ScanStatus::Failed);
        assert_eq!(metadata.error_message, Some("Test error".to_string()));
    }

    #[test]
    fn test_scan_metadata_serialization() {
        let metadata = ScanMetadata {
            scan_id: "test-scan-3".to_string(),
            timestamp: chrono::Utc::now(),
            process_count: 50,
            duration_ms: 2500,
            status: ScanStatus::InProgress,
            error_message: None,
        };

        let json = serde_json::to_string(&metadata).unwrap();
        let deserialized: ScanMetadata = serde_json::from_str(&json).unwrap();

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
            let json = serde_json::to_string(&status).unwrap();
            let deserialized: ScanStatus = serde_json::from_str(&json).unwrap();
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
            process_count: 100,
            rule_count: 10,
            alert_count: 5,
            system_info_count: 1,
            scan_count: 50,
        };

        let json = serde_json::to_string(&stats).unwrap();
        let deserialized: DatabaseStats = serde_json::from_str(&json).unwrap();
        assert_eq!(stats, deserialized);
    }

    #[test]
    fn test_storage_error_display() {
        let errors = vec![
            StorageError::TableNotFound {
                table: "test".to_string(),
            },
            StorageError::RecordNotFound {
                id: "test-id".to_string(),
            },
        ];

        for error in errors {
            let error_string = format!("{}", error);
            assert!(!error_string.is_empty());
        }
    }

    #[test]
    fn test_process_storage() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).unwrap();

        let process = ProcessRecord::new(1234, "test-process".to_string());

        // Test that store_process doesn't panic (currently stubbed)
        manager.store_process(1, &process).unwrap();

        // Test that get_process returns None (currently stubbed)
        let retrieved = manager.get_process(1).unwrap();
        assert!(retrieved.is_none()); // Currently stubbed to return None
    }

    #[test]
    fn test_rule_storage() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).unwrap();

        let rule = DetectionRule::new(
            "rule-1".to_string(),
            "Test Rule".to_string(),
            "Test detection rule".to_string(),
            "SELECT * FROM processes WHERE name = 'test'".to_string(),
            "test".to_string(),
            AlertSeverity::Medium,
        );

        // Test that store_rule doesn't panic (currently stubbed)
        manager.store_rule(&rule).unwrap();

        // Test that get_rule returns None (currently stubbed)
        let retrieved = manager.get_rule("rule-1").unwrap();
        assert!(retrieved.is_none()); // Currently stubbed to return None
    }

    #[test]
    fn test_alert_storage() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).unwrap();

        let process = ProcessRecord::new(1234, "test-process".to_string());
        let alert = Alert::new(
            AlertSeverity::High,
            "Test Alert",
            "This is a test alert",
            "test-rule",
            process,
        );

        // Test that store_alert doesn't panic (currently stubbed)
        manager.store_alert(1, &alert).unwrap();

        // Test that get_alert returns None (currently stubbed)
        let retrieved = manager.get_alert(1).unwrap();
        assert!(retrieved.is_none()); // Currently stubbed to return None
    }

    #[test]
    fn test_get_all_alerts() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).unwrap();

        // Test that get_all_alerts returns empty vector (currently stubbed)
        let alerts = manager.get_all_alerts().unwrap();
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_system_info_storage() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).unwrap();

        let system_info = SystemInfo {
            hostname: "test-host".to_string(),
            os_name: "TestOS".to_string(),
            os_version: "1.0".to_string(),
            architecture: "x86_64".to_string(),
            cpu_cores: 4,
            total_memory: 8192,
            uptime: 3600,
            capabilities: vec!["test_capability".to_string()],
        };

        // Test that store_system_info doesn't panic (currently stubbed)
        manager.store_system_info(1, &system_info).unwrap();

        // Test that get_latest_system_info returns None (currently stubbed)
        let retrieved = manager.get_latest_system_info().unwrap();
        assert!(retrieved.is_none());
    }

    #[test]
    fn test_database_stats() {
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let manager = DatabaseManager::new(&db_path).unwrap();

        let process = ProcessRecord::new(1234, "test-process".to_string());

        // Test that store_process doesn't panic (currently stubbed)
        manager.store_process(1, &process).unwrap();

        // Test that get_stats returns default values (currently stubbed)
        let stats = manager.get_stats().unwrap();
        assert_eq!(stats.process_count, 0); // Currently stubbed to return 0
    }

    #[test]
    fn test_tables_constants() {
        // Test that table definitions are accessible
        let _processes = Tables::PROCESSES;
        let _detection_rules = Tables::DETECTION_RULES;
        let _alerts = Tables::ALERTS;
        let _system_info = Tables::SYSTEM_INFO;
        let _scan_metadata = Tables::SCAN_METADATA;
    }
}

// TODO: Implement redb Value trait implementations in Task 8
// For now, just focus on getting the basic structure compiling for Task 1
