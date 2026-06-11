//! In-memory buffered event type used when disconnected from the broker.

use collector_core::event::ProcessEvent;

/// An event buffered in memory when disconnected from the broker.
#[derive(Debug)]
pub(super) struct BufferedEvent {
    /// WAL sequence number for this event.
    pub(super) sequence: u64,
    /// The process event to publish.
    pub(super) event: ProcessEvent,
    /// Topic to publish to.
    pub(super) topic: String,
    /// Estimated size in bytes for buffer accounting.
    pub(super) size_bytes: usize,
}

impl BufferedEvent {
    /// Create a new buffered event with size estimation.
    pub(super) fn new(sequence: u64, event: ProcessEvent, topic: String) -> Self {
        // Estimate size based on event fields
        let size_bytes = Self::estimate_size(&event, &topic);
        Self {
            sequence,
            event,
            topic,
            size_bytes,
        }
    }

    /// Estimate the serialized size of an event.
    pub(super) fn estimate_size(event: &ProcessEvent, topic: &str) -> usize {
        // Base overhead for struct fields
        let mut size = 64_usize;

        // Add string lengths
        size = size.saturating_add(event.name.len());
        if let Some(ref path) = event.executable_path {
            size = size.saturating_add(path.len());
        }
        for arg in &event.command_line {
            size = size.saturating_add(arg.len());
        }
        if let Some(ref hash) = event.executable_hash {
            size = size.saturating_add(hash.len());
        }
        if let Some(ref uid) = event.user_id {
            size = size.saturating_add(uid.len());
        }
        if let Some(ref meta) = event.platform_metadata {
            // Rough estimate for JSON metadata
            size = size.saturating_add(meta.to_string().len());
        }
        size = size.saturating_add(topic.len());

        size
    }
}
