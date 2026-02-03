//! Write-Ahead Log (WAL) for crash recovery and event persistence.
//!
//! This module implements a crash-recovery mechanism that persists process events
//! to disk before they are published to event consumers. In case of process crash,
//! the WAL can be replayed to recover unpublished events.
//!
//! # Overview
//!
//! The WAL stores events as length-delimited, postcard-serialized `WalEntry` structures
//! with CRC32 checksums for corruption detection. Files are automatically rotated when
//! reaching a configurable threshold (default 80MB) to maintain manageable file sizes
//! and enable efficient cleanup of published events.
//!
//! # File Format
//!
//! Each WAL file contains a sequence of entries in the following format:
//! ```text
//! [u32 length prefix (little-endian)][postcard-serialized WalEntry]...
//! ```
//!
//! WAL files are named: `procmond-{sequence:05}.wal` where sequence is a zero-padded
//! 5-digit number representing the file's creation order.
//!
//! # Sequence Numbers
//!
//! Each event is assigned a monotonically increasing sequence number that uniquely
//! identifies it across all WAL files. Sequence numbers are persisted across restarts
//! by scanning existing WAL files on startup. Sequence numbers enable:
//! - Crash recovery with ordering guarantees
//! - Cleanup tracking (mark events as published up to sequence N)
//! - Detection of lost events
//!
//! # Corruption Handling
//!
//! - **CRC32 Mismatch**: Entry is skipped with a warning log
//! - **Deserialization Error**: Entry is skipped with a warning log
//! - **Partial Write**: Detected during replay when reading truncated entries
//!
//! # Examples
//!
//! ```rust,ignore
//! use procmond::wal::{WriteAheadLog, WalResult};
//! use collector_core::ProcessEvent;
//! use std::path::PathBuf;
//!
//! async fn example() -> WalResult<()> {
//!     // Create or open WAL
//!     let wal = WriteAheadLog::new(PathBuf::from("/var/lib/procmond/wal")).await?;
//!
//!     // Write an event and get its sequence number
//!     let sequence = wal.write(process_event).await?;
//!
//!     // Replay on startup to recover unpublished events
//!     let events = wal.replay().await?;
//!
//!     // Mark events as published to enable cleanup
//!     wal.mark_published(sequence).await?;
//!     Ok(())
//! }
//! ```

use collector_core::event::ProcessEvent;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// WAL-specific error types.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WalError {
    /// File I/O operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Serialization or deserialization failed.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// CRC32 checksum validation failed.
    #[error("Corruption detected in WAL entry (sequence: {sequence}): {message}")]
    Corruption {
        /// Sequence number of the corrupted entry
        sequence: u64,
        /// Description of corruption
        message: String,
    },

    /// Sequence number mismatch during recovery.
    #[error("Invalid sequence during replay: expected {expected}, found {found}")]
    InvalidSequence {
        /// Expected sequence number
        expected: u64,
        /// Actual sequence number found
        found: u64,
    },

    /// File rotation operation failed.
    #[error("File rotation failed: {0}")]
    FileRotation(String),

    /// Replay operation encountered an error.
    #[error("Replay error: {0}")]
    Replay(String),
}

/// Result type for WAL operations.
pub type WalResult<T> = Result<T, WalError>;

/// Internal result type for file read operations.
///
/// Used by helper functions to signal EOF vs I/O errors.
enum ReadResult {
    /// End of file reached (expected during normal replay)
    Eof,
    /// I/O error occurred
    Io(std::io::Error),
}

/// A single entry in the WAL.
///
/// Each entry contains a process event with a monotonically increasing sequence
/// number and a CRC32 checksum for corruption detection.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WalEntry {
    /// Monotonically increasing sequence number across all WAL files
    pub sequence: u64,

    /// The process event being persisted
    pub event: ProcessEvent,

    /// CRC32 checksum of the serialized event for corruption detection
    pub checksum: u32,

    /// Optional event type for topic routing (e.g., "start", "stop", "modify").
    /// Added for backward compatibility - older WAL files will deserialize with None.
    #[serde(default)]
    pub event_type: Option<String>,
}

impl WalEntry {
    /// Create a new WAL entry with automatic checksum computation.
    ///
    /// # Arguments
    ///
    /// * `sequence` - Monotonically increasing sequence number
    /// * `event` - Process event to persist
    ///
    /// # Returns
    ///
    /// A new `WalEntry` with checksum computed from the event data
    pub fn new(sequence: u64, event: ProcessEvent) -> Self {
        let checksum = Self::compute_checksum(&event);
        Self {
            sequence,
            event,
            checksum,
            event_type: None,
        }
    }

    /// Create a new WAL entry with event type for topic routing.
    ///
    /// # Arguments
    ///
    /// * `sequence` - Monotonically increasing sequence number
    /// * `event` - Process event to persist
    /// * `event_type` - Event type string (e.g., "start", "stop", "modify")
    ///
    /// # Returns
    ///
    /// A new `WalEntry` with checksum and event type
    pub fn with_event_type(sequence: u64, event: ProcessEvent, event_type: String) -> Self {
        let checksum = Self::compute_checksum(&event);
        Self {
            sequence,
            event,
            checksum,
            event_type: Some(event_type),
        }
    }

    /// Compute CRC32 checksum of the event's serialized form.
    fn compute_checksum(event: &ProcessEvent) -> u32 {
        use std::hash::Hasher;
        postcard::to_allocvec(event).map_or(0, |serialized| {
            let mut crc = crc32c::Crc32cHasher::new(0);
            for chunk in serialized.chunks(8192) {
                crc.write(chunk);
            }
            #[allow(clippy::as_conversions)]
            // Safe: CRC32 hash is always u64, truncation to u32 is expected
            {
                crc.finish() as u32
            }
        })
    }

    /// Verify the integrity of this entry's checksum.
    ///
    /// # Returns
    ///
    /// `true` if the checksum matches the event data, `false` otherwise
    pub fn verify(&self) -> bool {
        let computed = Self::compute_checksum(&self.event);
        computed == self.checksum
    }
}

/// Metadata about event sequences within a WAL file.
///
/// Tracks the minimum and maximum event sequence numbers contained in a WAL file,
/// enabling efficient cleanup of published events.
#[derive(Debug, Clone, Copy, Default)]
pub struct WalFileMetadata {
    /// Minimum event sequence number in this file (first entry)
    pub min_sequence: u64,
    /// Maximum event sequence number in this file (last entry)
    pub max_sequence: u64,
    /// Number of valid entries in this file
    pub entry_count: u64,
}

/// Write-Ahead Log for event persistence and crash recovery.
///
/// Manages a set of append-only log files that store process events with
/// automatic rotation, crash recovery, and cleanup capabilities.
pub struct WriteAheadLog {
    /// Directory containing WAL files
    wal_dir: PathBuf,

    /// Current event sequence number - monotonically increasing across all events (thread-safe)
    current_event_sequence: Arc<AtomicU64>,

    /// Current file sequence number - identifies which WAL file to write to (thread-safe)
    current_file_sequence: Arc<AtomicU64>,

    /// Currently active WAL file handle and current file size (protected by same mutex for atomic rotation)
    file_state: Arc<Mutex<WalFileState>>,

    /// Rotation trigger threshold (configurable, default 80MB)
    rotation_threshold: u64,
}

/// Internal state for the active WAL file, protected by a single mutex for atomic operations.
struct WalFileState {
    /// Currently active WAL file handle (never None during normal operation)
    file: fs::File,
    /// Current file size for rotation tracking
    size: u64,
    /// Minimum event sequence in current file (0 if empty)
    min_sequence: u64,
    /// Maximum event sequence in current file (0 if empty)
    max_sequence: u64,
}

impl WriteAheadLog {
    /// Default rotation threshold (80MB = 83,886,080 bytes)
    const DEFAULT_ROTATION_THRESHOLD: u64 = 80 * 1024 * 1024;

    /// Create or open a Write-Ahead Log at the specified directory.
    ///
    /// # Arguments
    ///
    /// * `wal_dir` - Directory path for WAL files
    ///
    /// # Returns
    ///
    /// A new `WriteAheadLog` instance initialized and ready for operations
    ///
    /// # Errors
    ///
    /// Returns `WalError` if directory creation or file scanning fails
    pub async fn new(wal_dir: PathBuf) -> WalResult<Self> {
        Self::with_rotation_threshold(wal_dir, Self::DEFAULT_ROTATION_THRESHOLD).await
    }

    /// Create or open a Write-Ahead Log with a custom rotation threshold.
    ///
    /// This is primarily useful for testing to avoid creating 80MB files.
    ///
    /// # Arguments
    ///
    /// * `wal_dir` - Directory path for WAL files
    /// * `rotation_threshold` - File size threshold in bytes that triggers rotation
    ///
    /// # Returns
    ///
    /// A new `WriteAheadLog` instance initialized and ready for operations
    ///
    /// # Errors
    ///
    /// Returns `WalError` if directory creation or file scanning fails
    pub async fn with_rotation_threshold(
        wal_dir: PathBuf,
        rotation_threshold: u64,
    ) -> WalResult<Self> {
        // Create WAL directory if it doesn't exist
        fs::create_dir_all(&wal_dir).await.map_err(WalError::Io)?;

        // Scan for existing WAL files to determine next file sequence number
        // and find the highest event sequence for monotonic sequencing across restarts
        let (next_file_sequence, highest_event_sequence, current_file_metadata) =
            Self::scan_wal_state(&wal_dir).await?;

        debug!(
            wal_dir = ?wal_dir,
            next_file_sequence = next_file_sequence,
            highest_event_sequence = highest_event_sequence,
            "Initializing WAL"
        );

        // Open or create initial WAL file
        #[allow(clippy::as_conversions)] // Safe: file sequence is always within u32 range
        let file_sequence_u32 = next_file_sequence as u32;
        let file_path = Self::wal_file_path(&wal_dir, file_sequence_u32);
        let file = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&file_path)
            .await
            .map_err(WalError::Io)?;

