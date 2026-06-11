//! Priority-based trigger queue with backpressure handling.
//!
//! This submodule contains [`PriorityTriggerQueue`], which routes trigger
//! requests into high/normal priority lanes and applies backpressure when
//! the queue approaches capacity, along with its [`QueueStatistics`].

#![allow(clippy::as_conversions)]
#![allow(clippy::arithmetic_side_effects)]
#![allow(clippy::integer_division)]

use crate::event::{TriggerPriority, TriggerRequest};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use tracing::warn;

use super::types::TriggerError;

/// Priority-based trigger queue for managing analysis requests.
#[derive(Debug)]
pub struct PriorityTriggerQueue {
    /// High priority queue (Critical, High)
    high_priority: VecDeque<TriggerRequest>,

    /// Normal priority queue (Normal, Low)
    normal_priority: VecDeque<TriggerRequest>,

    /// Maximum total queue size across both priority lanes
    max_queue_size: usize,

    /// Backpressure threshold (percentage of max size)
    backpressure_threshold: f32,

    /// Queue statistics
    stats: QueueStatistics,
}

/// Queue statistics for monitoring.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueueStatistics {
    /// Total triggers enqueued
    pub total_enqueued: u64,

    /// Total triggers processed
    pub total_processed: u64,

    /// Triggers dropped due to backpressure
    pub dropped_backpressure: u64,

    /// Triggers dropped due to queue full
    pub dropped_queue_full: u64,

    /// Current queue depths
    pub high_priority_depth: usize,
    pub normal_priority_depth: usize,
}

impl PriorityTriggerQueue {
    /// Creates a new priority trigger queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use collector_core::trigger::PriorityTriggerQueue;
    ///
    /// let queue = PriorityTriggerQueue::new(1000, 0.8);
    /// ```
    pub fn new(max_queue_size: usize, backpressure_threshold: f32) -> Self {
        Self {
            high_priority: VecDeque::with_capacity(max_queue_size / 2),
            normal_priority: VecDeque::with_capacity(max_queue_size / 2),
            max_queue_size,
            backpressure_threshold,
            stats: QueueStatistics::default(),
        }
    }

    /// Enqueues a trigger request with priority-based routing.
    pub fn enqueue(&mut self, trigger: TriggerRequest) -> Result<(), TriggerError> {
        self.stats.total_enqueued += 1;

        // Check for backpressure
        if self.is_backpressure_active() {
            // Drop low priority triggers during backpressure
            if matches!(trigger.priority, TriggerPriority::Low) {
                self.stats.dropped_backpressure += 1;
                warn!(
                    trigger_id = %trigger.trigger_id,
                    priority = ?trigger.priority,
                    "Dropping low priority trigger due to backpressure"
                );
                return Err(TriggerError::BackpressureActive);
            }
        }

        // Enforce the capacity limit against total queue occupancy rather than
        // hard-reserving half of `max_queue_size` per lane. This lets a single
        // lane borrow unused capacity from the other, so a burst of high
        // priority triggers can use the full budget instead of being rejected
        // at the halfway point.
        if self.len() >= self.max_queue_size {
            self.stats.dropped_queue_full += 1;
            let lane = match trigger.priority {
                TriggerPriority::Critical | TriggerPriority::High => "high_priority",
                TriggerPriority::Normal | TriggerPriority::Low => "normal_priority",
            };
            return Err(TriggerError::QueueFull(lane.to_owned()));
        }

        // Route to appropriate queue based on priority
        match trigger.priority {
            TriggerPriority::Critical | TriggerPriority::High => {
                self.high_priority.push_back(trigger);
                self.stats.high_priority_depth = self.high_priority.len();
            }
            TriggerPriority::Normal | TriggerPriority::Low => {
                self.normal_priority.push_back(trigger);
                self.stats.normal_priority_depth = self.normal_priority.len();
            }
        }

        Ok(())
    }

