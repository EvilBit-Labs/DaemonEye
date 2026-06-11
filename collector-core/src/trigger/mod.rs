//! Trigger system for analysis collector coordination.
//!
//! This module provides the infrastructure for coordinating between process
//! monitoring collectors and analysis collectors (Binary Hasher, Memory Analyzer, etc.).
//! It implements trigger condition evaluation, deduplication, rate limiting, and
//! metadata tracking for forensic analysis.

mod manager;
mod queue;
mod sql_evaluator;
mod types;

pub use manager::TriggerManager;
pub use queue::{PriorityTriggerQueue, QueueStatistics};
pub use sql_evaluator::{
    CompiledTriggerCondition, ConditionEvaluationStats, SqlEvaluationStats, SqlTriggerEvaluator,
};
pub use types::{
    ConditionType, DeduplicationKey, ProcessTriggerData, TriggerCapabilities, TriggerCondition,
    TriggerConfig, TriggerEmissionStats, TriggerError, TriggerMetadata, TriggerResourceLimits,
    TriggerStatistics,
};