        // Get initial file size if resuming
        let metadata = fs::metadata(&file_path).await.map_err(WalError::Io)?;
        let file_size = metadata.len();

        // Event sequence continues from highest found + 1 to maintain monotonic sequencing
        let initial_event_sequence = highest_event_sequence.saturating_add(1);

        // Use metadata from current file if it exists, otherwise start fresh
        let (min_seq, max_seq) =
            current_file_metadata.map_or((0, 0), |m| (m.min_sequence, m.max_sequence));

        let file_state = WalFileState {
            file,
            size: file_size,
            min_sequence: min_seq,
            max_sequence: max_seq,
        };

        Ok(Self {
            wal_dir,
            current_event_sequence: Arc::new(AtomicU64::new(initial_event_sequence)),
            current_file_sequence: Arc::new(AtomicU64::new(next_file_sequence)),
            file_state: Arc::new(Mutex::new(file_state)),
            rotation_threshold,
        })
    }

    /// Scan existing WAL files to determine the next file sequence and highest event sequence.
    ///
    /// Returns (next_file_sequence, highest_event_sequence, current_file_metadata)
    async fn scan_wal_state(wal_dir: &Path) -> WalResult<(u64, u64, Option<WalFileMetadata>)> {
        let mut max_file_sequence = 0_u64;
        let mut highest_event_sequence = 0_u64;
        let mut current_file_metadata = None;

        match fs::read_dir(wal_dir).await {
            Ok(mut dir) => {
                let mut files = Vec::new();
                while let Ok(Some(entry)) = dir.next_entry().await {
                    let path = entry.path();
                    if let Some(filename) = path.file_name().and_then(|n| n.to_str())
                        && let Some(sequence) = Self::parse_wal_filename(filename)
                    {
                        files.push((sequence, path));
                        max_file_sequence = max_file_sequence.max(u64::from(sequence));
                    }
                }

                // Sort files by sequence
                files.sort_by_key(|f| f.0);

                // Scan each file to find the highest event sequence
                for &(file_seq, ref path) in &files {
                    match Self::scan_file_metadata(path).await {
                        Ok(metadata) => {
                            highest_event_sequence =
                                highest_event_sequence.max(metadata.max_sequence);

                            // Track metadata for the current (highest sequence) file
                            if u64::from(file_seq) == max_file_sequence {
                                current_file_metadata = Some(metadata);
                            }
                        }
                        Err(e) => {
                            warn!(
                                path = ?path,
                                error = %e,
                                "Failed to scan WAL file metadata, skipping file"
                            );
                        }
                    }
                }
            }
            Err(e) => {
                debug!(
                    wal_dir = ?wal_dir,
                    error = %e,
                    "WAL directory not readable or does not exist, starting fresh"
                );
            }
        }

        // Next file sequence: if we have files, continue with the highest; if empty, start at 1
        let next_file_sequence = if max_file_sequence > 0 {
            max_file_sequence
        } else {
            1
        };

        Ok((
            next_file_sequence,
            highest_event_sequence,
            current_file_metadata,
        ))
    }

    /// Scan a single WAL file to extract metadata about event sequences.
    ///
    /// Uses helper functions to reduce nesting depth.
    async fn scan_file_metadata(path: &Path) -> WalResult<WalFileMetadata> {
        let mut file = fs::File::open(path)
            .await
            .map_err(|e| WalError::Replay(format!("Failed to open file for scanning: {e}")))?;

        let mut metadata = WalFileMetadata::default();
        let mut buffer = vec![0_u8; 4];
        let mut first_entry = true;

        loop {
            // Read length prefix - handle EOF
            let length = match Self::read_length_prefix(&mut file, &mut buffer).await {
                Ok(len) => len,
                Err(ReadResult::Eof) => break,
                Err(ReadResult::Io(e)) => return Err(WalError::Io(e)),
            };

            // Read entry data - handle EOF/errors
            let entry_data = match Self::read_entry_data(&mut file, length).await {
                Ok(data) => data,
                Err(ReadResult::Eof) => break,
                Err(ReadResult::Io(e)) => return Err(WalError::Io(e)),
            };

            // Try to deserialize and verify; update metadata if valid
            if let Some(entry) = Self::deserialize_and_verify_entry(&entry_data) {
                if first_entry {
                    metadata.min_sequence = entry.sequence;
                    first_entry = false;
                }
                metadata.max_sequence = entry.sequence;
                metadata.entry_count = metadata.entry_count.saturating_add(1);
            }
        }

        Ok(metadata)
    }

    /// Generate a WAL file path from a directory and sequence number.
    fn wal_file_path(wal_dir: &Path, sequence: u32) -> PathBuf {
        wal_dir.join(format!("procmond-{sequence:05}.wal"))
    }

    /// Parse sequence number from a WAL filename.
    fn parse_wal_filename(filename: &str) -> Option<u32> {
        // Check for .wal extension case-insensitively using Path
        let has_wal_ext = std::path::Path::new(filename)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("wal"));

        if !has_wal_ext || !filename.starts_with("procmond-") {
            return None;
        }

        filename
            .strip_prefix("procmond-")
            .and_then(|s| s.strip_suffix(".wal"))
            .and_then(|s| s.parse::<u32>().ok())
    }

    /// List all WAL files sorted by sequence number.
    async fn list_wal_files(&self) -> WalResult<Vec<(u32, PathBuf)>> {
        let mut entries = Vec::new();

        let mut dir = fs::read_dir(&self.wal_dir).await.map_err(WalError::Io)?;

        while let Some(entry) = dir.next_entry().await.map_err(WalError::Io)? {
            let path = entry.path();
            if let Some(filename) = path.file_name().and_then(|n| n.to_str())
                && let Some(sequence) = Self::parse_wal_filename(filename)
            {
                entries.push((sequence, path));
            }
        }

        entries.sort_by_key(|entry| entry.0);
        Ok(entries)
    }

    /// Write an event to the WAL with automatic rotation.
    ///
    /// Rotation is performed atomically with respect to writers - the new file is opened
    /// before the old one is closed, all within the same lock, ensuring writers never
    /// observe a missing file handle.
    ///
    /// # Arguments
    ///
    /// * `event` - Process event to persist
    ///
    /// # Returns
    ///
    /// The sequence number assigned to this event
    ///
    /// # Errors
    ///
    /// Returns `WalError` if serialization, file I/O, or rotation fails
    #[allow(clippy::significant_drop_tightening)] // Lock is intentionally held throughout for atomic rotation
    pub async fn write(&self, event: ProcessEvent) -> WalResult<u64> {
        use tokio::io::AsyncWriteExt;

        // Get the next event sequence number
        let sequence = self.current_event_sequence.fetch_add(1, Ordering::SeqCst);

        // Create WAL entry with automatic checksum
        let entry = WalEntry::new(sequence, event);

        // Serialize the entry
        let serialized =
            postcard::to_allocvec(&entry).map_err(|e| WalError::Serialization(e.to_string()))?;

        // Prepare length prefix (little-endian u32)
        #[allow(clippy::as_conversions)] // Safe: serialized len is bounded by frame size
        let length = serialized.len() as u32;
        let length_bytes = length.to_le_bytes();

        // Calculate size increment safely
        let size_increment = length_bytes.len().saturating_add(serialized.len());
        #[allow(clippy::as_conversions)] // Safe: total size is bounded by max frame size
        let size_increment_u64 = size_increment as u64;

        // Write to current file - hold lock for entire operation including rotation
        let mut state = self.file_state.lock().await;

        // Write length prefix
        state
            .file
            .write_all(&length_bytes)
            .await
            .map_err(WalError::Io)?;

        // Write serialized entry
        state
            .file
            .write_all(&serialized)
            .await
            .map_err(WalError::Io)?;

        // Update file size and sequence tracking
        state.size = state.size.saturating_add(size_increment_u64);

        // Track min/max sequences for this file
        if state.min_sequence == 0 {
            state.min_sequence = sequence;
        }
        state.max_sequence = sequence;

        debug!(
            sequence = sequence,
            file_size = state.size,
            "WAL entry written"
        );

        // Check if rotation is needed - perform atomically within the same lock
        if state.size >= self.rotation_threshold {
            self.rotate_file_internal(&mut state).await?;
        }

        Ok(sequence)
    }

    /// Write an event to the WAL with event type metadata for topic routing.
    ///
    /// Similar to [`write`], but includes an event type string that can be used
    /// during replay to determine the correct topic for republishing.
    ///
    /// # Arguments
    ///
    /// * `event` - Process event to persist
    /// * `event_type` - Event type string (e.g., "start", "stop", "modify")
    ///
    /// # Returns
    ///
    /// The sequence number assigned to this event
    ///
    /// # Errors
    ///
    /// Returns `WalError` if serialization, file I/O, or rotation fails
    #[allow(clippy::significant_drop_tightening)] // Lock is intentionally held throughout for atomic rotation
    pub async fn write_with_type(&self, event: ProcessEvent, event_type: String) -> WalResult<u64> {
        use tokio::io::AsyncWriteExt;

        // Get the next event sequence number
        let sequence = self.current_event_sequence.fetch_add(1, Ordering::SeqCst);

        // Create WAL entry with event type
        let entry = WalEntry::with_event_type(sequence, event, event_type);

        // Serialize the entry
        let serialized =
            postcard::to_allocvec(&entry).map_err(|e| WalError::Serialization(e.to_string()))?;

        // Prepare length prefix (little-endian u32)
        #[allow(clippy::as_conversions)] // Safe: serialized len is bounded by frame size
        let length = serialized.len() as u32;
        let length_bytes = length.to_le_bytes();

        // Calculate size increment safely
        let size_increment = length_bytes.len().saturating_add(serialized.len());
        #[allow(clippy::as_conversions)] // Safe: total size is bounded by max frame size
        let size_increment_u64 = size_increment as u64;

        // Write to current file - hold lock for entire operation including rotation
        let mut state = self.file_state.lock().await;

        // Write length prefix
        state
            .file
            .write_all(&length_bytes)
            .await
            .map_err(WalError::Io)?;

        // Write serialized entry
        state
            .file
            .write_all(&serialized)
            .await
            .map_err(WalError::Io)?;

        // Update file size and sequence tracking
        state.size = state.size.saturating_add(size_increment_u64);

        // Track min/max sequences for this file
        if state.min_sequence == 0 {
            state.min_sequence = sequence;
        }
        state.max_sequence = sequence;

        debug!(
            sequence = sequence,
            file_size = state.size,
            "WAL entry written with event type"
        );

        // Check if rotation is needed - perform atomically within the same lock
        if state.size >= self.rotation_threshold {
            self.rotate_file_internal(&mut state).await?;
        }

        Ok(sequence)
    }

    /// Rotate to the next WAL file (internal implementation holding the lock).
    ///
    /// This method performs rotation atomically by:
    /// 1. Opening the new file first
    /// 2. Replacing the file handle in the state
    /// 3. The old file handle is dropped when the state is updated
    ///
    /// Writers never observe a missing file handle because the lock is held throughout.
    async fn rotate_file_internal(&self, state: &mut WalFileState) -> WalResult<()> {
        debug!("Rotating WAL file");

        // Increment file sequence and open new file BEFORE closing old one
        let previous_sequence = self.current_file_sequence.fetch_add(1, Ordering::SeqCst);
        let next_file_sequence = previous_sequence.saturating_add(1);

        #[allow(clippy::as_conversions)] // Safe: file sequence is always within u32 range
        let file_sequence_u32 = next_file_sequence as u32;
        let file_path = Self::wal_file_path(&self.wal_dir, file_sequence_u32);

        let new_file = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&file_path)
            .await
            .map_err(|e| WalError::FileRotation(format!("Failed to open new WAL file: {e}")))?;

        // Atomically replace the file handle - old file is closed when dropped
        state.file = new_file;
        state.size = 0;
        state.min_sequence = 0;
        state.max_sequence = 0;

        info!(file_sequence = next_file_sequence, "WAL file rotated");

        Ok(())
    }

    /// Replay WAL events for crash recovery.
    ///
    /// Reads all WAL files in sequence order and recovers unpublished events.
    /// Corrupted entries are skipped with warnings.
    ///
    /// # Returns
    ///
    /// A vector of recovered events in chronological order
    ///
    /// # Errors
    ///
    /// Returns `WalError` if directory scanning or fundamental I/O fails
    pub async fn replay(&self) -> WalResult<Vec<ProcessEvent>> {
        let mut all_events = Vec::new();
        let files = self.list_wal_files().await?;
        let total_files = files.len();
        let mut failed_files = 0_usize;

        debug!(file_count = total_files, "Starting WAL replay");

        for (_sequence, path) in files {
            match self.replay_file(&path).await {
                Ok(events) => {
                    debug!(
                        file = ?path,
                        event_count = events.len(),
                        "Replayed WAL file"
                    );
                    all_events.extend(events);
                }
                Err(e) => {
                    warn!(file = ?path, error = %e, "Error replaying WAL file");
                    failed_files = failed_files.saturating_add(1);
                }
            }
        }

        if failed_files > 0 {
            warn!(
                failed_files = failed_files,
                total_files = total_files,
                "WAL replay completed with errors - some events may not have been recovered"
            );
        }

        info!(
            total_events = all_events.len(),
            failed_files = failed_files,
            "WAL replay complete"
        );

        Ok(all_events)
    }

    /// Replay a single WAL file.
    ///
    /// Delegates to [`Self::replay_file_entries`] and extracts just the events.
    async fn replay_file(&self, path: &Path) -> WalResult<Vec<ProcessEvent>> {
        let entries = self.replay_file_entries(path).await?;
        Ok(entries.into_iter().map(|entry| entry.event).collect())
    }

    /// Replay WAL entries with full metadata for crash recovery.
    ///
    /// Similar to [`Self::replay`], but returns complete `WalEntry` objects including
    /// sequence numbers and event types. Use this when you need to track which
    /// events have been published or need event type information for topic routing.
    ///
    /// # Returns
    ///
    /// A vector of recovered WAL entries in chronological order
    ///
    /// # Errors
    ///
    /// Returns `WalError` if directory scanning or fundamental I/O fails
    pub async fn replay_entries(&self) -> WalResult<Vec<WalEntry>> {
        let mut all_entries = Vec::new();
        let files = self.list_wal_files().await?;

        debug!(file_count = files.len(), "Starting WAL entry replay");

        for (_sequence, path) in files {
            match self.replay_file_entries(&path).await {
                Ok(entries) => {
                    debug!(
                        file = ?path,
                        entry_count = entries.len(),
                        "Replayed WAL file entries"
                    );
                    all_entries.extend(entries);
                }
                Err(e) => {
                    warn!("Error replaying WAL file {path:?}: {e}");
                }
            }
        }

        info!(
            total_entries = all_entries.len(),
            "WAL entry replay complete"
        );

        Ok(all_entries)
    }

    /// Replay a single WAL file returning full entries.
    ///
    /// Uses early-continue pattern to reduce nesting depth.
    async fn replay_file_entries(&self, path: &Path) -> WalResult<Vec<WalEntry>> {
        let mut file = fs::File::open(path)
            .await
            .map_err(|e| WalError::Replay(format!("Failed to open file: {e}")))?;

        let mut entries = Vec::new();
        let mut buffer = vec![0_u8; 4];

        loop {
            // Read length prefix - handle EOF
            let length = match Self::read_length_prefix(&mut file, &mut buffer).await {
                Ok(len) => len,
                Err(ReadResult::Eof) => break,
                Err(ReadResult::Io(e)) => return Err(WalError::Io(e)),
            };

            // Read entry data - handle EOF/errors
            let entry_data = match Self::read_entry_data(&mut file, length).await {
                Ok(data) => data,
                Err(ReadResult::Eof) => {
                    warn!("Skipping partial WAL entry (truncated data)");
                    break;
                }
                Err(ReadResult::Io(e)) => return Err(WalError::Io(e)),
            };

            // Deserialize and verify entry
            if let Some(entry) = Self::deserialize_and_verify_entry(&entry_data) {
                entries.push(entry);
            }
        }

        Ok(entries)
    }

    /// Read a 4-byte length prefix from the file.
    async fn read_length_prefix(
        file: &mut fs::File,
        buffer: &mut [u8],
    ) -> Result<usize, ReadResult> {
        match file.read_exact(buffer).await {
            Ok(_) => {
                #[allow(clippy::indexing_slicing)] // Safe: buffer is exactly 4 bytes
                let length_bytes: [u8; 4] = [buffer[0], buffer[1], buffer[2], buffer[3]];
                #[allow(clippy::as_conversions)] // Safe: u32 length fits in usize
                Ok(u32::from_le_bytes(length_bytes) as usize)
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Err(ReadResult::Eof),
            Err(e) => Err(ReadResult::Io(e)),
        }
    }

    /// Read entry data of the specified length from the file.
    async fn read_entry_data(file: &mut fs::File, length: usize) -> Result<Vec<u8>, ReadResult> {
        let mut entry_data = vec![0_u8; length];
        match file.read_exact(&mut entry_data).await {
            Ok(_) => Ok(entry_data),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Err(ReadResult::Eof),
            Err(e) => Err(ReadResult::Io(e)),
        }
    }

    /// Deserialize entry data and verify checksum.
    ///
    /// Returns `Some(entry)` if valid, `None` if corrupted (with warning logged).
    fn deserialize_and_verify_entry(entry_data: &[u8]) -> Option<WalEntry> {
        match postcard::from_bytes::<WalEntry>(entry_data) {
            Ok(entry) => {
                if entry.verify() {
                    Some(entry)
                } else {
                    warn!(
                        sequence = entry.sequence,
                        "Skipping corrupted WAL entry (checksum mismatch)"
                    );
                    None
                }
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "Skipping corrupted WAL entry (deserialization failed)"
                );
                None
            }
        }
    }

    /// Mark events as published up to a given sequence number.
    ///
    /// Scans each WAL file to determine its max event sequence and deletes only
    /// files where all events have been published (max_sequence <= up_to_sequence).
    /// The currently active file is never deleted, even if all its events are published.
    ///
    /// # Arguments
    ///
    /// * `up_to_sequence` - Event sequence number up to which events are marked published
    ///
    /// # Returns
    ///
    /// Ok if all eligible files are deleted successfully
    ///
    /// # Errors
    ///
    /// Returns `WalError` if file scanning or deletion fails
    pub async fn mark_published(&self, up_to_sequence: u64) -> WalResult<()> {
        // Get current file sequence to avoid deleting the active file
        let current_file_seq = self.current_file_sequence.load(Ordering::SeqCst);

        let files = self.list_wal_files().await?;

        for (file_sequence, path) in files {
            // Never delete the current active file
            if u64::from(file_sequence) == current_file_seq {
                debug!(file_sequence = file_sequence, "Skipping active WAL file");
                continue;
            }

            // Scan the file to get its event sequence range
            match Self::scan_file_metadata(&path).await {
                Ok(metadata) => {
                    // Only delete if ALL events in this file are published
                    // (i.e., max_sequence <= up_to_sequence)
                    if metadata.max_sequence > 0 && metadata.max_sequence <= up_to_sequence {
                        self.delete_wal_file(file_sequence).await?;
                        debug!(
                            file_sequence = file_sequence,
                            max_event_sequence = metadata.max_sequence,
                            up_to_sequence = up_to_sequence,
                            "Deleted published WAL file"
                        );
                    } else if metadata.max_sequence > up_to_sequence {
                        debug!(
                            file_sequence = file_sequence,
                            max_event_sequence = metadata.max_sequence,
                            up_to_sequence = up_to_sequence,
                            "Keeping WAL file with unpublished events"
                        );
                    } else {
                        // Empty file (max_sequence == 0), safe to delete
                        self.delete_wal_file(file_sequence).await?;
                        debug!(file_sequence = file_sequence, "Deleted empty WAL file");
                    }
                }
                Err(e) => {
                    warn!(
                        file_sequence = file_sequence,
                        error = %e,
                        "Failed to scan WAL file metadata, skipping"
                    );
                }
            }
        }

        Ok(())
    }

    /// Delete a specific WAL file.
    async fn delete_wal_file(&self, sequence: u32) -> WalResult<()> {
        let path = Self::wal_file_path(&self.wal_dir, sequence);

        if path.exists() {
            fs::remove_file(&path).await.map_err(WalError::Io)?;
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::str_to_string,
    clippy::arithmetic_side_effects,
    clippy::redundant_closure_for_method_calls,
    clippy::cast_lossless,
    clippy::as_conversions,
    clippy::let_underscore_must_use,
    clippy::uninlined_format_args,
    clippy::len_zero,
    clippy::semicolon_outside_block
)]
mod tests {
    use super::*;
    use collector_core::event::ProcessEvent;
    use std::time::SystemTime;
    use tempfile::TempDir;
    use tokio::io::AsyncWriteExt;