    /// Dequeues the next trigger request, prioritizing high priority items.
    pub fn dequeue(&mut self) -> Option<TriggerRequest> {
        // Always process high priority first
        if let Some(trigger) = self.high_priority.pop_front() {
            self.stats.total_processed += 1;
            self.stats.high_priority_depth = self.high_priority.len();
            return Some(trigger);
        }

        // Then process normal priority
        if let Some(trigger) = self.normal_priority.pop_front() {
            self.stats.total_processed += 1;
            self.stats.normal_priority_depth = self.normal_priority.len();
            return Some(trigger);
        }

        None
    }

    /// Checks if backpressure is currently active.
    pub fn is_backpressure_active(&self) -> bool {
        let total_depth = self.high_priority.len() + self.normal_priority.len();
        let threshold = (self.max_queue_size as f32 * self.backpressure_threshold) as usize;
        total_depth >= threshold
    }

    /// Returns current queue statistics.
    pub fn get_statistics(&self) -> QueueStatistics {
        let mut stats = self.stats.clone();
        stats.high_priority_depth = self.high_priority.len();
        stats.normal_priority_depth = self.normal_priority.len();
        stats
    }

    /// Returns the total number of pending triggers.
    pub fn len(&self) -> usize {
        self.high_priority.len() + self.normal_priority.len()
    }

    /// Returns true if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.high_priority.is_empty() && self.normal_priority.is_empty()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::event::AnalysisType;
    use std::collections::HashMap;
    use std::time::SystemTime;

    fn make_trigger(priority: TriggerPriority) -> TriggerRequest {
        TriggerRequest {
            trigger_id: "trigger".to_owned(),
            target_collector: "collector".to_owned(),
            analysis_type: AnalysisType::BinaryHash,
            priority,
            target_pid: Some(1),
            target_path: None,
            correlation_id: "corr".to_owned(),
            metadata: HashMap::new(),
            timestamp: SystemTime::now(),
        }
    }

    #[test]
    fn high_priority_lane_can_use_full_budget() {
        // Disable backpressure so this exercises only the capacity limit.
        let budget = 10;
        let mut queue = PriorityTriggerQueue::new(budget, 1.0);

        // Fill the entire budget with high-priority triggers. Before the fix,
        // each lane was capped at budget / 2, so the 6th high-priority trigger
        // was rejected even though the normal lane was empty.
        for _ in 0..budget {
            queue
                .enqueue(make_trigger(TriggerPriority::High))
                .expect("high-priority trigger within total budget must be accepted");
        }

        assert_eq!(queue.len(), budget);
        assert_eq!(queue.get_statistics().dropped_queue_full, 0);

        // The next trigger exceeds the total budget and must be rejected.
        let result = queue.enqueue(make_trigger(TriggerPriority::High));
        assert!(matches!(result, Err(TriggerError::QueueFull(_))));
        assert_eq!(queue.get_statistics().dropped_queue_full, 1);
    }

    #[test]
    fn budget_of_one_yields_one_usable_slot() {
        let mut queue = PriorityTriggerQueue::new(1, 1.0);

        queue
            .enqueue(make_trigger(TriggerPriority::High))
            .expect("a budget of 1 must provide one usable slot");
        assert_eq!(queue.len(), 1);

        let result = queue.enqueue(make_trigger(TriggerPriority::High));
        assert!(matches!(result, Err(TriggerError::QueueFull(_))));
    }

    #[test]
    fn lanes_share_total_budget() {
        let budget = 4;
        let mut queue = PriorityTriggerQueue::new(budget, 1.0);

        queue.enqueue(make_trigger(TriggerPriority::High)).unwrap();
        queue.enqueue(make_trigger(TriggerPriority::High)).unwrap();
        queue
            .enqueue(make_trigger(TriggerPriority::Normal))
            .unwrap();
        queue
            .enqueue(make_trigger(TriggerPriority::Normal))
            .unwrap();
        assert_eq!(queue.len(), budget);

        // Total occupancy reached; any further enqueue is rejected.
        assert!(matches!(
            queue.enqueue(make_trigger(TriggerPriority::Normal)),
            Err(TriggerError::QueueFull(_))
        ));
    }
}