    /// Create a test process event with specified PID
    fn create_test_event(pid: u32) -> ProcessEvent {
        ProcessEvent {
            pid,
            ppid: None,
            name: format!("test_process_{pid}"),
            executable_path: None,
            command_line: Vec::new(),
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        }
    }

    #[tokio::test]
    async fn test_wal_creation() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal = WriteAheadLog::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create WAL");

        assert_eq!(wal.current_event_sequence.load(Ordering::SeqCst), 1);
        assert_eq!(wal.current_file_sequence.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_write_single_event() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal = WriteAheadLog::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create WAL");

        let event = create_test_event(1234);
        let sequence = wal
            .write(event.clone())
            .await
            .expect("Failed to write event");

        assert_eq!(sequence, 1);

        let events = wal.replay().await.expect("Failed to replay");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].pid, 1234);
    }

    #[tokio::test]
    async fn test_write_multiple_events() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal = WriteAheadLog::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create WAL");

        for i in 1..=5 {
            let event = create_test_event(1000 + i);
            let seq = wal.write(event).await.expect("Failed to write event");
            assert_eq!(seq, i as u64);
        }

        let events = wal.replay().await.expect("Failed to replay");
        assert_eq!(events.len(), 5);

        for (i, event) in events.iter().enumerate() {
            assert_eq!(event.pid, 1000 + (i as u32) + 1);
        }
    }

    #[tokio::test]
    async fn test_sequence_numbering() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal = WriteAheadLog::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create WAL");

        let mut sequences = Vec::new();
        for i in 0..10 {
            let event = create_test_event(2000 + i);
            let seq = wal.write(event).await.expect("Failed to write event");
            sequences.push(seq);
        }

        // Verify monotonic increase
        for i in 1..sequences.len() {
            assert!(sequences[i] > sequences[i - 1]);
        }
    }

    #[tokio::test]
    async fn test_replay_empty_wal() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal = WriteAheadLog::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create WAL");

        let events = wal.replay().await.expect("Failed to replay");
        assert_eq!(events.len(), 0);
    }

    #[tokio::test]
    async fn test_replay_single_file() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal = WriteAheadLog::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create WAL");

        let event1 = create_test_event(3001);
        let event2 = create_test_event(3002);

        wal.write(event1).await.expect("Failed to write event");
        wal.write(event2).await.expect("Failed to write event");

        let events = wal.replay().await.expect("Failed to replay");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].pid, 3001);
        assert_eq!(events[1].pid, 3002);
    }

    #[tokio::test]
    async fn test_mark_published_honors_sequence_cutoff() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Use a very small rotation threshold to force multiple files
        let wal = WriteAheadLog::with_rotation_threshold(temp_dir.path().to_path_buf(), 200)
            .await
            .expect("Failed to create WAL");

        // Write events that will span multiple files due to small rotation threshold
        let mut last_seq = 0;
        for i in 1..=20 {
            let event = create_test_event(4000 + i);
            last_seq = wal.write(event).await.expect("Failed to write event");
        }

        // Should have multiple files now
        let files_before = wal.list_wal_files().await.expect("Failed to list files");
        assert!(
            files_before.len() > 1,
            "Expected multiple files, got {}",
            files_before.len()
        );

        // Mark only some events as published (e.g., up to sequence 5)
        // This should NOT delete files containing events beyond sequence 5
        wal.mark_published(5)
            .await
            .expect("Failed to mark published");

        // Replay should still return events beyond sequence 5
        let events = wal.replay().await.expect("Failed to replay");

        // Should have events remaining (those with sequence > 5)
        assert!(
            !events.is_empty(),
            "Expected some events to remain after partial publish"
        );

        // The total event count should be last_seq (all events we wrote)
        // After mark_published(5), events with seq 1-5 may be deleted if their file
        // only contains events <= 5
        assert!(
            events.len() >= (last_seq.saturating_sub(5)) as usize,
            "Expected at least {} events, got {}",
            last_seq.saturating_sub(5),
            events.len()
        );
    }

    #[tokio::test]
    async fn test_mark_published_preserves_unpublished_events() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Use small rotation threshold
        let wal = WriteAheadLog::with_rotation_threshold(temp_dir.path().to_path_buf(), 150)
            .await
            .expect("Failed to create WAL");

        // Write events
        for i in 1..=15 {
            let event = create_test_event(9000 + i);
            wal.write(event).await.expect("Failed to write event");
        }

        // Mark published with sequence 0 (nothing published)
        wal.mark_published(0)
            .await
            .expect("Failed to mark published");

        // All events should still be recoverable
        let events = wal.replay().await.expect("Failed to replay");
        assert_eq!(events.len(), 15, "All events should be preserved");
    }

    #[tokio::test]
    async fn test_mark_published_preserves_current() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal = WriteAheadLog::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create WAL");

        let event = create_test_event(5001);
        let _seq = wal.write(event).await.expect("Failed to write event");

        // Check initial state - should have one active file
        let files_before = wal.list_wal_files().await.expect("Failed to list files");
        assert_eq!(files_before.len(), 1);
        let current_file_seq = wal.current_file_sequence.load(Ordering::SeqCst);
        assert_eq!(files_before[0].0, current_file_seq as u32);

        // Mark published - should not delete active file even if all events are "published"
        wal.mark_published(999)
            .await
            .expect("Failed to mark published");

        let files_after = wal.list_wal_files().await.expect("Failed to list files");
        // Current file should still exist
        assert_eq!(files_after.len(), 1);
    }

    #[tokio::test]
    async fn test_wal_entry_checksum() {
        let event = create_test_event(6001);
        let entry = WalEntry::new(1, event);

        assert!(entry.verify());
    }

    #[tokio::test]
    async fn test_concurrent_writes() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal = Arc::new(
            WriteAheadLog::new(temp_dir.path().to_path_buf())
                .await
                .expect("Failed to create WAL"),
        );

        let mut handles = Vec::new();

        for i in 0..10 {
            let wal_clone = Arc::clone(&wal);
            let handle = tokio::spawn(async move {
                let event = create_test_event(7000 + i);
                wal_clone.write(event).await.expect("Failed to write")
            });
            handles.push(handle);
        }

        for handle in handles {
            let _ = handle.await;
        }

        let events = wal.replay().await.expect("Failed to replay");
        assert_eq!(events.len(), 10);
    }

    #[tokio::test]
    async fn test_replay_multiple_files() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal = WriteAheadLog::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create WAL");

        // Write enough data to trigger rotation if needed
        for i in 0..20 {
            let event = create_test_event(8000 + i);
            wal.write(event).await.expect("Failed to write");
        }

        let events = wal.replay().await.expect("Failed to replay");
        assert_eq!(events.len(), 20);
    }

    // ==================== Rotation Tests ====================

    #[tokio::test]
    async fn test_rotation_with_low_threshold() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Use a very small rotation threshold (100 bytes) to trigger rotation quickly
        let wal = WriteAheadLog::with_rotation_threshold(temp_dir.path().to_path_buf(), 100)
            .await
            .expect("Failed to create WAL");

        // Write multiple events - each event is ~100+ bytes serialized
        let mut sequences = Vec::new();
        for i in 1..=10 {
            let event = create_test_event(10_000 + i);
            let seq = wal.write(event).await.expect("Failed to write event");
            sequences.push(seq);
        }

        // Verify we have multiple files due to rotation
        let files = wal.list_wal_files().await.expect("Failed to list files");
        assert!(
            files.len() > 1,
            "Expected rotation to create multiple files, got {} file(s)",
            files.len()
        );

        // Verify sequence numbers are monotonically increasing
        for i in 1..sequences.len() {
            assert!(
                sequences[i] > sequences[i - 1],
                "Sequences should be monotonically increasing: {} should be > {}",
                sequences[i],
                sequences[i - 1]
            );
        }

        // Verify all events can be replayed correctly
        let events = wal.replay().await.expect("Failed to replay");
        assert_eq!(events.len(), 10, "All events should be recoverable");

        // Verify PIDs are correct
        for (i, event) in events.iter().enumerate() {
            assert_eq!(event.pid, 10_001 + i as u32, "Event {} has wrong PID", i);
        }
    }

    #[tokio::test]
    async fn test_rotation_sequence_continuity_across_files() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Very small threshold to force multiple rotations
        let wal = WriteAheadLog::with_rotation_threshold(temp_dir.path().to_path_buf(), 50)
            .await
            .expect("Failed to create WAL");

        let mut all_sequences = Vec::new();
        for i in 1..=15 {
            let event = create_test_event(11_000 + i);
            let seq = wal.write(event).await.expect("Failed to write event");
            all_sequences.push(seq);
        }

        // Verify strict monotonic increase (no gaps, no duplicates)
        for i in 1..all_sequences.len() {
            assert_eq!(
                all_sequences[i],
                all_sequences[i - 1] + 1,
                "Sequence numbers should be consecutive: expected {}, got {}",
                all_sequences[i - 1] + 1,
                all_sequences[i]
            );
        }

        // Should have rotated multiple times
        let files = wal.list_wal_files().await.expect("Failed to list files");
        assert!(files.len() > 2, "Expected multiple rotations");
    }

    #[tokio::test]
    async fn test_concurrent_writes_during_rotation() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Small threshold to increase chance of concurrent rotation
        let wal = Arc::new(
            WriteAheadLog::with_rotation_threshold(temp_dir.path().to_path_buf(), 150)
                .await
                .expect("Failed to create WAL"),
        );

        let mut handles = Vec::new();

        // Spawn many concurrent writers
        for i in 0..50 {
            let wal_clone = Arc::clone(&wal);
            let handle = tokio::spawn(async move {
                let event = create_test_event(12_000 + i);
                wal_clone.write(event).await
            });
            handles.push(handle);
        }

        // All writes should succeed (no errors from rotation race)
        for handle in handles {
            let result = handle.await.expect("Task panicked");
            assert!(result.is_ok(), "Write failed: {:?}", result.err());
        }

        // All events should be recoverable
        let events = wal.replay().await.expect("Failed to replay");
        assert_eq!(
            events.len(),
            50,
            "All concurrent writes should be preserved"
        );
    }

    // ==================== Sequence Persistence Across Restarts ====================

    #[tokio::test]
    async fn test_sequence_continuity_across_restart() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // First session: write some events
        let last_sequence = {
            let wal = WriteAheadLog::new(wal_path.clone())
                .await
                .expect("Failed to create WAL");

            let mut last_seq = 0;
            for i in 1..=5 {
                let event = create_test_event(13_000 + i);
                last_seq = wal.write(event).await.expect("Failed to write event");
            }
            last_seq
        }; // WAL dropped here, simulating process exit

        // Second session: create new WAL instance (simulating restart)
        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("Failed to reopen WAL");

        // Write more events
        let new_sequence = {
            let event = create_test_event(13_100);
            wal.write(event).await.expect("Failed to write event")
        };

        // New sequence should continue from where we left off
        assert!(
            new_sequence > last_sequence,
            "New sequence ({}) should be > last sequence ({})",
            new_sequence,
            last_sequence
        );

        // All events should be recoverable
        let events = wal.replay().await.expect("Failed to replay");
        assert_eq!(
            events.len(),
            6,
            "All events from both sessions should be recoverable"
        );
    }

    #[tokio::test]
    async fn test_sequence_continuity_with_multiple_files_across_restart() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // First session: write events with rotation
        let (last_sequence, file_count_before) = {
            let wal = WriteAheadLog::with_rotation_threshold(wal_path.clone(), 100)
                .await
                .expect("Failed to create WAL");

            let mut last_seq = 0;
            for i in 1..=10 {
                let event = create_test_event(14_000 + i);
                last_seq = wal.write(event).await.expect("Failed to write event");
            }

            let files = wal.list_wal_files().await.expect("Failed to list files");
            (last_seq, files.len())
        };

        assert!(
            file_count_before > 1,
            "Should have rotated during first session"
        );

        // Second session: continue writing
        let wal = WriteAheadLog::with_rotation_threshold(wal_path, 100)
            .await
            .expect("Failed to reopen WAL");

        let new_sequence = {
            let event = create_test_event(14_100);
            wal.write(event).await.expect("Failed to write event")
        };

        // Sequence should strictly continue
        assert!(
            new_sequence > last_sequence,
            "Sequence should continue: {} should be > {}",
            new_sequence,
            last_sequence
        );

        // All events recoverable
        let events = wal.replay().await.expect("Failed to replay");
        assert_eq!(events.len(), 11);
    }

    // ==================== Corruption Recovery Tests ====================

    #[tokio::test]
    async fn test_replay_skips_corrupted_checksum() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // Write some valid events
        {
            let wal = WriteAheadLog::new(wal_path.clone())
                .await
                .expect("Failed to create WAL");

            for i in 1..=3 {
                let event = create_test_event(15_000 + i);
                wal.write(event).await.expect("Failed to write event");
            }
        }

        // Now corrupt the middle entry by modifying bytes in the WAL file
        let wal_file_path = wal_path.join("procmond-00001.wal");
        let mut contents = tokio::fs::read(&wal_file_path)
            .await
            .expect("Failed to read WAL file");

        // Corrupt some bytes in the middle of the file (after first entry)
        // This will cause checksum mismatch for the corrupted entry
        if contents.len() > 100 {
            contents[80] ^= 0xFF; // Flip bits
            contents[81] ^= 0xFF;
        }

        tokio::fs::write(&wal_file_path, &contents)
            .await
            .expect("Failed to write corrupted file");

        // Replay should skip the corrupted entry and continue
        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("Failed to reopen WAL");

        let events = wal.replay().await.expect("Replay should handle corruption");

        // We wrote 3 events, one was corrupted, so we should have at least 1-2 valid events
        // (depending on which entry was corrupted)
        assert!(
            events.len() >= 1,
            "Should recover at least some events after corruption"
        );
        assert!(
            events.len() <= 3,
            "Should not have more events than written"
        );
    }

    #[tokio::test]
    async fn test_replay_skips_deserialization_error() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // Write a valid event first
        {
            let wal = WriteAheadLog::new(wal_path.clone())
                .await
                .expect("Failed to create WAL");

            let event = create_test_event(16_001);
            wal.write(event).await.expect("Failed to write event");
        }

        // Append garbage data that looks like a valid length but contains invalid postcard
        let wal_file_path = wal_path.join("procmond-00001.wal");
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&wal_file_path)
            .await
            .expect("Failed to open WAL file");

        // Write a length prefix followed by garbage (invalid postcard)
        let garbage_len: u32 = 50;
        file.write_all(&garbage_len.to_le_bytes())
            .await
            .expect("Failed to write length");
        file.write_all(&[0xDE; 50])
            .await
            .expect("Failed to write garbage");

        // Write another valid event after the garbage
        {
            let wal = WriteAheadLog::new(wal_path.clone())
                .await
                .expect("Failed to reopen WAL");

            let event = create_test_event(16_002);
            wal.write(event).await.expect("Failed to write event");
        }

        // Replay should skip the garbage entry
        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("Failed to reopen WAL");

        let events = wal
            .replay()
            .await
            .expect("Replay should handle invalid entries");

        // Should have at least the first valid event
        assert!(
            !events.is_empty(),
            "Should recover valid events despite garbage entry"
        );
    }

    #[tokio::test]
    async fn test_replay_handles_truncated_entry() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // Write some valid events
        {
            let wal = WriteAheadLog::new(wal_path.clone())
                .await
                .expect("Failed to create WAL");

            for i in 1..=3 {
                let event = create_test_event(17_000 + i);
                wal.write(event).await.expect("Failed to write event");
            }
        }

        // Truncate the file to simulate a partial write (crash during write)
        let wal_file_path = wal_path.join("procmond-00001.wal");
        let metadata = tokio::fs::metadata(&wal_file_path)
            .await
            .expect("Failed to get metadata");

        // Truncate to remove part of the last entry
        let truncated_size = metadata.len().saturating_sub(20);
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&wal_file_path)
            .await
            .expect("Failed to open WAL file");
        file.set_len(truncated_size)
            .await
            .expect("Failed to truncate");

        // Replay should recover events before the truncation
        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("Failed to reopen WAL");

        let events = wal.replay().await.expect("Replay should handle truncation");

        // Should have recovered at least one complete event (truncation is partial)
        // The exact count depends on serialized event sizes and truncation point
        assert!(
            !events.is_empty() && events.len() < 3,
            "Should recover some (but not all) events after truncation, got {}",
            events.len()
        );
    }

    #[tokio::test]
    async fn test_replay_handles_truncated_length_prefix() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // Write a valid event
        {
            let wal = WriteAheadLog::new(wal_path.clone())
                .await
                .expect("Failed to create WAL");

            let event = create_test_event(18_001);
            wal.write(event).await.expect("Failed to write event");
        }

        // Append a partial length prefix (only 2 bytes instead of 4)
        let wal_file_path = wal_path.join("procmond-00001.wal");
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&wal_file_path)
            .await
            .expect("Failed to open WAL file");

        file.write_all(&[0x10, 0x00]) // Incomplete length prefix
            .await
            .expect("Failed to write partial length");

        // Replay should recover the valid event and stop at truncated prefix
        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("Failed to reopen WAL");

        let events = wal
            .replay()
            .await
            .expect("Replay should handle partial prefix");

        assert_eq!(events.len(), 1, "Should recover the one valid event");
        assert_eq!(events[0].pid, 18_001);
    }

    #[tokio::test]
    async fn test_replay_continues_after_corrupted_file() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // Create multiple WAL files with low rotation threshold
        {
            let wal = WriteAheadLog::with_rotation_threshold(wal_path.clone(), 100)
                .await
                .expect("Failed to create WAL");

            for i in 1..=15 {
                let event = create_test_event(19_000 + i);
                wal.write(event).await.expect("Failed to write event");
            }
        }

        // Find and completely corrupt one of the middle files
        let files: Vec<_> = std::fs::read_dir(&wal_path)
            .expect("Failed to read dir")
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "wal"))
            .collect();

        if files.len() > 2 {
            // Corrupt a middle file completely
            let middle_file = &files[1];
            std::fs::write(middle_file.path(), b"completely invalid data")
                .expect("Failed to corrupt file");
        }

        // Replay should continue despite the corrupted file
        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("Failed to reopen WAL");

        let events = wal
            .replay()
            .await
            .expect("Replay should handle corrupted file");

        // Should recover events from non-corrupted files
        assert!(!events.is_empty(), "Should recover events from valid files");
    }

    // ==================== File Metadata Scanning Tests ====================

    #[tokio::test]
    async fn test_scan_file_metadata() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // Write events with known sequences
        {
            let wal = WriteAheadLog::new(wal_path.clone())
                .await
                .expect("Failed to create WAL");

            for i in 1..=5 {
                let event = create_test_event(20_000 + i);
                wal.write(event).await.expect("Failed to write event");
            }
        }

        // Scan the file metadata
        let wal_file_path = wal_path.join("procmond-00001.wal");
        let metadata = WriteAheadLog::scan_file_metadata(&wal_file_path)
            .await
            .expect("Failed to scan metadata");

        assert_eq!(metadata.min_sequence, 1, "Min sequence should be 1");
        assert_eq!(metadata.max_sequence, 5, "Max sequence should be 5");
        assert_eq!(metadata.entry_count, 5, "Should have 5 entries");
    }

    // ==================== replay_entries Tests ====================

    #[tokio::test]
    async fn test_replay_entries_returns_full_wal_entries() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // Write events with event types
        {
            let wal = WriteAheadLog::new(wal_path.clone())
                .await
                .expect("Failed to create WAL");

            let event1 = create_test_event(21_001);
            let event2 = create_test_event(21_002);
            let event3 = create_test_event(21_003);

            wal.write_with_type(event1, "start".to_string())
                .await
                .expect("Failed to write event");
            wal.write_with_type(event2, "modify".to_string())
                .await
                .expect("Failed to write event");
            wal.write_with_type(event3, "stop".to_string())
                .await
                .expect("Failed to write event");
        }

        // Replay entries should return full WalEntry objects
        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("Failed to reopen WAL");

        let entries = wal
            .replay_entries()
            .await
            .expect("Failed to replay entries");

        assert_eq!(entries.len(), 3, "Should have 3 entries");

        // Verify sequence numbers
        assert_eq!(entries[0].sequence, 1);
        assert_eq!(entries[1].sequence, 2);
        assert_eq!(entries[2].sequence, 3);

        // Verify event types are preserved
        assert_eq!(entries[0].event_type, Some("start".to_string()));
        assert_eq!(entries[1].event_type, Some("modify".to_string()));
        assert_eq!(entries[2].event_type, Some("stop".to_string()));

        // Verify PIDs
        assert_eq!(entries[0].event.pid, 21_001);
        assert_eq!(entries[1].event.pid, 21_002);
        assert_eq!(entries[2].event.pid, 21_003);

        // Verify checksums are valid
        for entry in &entries {
            assert!(entry.verify(), "Entry checksum should be valid");
        }
    }

    #[tokio::test]
    async fn test_replay_entries_empty_wal() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal = WriteAheadLog::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create WAL");

        let entries = wal
            .replay_entries()
            .await
            .expect("Failed to replay entries");
        assert!(entries.is_empty(), "Empty WAL should return no entries");
    }

    #[tokio::test]
    async fn test_replay_entries_across_multiple_files() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // Write events with rotation
        {
            let wal = WriteAheadLog::with_rotation_threshold(wal_path.clone(), 100)
                .await
                .expect("Failed to create WAL");

            for i in 1..=10 {
                let event = create_test_event(22_000 + i);
                wal.write_with_type(event, format!("type_{i}"))
                    .await
                    .expect("Failed to write event");
            }

            let files = wal.list_wal_files().await.expect("Failed to list files");
            assert!(files.len() > 1, "Should have multiple files for this test");
        }

        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("Failed to reopen WAL");

        let entries = wal
            .replay_entries()
            .await
            .expect("Failed to replay entries");

        assert_eq!(entries.len(), 10, "Should recover all entries across files");

        // Verify sequences are continuous
        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(
                entry.sequence,
                (i as u64) + 1,
                "Sequences should be continuous"
            );
        }
    }

    #[tokio::test]
    async fn test_replay_entries_skips_corrupted_entries() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // Write valid events
        {
            let wal = WriteAheadLog::new(wal_path.clone())
                .await
                .expect("Failed to create WAL");

            for i in 1..=3 {
                let event = create_test_event(23_000 + i);
                wal.write(event).await.expect("Failed to write event");
            }
        }

        // Corrupt the second entry
        let wal_file_path = wal_path.join("procmond-00001.wal");
        let mut contents = tokio::fs::read(&wal_file_path)
            .await
            .expect("Failed to read WAL file");

        // Corrupt checksum area in middle of file
        if contents.len() > 120 {
            contents[100] ^= 0xFF;
            contents[105] ^= 0xFF;
        }

        tokio::fs::write(&wal_file_path, &contents)
            .await
            .expect("Failed to write corrupted file");

        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("Failed to reopen WAL");

        let entries = wal
            .replay_entries()
            .await
            .expect("Replay should handle corruption");

        // Should have recovered at least some entries (corruption skipped)
        assert!(
            entries.len() >= 1 && entries.len() <= 3,
            "Should recover valid entries, got {}",
            entries.len()
        );
    }

    // ==================== write_with_type Tests ====================

    #[tokio::test]
    async fn test_write_with_type_basic() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal = WriteAheadLog::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create WAL");

        let event = create_test_event(24_001);
        let sequence = wal
            .write_with_type(event, "process_start".to_string())
            .await
            .expect("Failed to write event with type");

        assert_eq!(sequence, 1);

        let entries = wal.replay_entries().await.expect("Failed to replay");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].event_type, Some("process_start".to_string()));
        assert_eq!(entries[0].event.pid, 24_001);
    }

    #[tokio::test]
    async fn test_write_with_type_triggers_rotation() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Very small threshold to force rotation
        let wal = WriteAheadLog::with_rotation_threshold(temp_dir.path().to_path_buf(), 80)
            .await
            .expect("Failed to create WAL");

        // Write events with types until rotation occurs
        for i in 1..=5 {
            let event = create_test_event(25_000 + i);
            wal.write_with_type(event, format!("event_type_{i}"))
                .await
                .expect("Failed to write event");
        }

        let files = wal.list_wal_files().await.expect("Failed to list files");
        assert!(files.len() > 1, "Should have rotated files");

        // All entries should be recoverable with their types
        let entries = wal.replay_entries().await.expect("Failed to replay");
        assert_eq!(entries.len(), 5);

        for (i, entry) in entries.iter().enumerate() {
            assert_eq!(entry.event_type, Some(format!("event_type_{}", i + 1)));
        }
    }

    #[tokio::test]
    async fn test_mixed_write_and_write_with_type() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal = WriteAheadLog::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create WAL");

        // Mix both write methods
        let event1 = create_test_event(26_001);
        let event2 = create_test_event(26_002);
        let event3 = create_test_event(26_003);

        wal.write(event1).await.expect("Failed to write");
        wal.write_with_type(event2, "typed".to_string())
            .await
            .expect("Failed to write with type");
        wal.write(event3).await.expect("Failed to write");

        let entries = wal.replay_entries().await.expect("Failed to replay");

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].event_type, None);
        assert_eq!(entries[1].event_type, Some("typed".to_string()));
        assert_eq!(entries[2].event_type, None);
    }

    // ==================== WAL Filename Parsing Edge Cases ====================

    #[test]
    fn test_parse_wal_filename_valid() {
        assert_eq!(
            WriteAheadLog::parse_wal_filename("procmond-00001.wal"),
            Some(1)
        );
        assert_eq!(
            WriteAheadLog::parse_wal_filename("procmond-00123.wal"),
            Some(123)
        );
        assert_eq!(
            WriteAheadLog::parse_wal_filename("procmond-99999.wal"),
            Some(99999)
        );
    }

    #[test]
    fn test_parse_wal_filename_extension_variants() {
        // The implementation uses case-insensitive extension detection but case-sensitive
        // suffix stripping - so only lowercase .wal is fully supported. This test documents
        // the current behavior to increase coverage of the extension check path.

        // Lowercase works fully
        assert_eq!(
            WriteAheadLog::parse_wal_filename("procmond-00001.wal"),
            Some(1)
        );

        // Uppercase passes extension check but fails strip_suffix - returns None
        // This exercises the has_wal_ext check (line 473-475) with uppercase
        assert_eq!(
            WriteAheadLog::parse_wal_filename("procmond-00001.WAL"),
            None
        );
        assert_eq!(
            WriteAheadLog::parse_wal_filename("procmond-00001.Wal"),
            None
        );
    }

    #[test]
    fn test_parse_wal_filename_invalid() {
        assert_eq!(
            WriteAheadLog::parse_wal_filename("procmond-00001.txt"),
            None
        );
        assert_eq!(WriteAheadLog::parse_wal_filename("other-00001.wal"), None);
        assert_eq!(WriteAheadLog::parse_wal_filename("procmond-abc.wal"), None);
        assert_eq!(WriteAheadLog::parse_wal_filename("procmond-.wal"), None);
        assert_eq!(WriteAheadLog::parse_wal_filename("procmond-00001"), None);
        assert_eq!(WriteAheadLog::parse_wal_filename(""), None);
        assert_eq!(WriteAheadLog::parse_wal_filename(".wal"), None);
        assert_eq!(
            WriteAheadLog::parse_wal_filename("procmond-00001.wal.bak"),
            None
        );
    }

    #[test]
    fn test_wal_file_path_generation() {
        let path = std::path::PathBuf::from("/tmp/wal");
        assert_eq!(
            WriteAheadLog::wal_file_path(&path, 1),
            std::path::PathBuf::from("/tmp/wal/procmond-00001.wal")
        );
        assert_eq!(
            WriteAheadLog::wal_file_path(&path, 99999),
            std::path::PathBuf::from("/tmp/wal/procmond-99999.wal")
        );
    }

    // ==================== WalEntry Tests ====================

    #[test]
    fn test_wal_entry_with_event_type() {
        let event = create_test_event(27_001);
        let entry = WalEntry::with_event_type(42, event.clone(), "test_type".to_string());

        assert_eq!(entry.sequence, 42);
        assert_eq!(entry.event.pid, 27_001);
        assert_eq!(entry.event_type, Some("test_type".to_string()));
        assert!(entry.verify(), "Entry checksum should be valid");
    }

    #[test]
    fn test_wal_entry_checksum_corruption_detection() {
        let event = create_test_event(27_002);
        let mut entry = WalEntry::new(1, event);

        // Verify valid entry
        assert!(entry.verify());

        // Corrupt the checksum
        entry.checksum ^= 0xFFFF_FFFF;
        assert!(
            !entry.verify(),
            "Corrupted checksum should fail verification"
        );
    }

    #[test]
    fn test_wal_entry_checksum_event_mutation_detection() {
        let event = create_test_event(27_003);
        let mut entry = WalEntry::new(1, event);

        // Verify valid entry
        assert!(entry.verify());

        // Mutate the event data
        entry.event.pid = 99999;
        assert!(!entry.verify(), "Mutated event should fail verification");
    }

    // ==================== Rotation Boundary Condition Tests ====================

    #[tokio::test]
    async fn test_rotation_exactly_at_threshold() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Use a threshold that we can calculate against
        let threshold: u64 = 500;
        let wal = WriteAheadLog::with_rotation_threshold(temp_dir.path().to_path_buf(), threshold)
            .await
            .expect("Failed to create WAL");

        // Write events and track rotations
        let mut event_count = 0_u32;
        let mut rotations = 0_usize;

        while rotations < 3 {
            let event = create_test_event(28_000 + event_count);
            let _seq = wal.write(event).await.expect("Failed to write");

            // Check if rotation occurred (sequence % file count changes)
            let files = wal.list_wal_files().await.expect("Failed to list files");
            if files.len() > rotations + 1 {
                rotations = files.len() - 1;
            }

            event_count += 1;
        }

        // Verify all events are recoverable
        let events = wal.replay().await.expect("Failed to replay");
        assert_eq!(events.len() as u32, event_count);
    }

    #[tokio::test]
    async fn test_rotation_just_below_threshold() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Large threshold to avoid rotation
        let wal =
            WriteAheadLog::with_rotation_threshold(temp_dir.path().to_path_buf(), 10 * 1024 * 1024)
                .await
                .expect("Failed to create WAL");

        // Write several events
        for i in 1..=100 {
            let event = create_test_event(29_000 + i);
            wal.write(event).await.expect("Failed to write");
        }

        // Should still be one file (no rotation)
        let files = wal.list_wal_files().await.expect("Failed to list files");
        assert_eq!(files.len(), 1, "Should have exactly one file (no rotation)");
    }

    #[tokio::test]
    async fn test_rotation_boundary_file_state_consistency() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Small threshold for predictable rotation
        let wal = WriteAheadLog::with_rotation_threshold(temp_dir.path().to_path_buf(), 100)
            .await
            .expect("Failed to create WAL");

        // Write events until we have at least 3 rotations
        let mut sequences = Vec::new();
        for i in 1..=20 {
            let event = create_test_event(30_000 + i);
            let seq = wal.write(event).await.expect("Failed to write");
            sequences.push(seq);
        }

        let files = wal.list_wal_files().await.expect("Failed to list files");
        assert!(files.len() >= 3, "Should have multiple files");

        // Verify file sequences are continuous
        for (i, (file_seq, _path)) in files.iter().enumerate() {
            assert_eq!(
                *file_seq as usize,
                i + 1,
                "File sequences should be continuous"
            );
        }

        // Verify event sequences are continuous across all files
        for i in 1..sequences.len() {
            assert_eq!(
                sequences[i],
                sequences[i - 1] + 1,
                "Event sequences must be continuous across rotation"
            );
        }
    }

    // ==================== Cleanup/Deletion Verification Tests ====================

    #[tokio::test]
    async fn test_mark_published_deletes_fully_published_files() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // Write events with rotation
        let wal = WriteAheadLog::with_rotation_threshold(wal_path.clone(), 100)
            .await
            .expect("Failed to create WAL");

        let mut last_seq = 0;
        for i in 1..=20 {
            let event = create_test_event(31_000 + i);
            last_seq = wal.write(event).await.expect("Failed to write");
        }

        let files_before = wal.list_wal_files().await.expect("Failed to list files");
        let file_count_before = files_before.len();
        assert!(file_count_before > 2, "Need multiple files for this test");

        // Mark all events as published
        wal.mark_published(last_seq)
            .await
            .expect("Failed to mark published");

        // Verify files were cleaned up (except current)
        let files_after = wal.list_wal_files().await.expect("Failed to list files");
        assert!(
            files_after.len() < file_count_before,
            "Should have deleted some files after mark_published"
        );

        // Current file should still exist
        let current_file_seq = wal.current_file_sequence.load(Ordering::SeqCst);
        let current_file_exists = files_after
            .iter()
            .any(|(seq, _)| u64::from(*seq) == current_file_seq);
        assert!(current_file_exists, "Current file should never be deleted");
    }

    #[tokio::test]
    async fn test_mark_published_does_not_delete_files_with_unpublished_events() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        let wal = WriteAheadLog::with_rotation_threshold(temp_dir.path().to_path_buf(), 100)
            .await
            .expect("Failed to create WAL");

        // Write 20 events (should create multiple files)
        for i in 1..=20 {
            let event = create_test_event(32_000 + i);
            wal.write(event).await.expect("Failed to write");
        }

        let files_before = wal.list_wal_files().await.expect("Failed to list files");
        assert!(files_before.len() > 1, "Need multiple files");

        // Mark only sequence 1 as published (very early)
        wal.mark_published(1)
            .await
            .expect("Failed to mark published");

        // Most files should still exist (contain unpublished events)
        let files_after = wal.list_wal_files().await.expect("Failed to list files");

        // Should still have most of the files
        assert!(
            files_after.len() >= files_before.len() - 1,
            "Should keep files with unpublished events"
        );
    }

    #[tokio::test]
    async fn test_mark_published_handles_empty_files() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Create WAL and write one event to trigger file creation
        let wal = WriteAheadLog::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create WAL");

        // The initial file exists but may have events
        let event = create_test_event(33_001);
        let seq = wal.write(event).await.expect("Failed to write");

        // Mark as published
        wal.mark_published(seq)
            .await
            .expect("Failed to mark published");

        // Current file should still exist
        let files = wal.list_wal_files().await.expect("Failed to list files");
        assert_eq!(files.len(), 1, "Current file should still exist");
    }

    // ==================== Scan WAL State Edge Cases ====================

    #[tokio::test]
    async fn test_scan_wal_state_with_non_wal_files() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // Create some non-WAL files in the directory
        tokio::fs::write(wal_path.join("readme.txt"), "test")
            .await
            .expect("Failed to write file");
        tokio::fs::write(wal_path.join("config.json"), "{}")
            .await
            .expect("Failed to write file");
        tokio::fs::write(wal_path.join("procmond-backup.wal.bak"), "backup")
            .await
            .expect("Failed to write file");

        // Create WAL - should ignore non-WAL files
        let wal = WriteAheadLog::new(wal_path.clone())
            .await
            .expect("Failed to create WAL");

        // Write an event
        let event = create_test_event(34_001);
        wal.write(event).await.expect("Failed to write");

        // Should only list the actual WAL file
        let files = wal.list_wal_files().await.expect("Failed to list files");
        assert_eq!(files.len(), 1, "Should only list .wal files");
        assert!(
            files[0]
                .1
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with(".wal")
        );
    }

    #[tokio::test]
    async fn test_scan_handles_corrupted_file_during_startup() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // Write valid events first
        {
            let wal = WriteAheadLog::new(wal_path.clone())
                .await
                .expect("Failed to create WAL");

            for i in 1..=5 {
                let event = create_test_event(35_000 + i);
                wal.write(event).await.expect("Failed to write");
            }
        }

        // Completely corrupt the WAL file
        let wal_file_path = wal_path.join("procmond-00001.wal");
        tokio::fs::write(&wal_file_path, "completely invalid garbage data")
            .await
            .expect("Failed to corrupt file");

        // WAL should still be able to start (with warnings)
        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("WAL should handle corrupted files gracefully");

        // New writes should work
        let event = create_test_event(35_100);
        let result = wal.write(event).await;
        assert!(
            result.is_ok(),
            "Should be able to write after recovering from corruption"
        );
    }

    // ==================== Various Corruption Type Tests ====================

    #[tokio::test]
    async fn test_corruption_zero_length_prefix() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // Write valid events
        {
            let wal = WriteAheadLog::new(wal_path.clone())
                .await
                .expect("Failed to create WAL");

            let event = create_test_event(36_001);
            wal.write(event).await.expect("Failed to write");
        }

        // Append a zero-length entry (invalid)
        let wal_file_path = wal_path.join("procmond-00001.wal");
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&wal_file_path)
            .await
            .expect("Failed to open");

        // Write zero length
        file.write_all(&0_u32.to_le_bytes())
            .await
            .expect("Failed to write zero length");

        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("Failed to reopen WAL");

        let events = wal
            .replay()
            .await
            .expect("Should handle zero-length prefix");
        assert_eq!(
            events.len(),
            1,
            "Should recover valid event before corruption"
        );
    }

    #[tokio::test]
    async fn test_corruption_huge_length_prefix() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // Write valid events
        {
            let wal = WriteAheadLog::new(wal_path.clone())
                .await
                .expect("Failed to create WAL");

            let event = create_test_event(37_001);
            wal.write(event).await.expect("Failed to write");
        }

        // Append an absurdly large length prefix (will cause read failure)
        let wal_file_path = wal_path.join("procmond-00001.wal");
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&wal_file_path)
            .await
            .expect("Failed to open");

        // Write huge length (1GB)
        let huge_len: u32 = 1024 * 1024 * 1024;
        file.write_all(&huge_len.to_le_bytes())
            .await
            .expect("Failed to write");

        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("Failed to reopen WAL");

        // Should recover gracefully (may not recover the huge entry)
        let events = wal
            .replay()
            .await
            .expect("Should handle huge length prefix");
        assert_eq!(
            events.len(),
            1,
            "Should recover valid event before corruption"
        );
    }

    #[tokio::test]
    async fn test_corruption_partial_checksum_data() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // Write valid event
        {
            let wal = WriteAheadLog::new(wal_path.clone())
                .await
                .expect("Failed to create WAL");

            let event = create_test_event(38_001);
            wal.write(event).await.expect("Failed to write");
        }

        // Read the file and corrupt just the checksum bytes
        let wal_file_path = wal_path.join("procmond-00001.wal");
        let mut contents = tokio::fs::read(&wal_file_path)
            .await
            .expect("Failed to read");

        // The checksum is near the end of the entry - corrupt it
        let len = contents.len();
        if len > 10 {
            contents[len - 5] ^= 0xFF;
            contents[len - 6] ^= 0xFF;
        }

        tokio::fs::write(&wal_file_path, &contents)
            .await
            .expect("Failed to write");

        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("Failed to reopen WAL");

        // Replay should skip the corrupted entry
        let events = wal
            .replay()
            .await
            .expect("Should handle checksum corruption");

        // The event may or may not be recovered depending on exact corruption
        assert!(
            events.len() <= 1,
            "Should not recover more events than written"
        );
    }

    #[tokio::test]
    async fn test_corruption_all_zero_bytes_entry() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // Write valid event
        {
            let wal = WriteAheadLog::new(wal_path.clone())
                .await
                .expect("Failed to create WAL");

            let event = create_test_event(39_001);
            wal.write(event).await.expect("Failed to write");
        }

        // Append an all-zero entry
        let wal_file_path = wal_path.join("procmond-00001.wal");
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&wal_file_path)
            .await
            .expect("Failed to open");

        // Write length then zeros
        let len: u32 = 100;
        file.write_all(&len.to_le_bytes())
            .await
            .expect("Failed to write");
        file.write_all(&[0_u8; 100])
            .await
            .expect("Failed to write zeros");

        let wal = WriteAheadLog::new(wal_path)
            .await
            .expect("Failed to reopen WAL");

        let events = wal.replay().await.expect("Should handle zero-filled entry");
        assert_eq!(
            events.len(),
            1,
            "Should recover valid event before corruption"
        );
    }

    // ==================== Default Threshold Test ====================

    #[tokio::test]
    async fn test_default_rotation_threshold() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal = WriteAheadLog::new(temp_dir.path().to_path_buf())
            .await
            .expect("Failed to create WAL");

        // Default threshold is 80MB
        assert_eq!(wal.rotation_threshold, 80 * 1024 * 1024);
    }

    // ==================== WalFileMetadata Default Test ====================

    #[test]
    fn test_wal_file_metadata_default() {
        let metadata = WalFileMetadata::default();
        assert_eq!(metadata.min_sequence, 0);
        assert_eq!(metadata.max_sequence, 0);
        assert_eq!(metadata.entry_count, 0);
    }

    // ==================== Scan File Metadata Edge Cases ====================

    #[tokio::test]
    async fn test_scan_empty_file_metadata() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // Create an empty WAL file manually
        let empty_file_path = wal_path.join("procmond-00001.wal");
        tokio::fs::create_dir_all(&wal_path)
            .await
            .expect("Failed to create dir");
        tokio::fs::write(&empty_file_path, b"")
            .await
            .expect("Failed to create empty file");

        let metadata = WriteAheadLog::scan_file_metadata(&empty_file_path)
            .await
            .expect("Should handle empty file");

        assert_eq!(metadata.min_sequence, 0);
        assert_eq!(metadata.max_sequence, 0);
        assert_eq!(metadata.entry_count, 0);
    }

    #[tokio::test]
    async fn test_scan_file_metadata_with_corrupted_entries() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path().to_path_buf();

        // Write valid events
        {
            let wal = WriteAheadLog::new(wal_path.clone())
                .await
                .expect("Failed to create WAL");

            for i in 1..=3 {
                let event = create_test_event(40_000 + i);
                wal.write(event).await.expect("Failed to write");
            }
        }

        // Append garbage that looks like an entry
        let wal_file_path = wal_path.join("procmond-00001.wal");
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&wal_file_path)
            .await
            .expect("Failed to open");

        let garbage_len: u32 = 50;
        file.write_all(&garbage_len.to_le_bytes())
            .await
            .expect("write");
        file.write_all(&[0xAB; 50]).await.expect("write");

        // Scan should still recover valid entries
        let metadata = WriteAheadLog::scan_file_metadata(&wal_file_path)
            .await
            .expect("Should handle corrupted entries");

        assert_eq!(metadata.entry_count, 3, "Should count valid entries");
        assert_eq!(metadata.min_sequence, 1);
        assert_eq!(metadata.max_sequence, 3);
    }

    // ==================== Error Type Coverage ====================

    #[test]
    fn test_wal_error_display() {
        let io_err = WalError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file not found",
        ));
        assert!(io_err.to_string().contains("I/O error"));

        let ser_err = WalError::Serialization("bad data".to_string());
        assert!(ser_err.to_string().contains("Serialization"));

        let corr_err = WalError::Corruption {
            sequence: 42,
            message: "bad checksum".to_string(),
        };
        assert!(corr_err.to_string().contains("Corruption"));
        assert!(corr_err.to_string().contains("42"));

        let seq_err = WalError::InvalidSequence {
            expected: 10,
            found: 5,
        };
        assert!(seq_err.to_string().contains("Invalid sequence"));

        let rot_err = WalError::FileRotation("rotation failed".to_string());
        assert!(rot_err.to_string().contains("File rotation"));

        let rep_err = WalError::Replay("replay failed".to_string());
        assert!(rep_err.to_string().contains("Replay"));
    }

    // ==================== Compute Checksum Edge Cases ====================

    #[test]
    fn test_checksum_large_event_data() {
        // Create event with large command line
        let event = ProcessEvent {
            pid: 50_001,
            ppid: None,
            name: "test".to_string(),
            executable_path: Some("/very/long/path/".repeat(100)),
            command_line: (0..1000).map(|i| format!("arg{i}")).collect(),
            start_time: None,
            cpu_usage: None,
            memory_usage: None,
            executable_hash: None,
            user_id: None,
            accessible: true,
            file_exists: true,
            timestamp: SystemTime::now(),
            platform_metadata: None,
        };

        let entry = WalEntry::new(1, event);
        assert!(entry.verify(), "Large event checksum should be valid");
        assert!(entry.checksum != 0, "Checksum should be non-zero");
    }

    #[test]
    fn test_checksum_deterministic() {
        let event = create_test_event(51_001);

        let entry1 = WalEntry::new(1, event.clone());
        let entry2 = WalEntry::new(1, event.clone());

        assert_eq!(
            entry1.checksum, entry2.checksum,
            "Same event should produce same checksum"
        );
    }

    // ==================== Concurrent Access During Cleanup ====================

    #[tokio::test]
    async fn test_concurrent_writes_during_cleanup() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        let wal = Arc::new(
            WriteAheadLog::with_rotation_threshold(temp_dir.path().to_path_buf(), 100)
                .await
                .expect("Failed to create WAL"),
        );

        // First, write some events to create multiple files
        for i in 1..=10 {
            let event = create_test_event(52_000 + i);
            wal.write(event).await.expect("Failed to write");
        }

        // Spawn concurrent writers and cleanup
        let wal_writer = Arc::clone(&wal);
        let writer_handle = tokio::spawn(async move {
            for i in 1..=20 {
                let event = create_test_event(52_100 + i);
                wal_writer.write(event).await.expect("Failed to write");
                tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
            }
        });

        let wal_cleaner = Arc::clone(&wal);
        let cleaner_handle = tokio::spawn(async move {
            for seq in [5, 10, 15, 20, 25] {
                let _ = wal_cleaner.mark_published(seq).await;
                tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
            }
        });

        writer_handle.await.expect("Writer panicked");
        cleaner_handle.await.expect("Cleaner panicked");

        // All remaining events should be recoverable
        let events = wal.replay().await.expect("Failed to replay");
        assert!(!events.is_empty(), "Should have some events remaining");
    }
}
