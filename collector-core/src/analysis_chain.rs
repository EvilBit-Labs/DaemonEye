//! Analysis chain coordination for multi-stage analysis workflows.
//!
//! This module provides the infrastructure for managing complex analysis workflows
//! that span multiple collector types. It handles stage dependencies, execution order,
//! result aggregation, correlation, timeout management, and recovery logic.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                AnalysisChainCoordinator                         │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────────────┐ │
//! │  │  Workflow   │  │   Stage     │  │    Result Aggregator    │ │
//! │  │ Definition  │  │ Executor    │  │                         │ │
//! │  └─────────────┘  └─────────────┘  │  - Cross-collector      │ │
//! │         │                │         │    correlation          │ │
//! │         └────────────────┼─────────│  - Result aggregation   │ │
//! │                          │         │  - Status tracking      │ │
//! │                          │         └─────────────────────────┘ │
//! │                          │                   │                 │
//! │                          └───────────────────┘                 │
//! └─────────────────────────────────────────────────────────────────┘
//! ```

use crate::{
    event::{AnalysisType, CollectionEvent, TriggerPriority, TriggerRequest},
    event_bus::{BusEvent, EventBus, EventFilter, EventSubscription},
    source::SourceCaps,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};
use tokio::{
    sync::{Mutex, RwLock, mpsc},
    task::JoinHandle,
    time::{Instant, interval},
};
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

/// Configuration for analysis chain coordination.
///
/// Controls workflow execution behavior, timeout management, and resource limits
/// to ensure reliable multi-stage analysis coordination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisChainConfig {
    /// Maximum number of concurrent workflows
    pub max_concurrent_workflows: usize,

    /// Default timeout for individual analysis stages
    pub default_stage_timeout: Duration,

    /// Maximum total workflow execution time
    pub max_workflow_timeout: Duration,

    /// Maximum number of retry attempts for failed stages
    pub max_retry_attempts: usize,

    /// Delay between retry attempts (exponential backoff base)
    pub retry_base_delay: Duration,

    /// Maximum delay between retry attempts
    pub max_retry_delay: Duration,

    /// Interval for workflow status monitoring
    pub status_monitoring_interval: Duration,

    /// Maximum number of completed workflows to retain for debugging
    pub max_completed_workflows: usize,

    /// Enable detailed workflow execution logging
    pub enable_debug_logging: bool,

    /// Timeout for result aggregation operations
    pub result_aggregation_timeout: Duration,
}

impl Default for AnalysisChainConfig {
    fn default() -> Self {
        Self {
            max_concurrent_workflows: 50,
            default_stage_timeout: Duration::from_secs(300), // 5 minutes
            max_workflow_timeout: Duration::from_secs(1800), // 30 minutes
            max_retry_attempts: 3,
            retry_base_delay: Duration::from_secs(2),
            max_retry_delay: Duration::from_secs(60),
            status_monitoring_interval: Duration::from_secs(30),
            max_completed_workflows: 100,
            enable_debug_logging: false,
            result_aggregation_timeout: Duration::from_secs(30),
        }
    }
}

/// Analysis workflow definition with stage dependencies and execution order.
///
/// Defines a multi-stage analysis workflow that coordinates between different
/// collector types to perform comprehensive threat analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisWorkflowDefinition {
    /// Unique workflow identifier
    pub workflow_id: String,

    /// Human-readable workflow name
    pub name: String,

    /// Workflow description
    pub description: String,

    /// Workflow version for compatibility tracking
    pub version: String,

    /// Analysis stages in dependency order
    pub stages: Vec<AnalysisStage>,

    /// Stage dependency graph (stage_id -> dependencies)
    pub dependencies: HashMap<String, Vec<String>>,

    /// Workflow-level timeout override
    pub timeout: Option<Duration>,

    /// Priority level for the entire workflow
    pub priority: TriggerPriority,

    /// Workflow metadata for correlation and debugging
    pub metadata: HashMap<String, String>,
}

/// Individual analysis stage within a workflow.
///
/// Represents a single analysis step that can be executed by a specific
/// collector type, with its own configuration and requirements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisStage {
    /// Unique stage identifier within the workflow
    pub stage_id: String,

    /// Human-readable stage name
    pub name: String,

    /// Target collector for this stage
    pub target_collector: String,

    /// Type of analysis to perform
    pub analysis_type: AnalysisType,

    /// Stage-specific timeout
    pub timeout: Option<Duration>,

    /// Stage priority (can differ from workflow priority)
    pub priority: TriggerPriority,

    /// Whether this stage is optional (failure doesn't fail workflow)
    pub optional: bool,

    /// Stage-specific configuration
    pub config: HashMap<String, String>,

    /// Input requirements from previous stages
    pub input_requirements: Vec<String>,

    /// Output data that this stage provides
    pub output_data: Vec<String>,
}

/// Current status of an analysis workflow execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WorkflowStatus {
    /// Workflow is queued for execution
    Queued,

    /// Workflow is currently running
    Running,

    /// Workflow completed successfully
    Completed,

    /// Workflow failed due to stage failures
    Failed,

    /// Workflow was cancelled
    Cancelled,

    /// Workflow timed out
    TimedOut,
}

/// Status of an individual analysis stage.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StageStatus {
    /// Stage is waiting for dependencies
    Pending,

    /// Stage is queued for execution
    Queued,

    /// Stage is currently running
    Running,

    /// Stage completed successfully
    Completed,

    /// Stage failed
    Failed,

    /// Stage was skipped due to dependencies
    Skipped,

    /// Stage was cancelled
    Cancelled,

    /// Stage timed out
    TimedOut,
}

/// Analysis workflow execution instance.
///
/// Tracks the execution state of a specific workflow instance, including
/// stage progress, results, timing, and error information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowExecution {
    /// Unique execution identifier
    pub execution_id: String,

    /// Workflow definition being executed
    pub workflow_definition: AnalysisWorkflowDefinition,

    /// Current workflow status
    pub status: WorkflowStatus,

    /// Individual stage statuses
    pub stage_statuses: HashMap<String, StageStatus>,

    /// Stage execution results
    pub stage_results: HashMap<String, AnalysisResult>,

    /// Workflow start time
    pub started_at: SystemTime,

    /// Workflow completion time
    pub completed_at: Option<SystemTime>,

    /// Last status update time
    pub last_updated: SystemTime,

    /// Correlation ID for tracking related events
    pub correlation_id: String,

    /// Execution context and metadata
    pub context: HashMap<String, String>,

    /// Error information if workflow failed
    pub error_info: Option<WorkflowError>,

    /// Progress tracking (completed stages / total stages)
    pub progress: WorkflowProgress,
}

/// Analysis result from a completed stage.
///
/// Contains the output data and metadata from a successfully completed
/// analysis stage, used for correlation and input to subsequent stages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    /// Stage that produced this result
    pub stage_id: String,

    /// Analysis type that was performed
    pub analysis_type: AnalysisType,

    /// Target collector that performed the analysis
    pub collector_id: String,

    /// Result data (JSON-serialized)
    pub result_data: serde_json::Value,

    /// Result metadata
    pub metadata: HashMap<String, String>,

    /// Analysis completion time
    pub completed_at: SystemTime,

    /// Analysis execution duration
    pub execution_duration: Duration,

    /// Confidence score (0.0 to 1.0)
    pub confidence: f64,
}

/// Workflow error information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowError {
    /// Error type classification
    pub error_type: WorkflowErrorType,

    /// Human-readable error message
    pub message: String,

    /// Stage that caused the error (if applicable)
    pub failed_stage: Option<String>,

    /// Detailed error context
    pub context: HashMap<String, String>,

    /// Error occurrence time
    pub occurred_at: SystemTime,
}

/// Types of workflow errors.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum WorkflowErrorType {
    /// Stage execution timeout
    StageTimeout,

    /// Workflow execution timeout
    WorkflowTimeout,

    /// Collector communication failure
    CollectorError,

    /// Dependency resolution failure
    DependencyError,

    /// Configuration validation error
    ConfigurationError,

    /// Resource exhaustion
    ResourceError,

    /// Unknown error
    Unknown,
}

/// Workflow progress tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowProgress {
    /// Number of completed stages
    pub completed_stages: usize,

    /// Total number of stages
    pub total_stages: usize,

    /// Number of failed stages
    pub failed_stages: usize,

    /// Number of skipped stages
    pub skipped_stages: usize,

    /// Overall progress percentage (0.0 to 1.0)
    pub percentage: f64,
}

/// Statistics about workflow execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStatistics {
    /// Total workflows executed
    pub total_workflows: u64,

    /// Currently running workflows
    pub running_workflows: usize,

    /// Successfully completed workflows
    pub completed_workflows: u64,

    /// Failed workflows
    pub failed_workflows: u64,

    /// Cancelled workflows
    pub cancelled_workflows: u64,

    /// Timed out workflows
    pub timed_out_workflows: u64,

    /// Average workflow execution time
    pub avg_execution_time: Duration,

    /// Average stage execution time
    pub avg_stage_time: Duration,

    /// Most common failure reasons
    pub failure_reasons: HashMap<WorkflowErrorType, u64>,

    /// Last statistics update
    pub last_updated: SystemTime,
}

/// Analysis chain coordinator for managing multi-stage analysis workflows.
///
/// Coordinates complex analysis workflows across multiple collector types,
/// handling stage dependencies, execution order, result aggregation,
/// timeout management, and recovery logic.
pub struct AnalysisChainCoordinator {
    /// Configuration
    config: AnalysisChainConfig,

    /// Registered workflow definitions
    workflow_definitions: Arc<RwLock<HashMap<String, AnalysisWorkflowDefinition>>>,

    /// Active workflow executions
    active_executions: Arc<RwLock<HashMap<String, WorkflowExecution>>>,

    /// Completed workflow executions (for debugging)
    completed_executions: Arc<RwLock<VecDeque<WorkflowExecution>>>,

    /// Event bus for collector communication
    event_bus: Arc<RwLock<Option<Box<dyn EventBus + Send + Sync>>>>,

    /// Event subscription for receiving analysis results
    result_subscription: Arc<Mutex<Option<mpsc::UnboundedReceiver<BusEvent>>>>,

    /// Workflow execution statistics
    statistics: Arc<RwLock<WorkflowStatistics>>,

    /// Shutdown signal
    shutdown_signal: Arc<AtomicBool>,

    /// Background task handles
    task_handles: Arc<Mutex<Vec<JoinHandle<Result<()>>>>>,

    /// Workflow execution counter
    execution_counter: Arc<AtomicU64>,

    /// Stage execution tracking
    stage_tracker: Arc<RwLock<HashMap<String, StageExecution>>>,
}

/// Internal stage execution tracking.
#[derive(Debug, Clone)]
struct StageExecution {
    execution_id: String,
    stage_id: String,
    started_at: Instant,
    timeout: Duration,
    #[allow(dead_code)] // Reserved for future retry logic implementation
    retry_count: usize,
}

impl AnalysisChainCoordinator {
    /// Calculates workflow progress based on stage statuses.
    fn calculate_progress(
        stage_statuses: &std::collections::HashMap<String, StageStatus>,
        total_stages: usize,
    ) -> WorkflowProgress {
        let completed_stages = stage_statuses
            .values()
            .filter(|status| matches!(status, StageStatus::Completed))
            .count();
        // Count both failed and cancelled stages as failures, since cancelled stages
        // indicate workflows that didn't complete normally (typically due to dependency failures)
        let failed_stages = stage_statuses
            .values()
            .filter(|status| matches!(status, StageStatus::Failed | StageStatus::Cancelled))
            .count();
        let skipped_stages = stage_statuses
            .values()
            .filter(|status| matches!(status, StageStatus::Skipped))
            .count();

        let percentage = if total_stages > 0 {
            let completed_stages_f64 = completed_stages as f64;
            let total_stages_f64 = total_stages as f64;
            completed_stages_f64 / total_stages_f64
        } else {
            0.0
        };

        WorkflowProgress {
            completed_stages,
            total_stages,
            failed_stages,
            skipped_stages,
            percentage,
        }
    }

    /// Creates a new analysis chain coordinator with the specified configuration.
    ///
    /// Initializes all internal state including workflow definitions, execution tracking,
    /// and background task management. The coordinator starts in a stopped state and
    /// requires calling [`start`](Self::start) before executing workflows.
    ///
    /// Returns an `Arc<Self>` to enable shared ownership across async tasks.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration for workflow coordination behavior including timeouts,
    ///   concurrency limits, and monitoring intervals
    ///
    /// # Examples
    ///
    /// ```rust
    /// use collector_core::analysis_chain::{AnalysisChainCoordinator, AnalysisChainConfig};
    /// use std::time::Duration;
    ///
    /// let config = AnalysisChainConfig {
    ///     max_concurrent_workflows: 10,
    ///     default_stage_timeout: Duration::from_secs(300),
    ///     ..Default::default()
    /// };
    /// let coordinator = AnalysisChainCoordinator::new(config);
    /// ```
    pub fn new(config: AnalysisChainConfig) -> Arc<Self> {
        let statistics = WorkflowStatistics {
            total_workflows: 0,
            running_workflows: 0,
            completed_workflows: 0,
            failed_workflows: 0,
            cancelled_workflows: 0,
            timed_out_workflows: 0,
            avg_execution_time: Duration::from_secs(0),
            avg_stage_time: Duration::from_secs(0),
            failure_reasons: HashMap::new(),
            last_updated: SystemTime::now(),
        };

        Arc::new(Self {
            config,
            workflow_definitions: Arc::new(RwLock::new(HashMap::new())),
            active_executions: Arc::new(RwLock::new(HashMap::new())),
            completed_executions: Arc::new(RwLock::new(VecDeque::new())),
            event_bus: Arc::new(RwLock::new(None)),
            result_subscription: Arc::new(Mutex::new(None)),
            statistics: Arc::new(RwLock::new(statistics)),
            shutdown_signal: Arc::new(AtomicBool::new(false)),
            task_handles: Arc::new(Mutex::new(Vec::new())),
            execution_counter: Arc::new(AtomicU64::new(0)),
            stage_tracker: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Registers a workflow definition for execution.
    ///
    /// Validates the workflow definition and makes it available for execution.
    /// The workflow must have valid stage dependencies and collector references.
    ///
    /// # Arguments
    ///
    /// * `workflow` - Workflow definition to register
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Workflow validation fails
    /// - Circular dependencies are detected
    /// - Invalid collector references are found
    pub async fn register_workflow(&self, workflow: AnalysisWorkflowDefinition) -> Result<()> {
        info!(
            workflow_id = %workflow.workflow_id,
            name = %workflow.name,
            stages = workflow.stages.len(),
            "Registering analysis workflow definition"
        );

        // Validate workflow definition
        self.validate_workflow_definition(&workflow)?;

        // Check for circular dependencies
        self.validate_dependencies(&workflow)?;

        // Store workflow definition
        let mut definitions = self.workflow_definitions.write().await;
        definitions.insert(workflow.workflow_id.clone(), workflow);

        Ok(())
    }

    /// Validates a workflow definition for correctness.
    fn validate_workflow_definition(&self, workflow: &AnalysisWorkflowDefinition) -> Result<()> {
        // Validate workflow ID
        if workflow.workflow_id.is_empty() {
            anyhow::bail!("Workflow ID cannot be empty");
        }

        // Validate stages
        if workflow.stages.is_empty() {
            anyhow::bail!("Workflow must have at least one stage");
        }

        // Validate stage IDs are unique
        let stage_ids = self.validate_stage_ids(&workflow.stages)?;

        // Validate dependencies reference existing stages
        for (stage_id, deps) in &workflow.dependencies {
            if !stage_ids.contains(stage_id) {
                anyhow::bail!("Dependency references unknown stage: {}", stage_id);
            }

            for dep in deps {
                if !stage_ids.contains(dep) {
                    anyhow::bail!("Dependency references unknown stage: {}", dep);
                }
            }
        }

        Ok(())
    }

    /// Validates stage IDs for uniqueness and completeness.
    fn validate_stage_ids(&self, stages: &[AnalysisStage]) -> Result<HashSet<String>> {
        let mut stage_ids = HashSet::with_capacity(stages.len());

        for stage in stages {
            if stage.stage_id.is_empty() {
                anyhow::bail!("Stage ID cannot be empty");
            }

            if !stage_ids.insert(stage.stage_id.clone()) {
                anyhow::bail!("Duplicate stage ID: {}", stage.stage_id);
            }

            if stage.target_collector.is_empty() {
                anyhow::bail!("Stage target collector cannot be empty");
            }
        }

        Ok(stage_ids)
    }

    /// Validates workflow dependencies for circular references.
    fn validate_dependencies(&self, workflow: &AnalysisWorkflowDefinition) -> Result<()> {
        // Use topological sort to detect cycles
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut graph: HashMap<String, Vec<String>> = HashMap::new();

        // Initialize in-degree count and graph
        for stage in &workflow.stages {
            in_degree.insert(stage.stage_id.clone(), 0);
            graph.insert(stage.stage_id.clone(), Vec::new());
        }

        // Build dependency graph
        for (stage_id, deps) in &workflow.dependencies {
            for dep in deps {
                graph
                    .get_mut(dep)
                    .expect("Dependency should exist in graph")
                    .push(stage_id.clone());
                *in_degree
                    .get_mut(stage_id)
                    .expect("Stage should exist in in_degree map") += 1;
            }
        }

        // Topological sort
        let mut queue: VecDeque<String> = in_degree
            .iter()
            .filter(|&(_, &degree)| degree == 0)
            .map(|(id, _)| id.clone())
            .collect();

        let mut processed = 0;

        while let Some(stage_id) = queue.pop_front() {
            processed += 1;

            if let Some(dependents) = graph.get(&stage_id) {
                for dependent in dependents {
                    let degree = in_degree
                        .get_mut(dependent)
                        .expect("Dependent should exist in in_degree map");
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(dependent.clone());
                    }
                }
            }
        }

        if processed != workflow.stages.len() {
            anyhow::bail!("Circular dependency detected in workflow");
        }

        Ok(())
    }

    /// Sets the event bus for collector communication.
    ///
    /// This method must be called before executing workflows to enable
    /// communication with analysis collectors.
    ///
    /// # Arguments
    ///
    /// * `event_bus` - Event bus implementation for inter-collector communication
    pub async fn set_event_bus(&self, event_bus: Box<dyn EventBus + Send + Sync>) {
        let mut bus_guard = self.event_bus.write().await;
        *bus_guard = Some(event_bus);
        info!("Event bus configured for analysis chain coordination");
    }

    /// Starts the analysis chain coordinator background tasks.
    ///
    /// This method must be called before executing workflows. It starts
    /// the monitoring, result processing, and timeout management tasks.
    pub async fn start(self: &Arc<Self>) -> Result<()> {
        info!("Starting analysis chain coordinator");

        // Subscribe to analysis results
        self.subscribe_to_results().await?;

        // Start monitoring tasks
        self.start_workflow_monitoring().await?;
        self.start_result_processing().await?;
        self.start_timeout_management().await?;

        info!("Analysis chain coordinator started successfully");
        Ok(())
    }

    /// Subscribes to analysis results from the event bus.
    async fn subscribe_to_results(&self) -> Result<()> {
        let mut event_bus_guard = self.event_bus.write().await;
        let bus = event_bus_guard
            .as_mut()
            .with_context(|| "Event bus not configured for analysis chain coordination")?;

        // Create subscription for analysis results
        let subscription = EventSubscription {
            subscriber_id: "analysis-chain-coordinator".to_string(),
            capabilities: SourceCaps::all(), // Subscribe to all collector types
            event_filter: Some(EventFilter {
                event_types: vec!["analysis_result".to_string()],
                pids: vec![],
                min_priority: None,
                metadata_filters: HashMap::new(),
                topic_filters: Vec::new(),
                source_collectors: Vec::new(),
                eventbus_filters: None, // No daemoneye-eventbus specific filtering
            }),
            correlation_filter: None,
            topic_patterns: None,
            enable_wildcards: false,
            topic_filter: None,
        };

        let receiver = bus.subscribe(subscription).await?;

        let mut subscription_guard = self.result_subscription.lock().await;
        *subscription_guard = Some(receiver);

        Ok(())
    }

    /// Starts workflow monitoring background task.
    async fn start_workflow_monitoring(&self) -> Result<()> {
        let active_executions = Arc::clone(&self.active_executions);
        let statistics = Arc::clone(&self.statistics);
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        let monitoring_interval = self.config.status_monitoring_interval;
        let enable_debug = self.config.enable_debug_logging;

        let handle = tokio::spawn(async move {
            let mut interval_timer = interval(monitoring_interval);

            while !shutdown_signal.load(Ordering::Relaxed) {
                interval_timer.tick().await;

                let executions = active_executions.read().await;
                let execution_count = executions.len();

                if enable_debug {
                    debug!(
                        active_workflows = execution_count,
                        "Monitoring workflow executions"
                    );
                }

                // Update statistics
                {
                    let mut stats = statistics.write().await;
                    stats.running_workflows = execution_count;
                    stats.last_updated = SystemTime::now();
                }

                // Log status for long-running workflows
                for (execution_id, execution) in executions.iter() {
                    let elapsed = SystemTime::now()
                        .duration_since(execution.started_at)
                        .unwrap_or(Duration::from_secs(0));

                    if elapsed > Duration::from_secs(600) {
                        // 10 minutes
                        warn!(
                            execution_id = %execution_id,
                            workflow_id = %execution.workflow_definition.workflow_id,
                            elapsed_seconds = elapsed.as_secs(),
                            status = ?execution.status,
                            progress = execution.progress.percentage,
                            "Long-running workflow detected"
                        );
                    }
                }
            }

            Ok(())
        });

        let mut handles = self.task_handles.lock().await;
        handles.push(handle);
        Ok(())
    }

    /// Starts result processing background task.
    async fn start_result_processing(self: &Arc<Self>) -> Result<()> {
        let active_executions = Arc::clone(&self.active_executions);
        let completed_executions = Arc::clone(&self.completed_executions);
        let statistics = Arc::clone(&self.statistics);
        let stage_tracker = Arc::clone(&self.stage_tracker);
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        let max_completed = self.config.max_completed_workflows;
        let result_subscription = Arc::clone(&self.result_subscription);
        let coordinator = Arc::clone(self);

        let handle = tokio::spawn(async move {
            let mut check_interval = interval(Duration::from_secs(10));

            while !shutdown_signal.load(Ordering::Relaxed) {
                // Process events from the event bus subscription
                if let Some(receiver) = result_subscription.lock().await.as_mut() {
                    // Try to receive events with a short timeout to avoid blocking
                    match tokio::time::timeout(Duration::from_millis(100), receiver.recv()).await {
                        Ok(Some(bus_event)) => {
                            if let Err(e) = Self::process_result_event(
                                &coordinator,
                                &active_executions,
                                &completed_executions,
                                &statistics,
                                &stage_tracker,
                                &bus_event,
                                max_completed,
                            )
                            .await
                            {
                                warn!("Failed to process result event: {}", e);
                            }
                        }
                        Ok(None) => {
                            // Channel closed, break out of event processing
                            break;
                        }
                        Err(_) => {
                            // Timeout, continue to fallback interval processing
                        }
                    }
                }

                // Fallback: periodic reconciliation via interval tick
                check_interval.tick().await;
                if let Err(e) = Self::reconcile_completed_workflows(
                    &active_executions,
                    &completed_executions,
                    &statistics,
                    max_completed,
                )
                .await
                {
                    warn!("Failed to reconcile completed workflows: {}", e);
                }
            }

            Ok(())
        });

        let mut handles = self.task_handles.lock().await;
        handles.push(handle);
        Ok(())
    }

    /// Starts timeout management background task.
    async fn start_timeout_management(&self) -> Result<()> {
        let active_executions = Arc::clone(&self.active_executions);
        let stage_tracker = Arc::clone(&self.stage_tracker);
        let shutdown_signal = Arc::clone(&self.shutdown_signal);
        let max_workflow_timeout = self.config.max_workflow_timeout;

        let handle = tokio::spawn(async move {
            let mut timeout_check = interval(Duration::from_secs(30));

            while !shutdown_signal.load(Ordering::Relaxed) {
                timeout_check.tick().await;

                let now = SystemTime::now();

                // Check workflow timeouts
                {
                    let mut executions = active_executions.write().await;
                    for (execution_id, execution) in executions.iter_mut() {
                        let elapsed = now
                            .duration_since(execution.started_at)
                            .unwrap_or(Duration::from_secs(0));

                        let timeout = execution
                            .workflow_definition
                            .timeout
                            .unwrap_or(max_workflow_timeout);

                        if elapsed > timeout && execution.status == WorkflowStatus::Running {
                            warn!(
                                execution_id = %execution_id,
                                workflow_id = %execution.workflow_definition.workflow_id,
                                elapsed_seconds = elapsed.as_secs(),
                                timeout_seconds = timeout.as_secs(),
                                "Workflow execution timed out"
                            );

                            execution.status = WorkflowStatus::TimedOut;
                            execution.completed_at = Some(now);
                            execution.last_updated = now;
                            execution.error_info = Some(WorkflowError {
                                error_type: WorkflowErrorType::WorkflowTimeout,
                                message: format!(
                                    "Workflow timed out after {} seconds",
                                    elapsed.as_secs()
                                ),
                                failed_stage: None,
                                context: HashMap::new(),
                                occurred_at: now,
                            });
                        }
                    }
                }

                // Check stage timeouts
                // Collect timed-out stages while holding the lock, then drop it before any awaits
                let timed_out_stage_data = {
                    let mut tracker = stage_tracker.write().await;
                    let mut timed_out = Vec::new();

                    for (stage_key, stage_exec) in tracker.iter() {
                        if stage_exec.started_at.elapsed() > stage_exec.timeout {
                            timed_out.push((stage_key.clone(), stage_exec.clone()));
                        }
                    }

                    // Remove timed-out stages from tracker while we still hold the lock
                    for (stage_key, _) in &timed_out {
                        tracker.remove(stage_key);
                    }

                    timed_out
                };
                // stage_tracker lock is now dropped

                // Process timed-out stages with consistent lock ordering: active_executions first
                for (_stage_key, stage_exec) in timed_out_stage_data {
                    // Acquire active_executions lock (read first, then write if needed)
                    let executions_read = active_executions.read().await;
                    let should_timeout =
                        if let Some(execution) = executions_read.get(&stage_exec.execution_id) {
                            execution
                                .stage_statuses
                                .get(&stage_exec.stage_id)
                                .map(|status| matches!(status, StageStatus::Running))
                                .unwrap_or(true)
                        } else {
                            false
                        };
                    drop(executions_read);

                    if should_timeout {
                        warn!(
                            execution_id = %stage_exec.execution_id,
                            stage_id = %stage_exec.stage_id,
                            elapsed_seconds = stage_exec.started_at.elapsed().as_secs(),
                            timeout_seconds = stage_exec.timeout.as_secs(),
                            "Stage execution timed out"
                        );

                        // Update execution status with write lock
                        let mut executions = active_executions.write().await;
                        if let Some(execution) = executions.get_mut(&stage_exec.execution_id) {
                            execution
                                .stage_statuses
                                .insert(stage_exec.stage_id.clone(), StageStatus::TimedOut);
                            execution.last_updated = now;
                        }
                    } else {
                        // Stage was already completed/failed, don't override with timeout
                        debug!(
                            execution_id = %stage_exec.execution_id,
                            stage_id = %stage_exec.stage_id,
                            "Skipping timeout for already completed stage"
                        );
                    }
                }
            }

            Ok(())
        });

        let mut handles = self.task_handles.lock().await;
        handles.push(handle);
        Ok(())
    }

    /// Executes a workflow with the specified parameters.
    ///
    /// Creates a new workflow execution instance and begins coordinating
    /// the multi-stage analysis process according to the workflow definition.
    ///
    /// # Arguments
    ///
    /// * `workflow_id` - ID of the registered workflow to execute
    /// * `correlation_id` - Correlation ID for tracking related events
    /// * `context` - Execution context and parameters
    ///
    /// # Returns
    ///
    /// Returns the execution ID for tracking workflow progress.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Workflow definition is not found
    /// - Maximum concurrent workflows exceeded
    /// - Event bus is not configured
    #[instrument(skip(self), fields(workflow_id = %workflow_id, correlation_id = %correlation_id))]
    pub async fn execute_workflow(
        &self,
        workflow_id: &str,
        correlation_id: String,
        context: HashMap<String, String>,
    ) -> Result<String> {
        // Check concurrent workflow limit
        {
            let executions = self.active_executions.read().await;
            if executions.len() >= self.config.max_concurrent_workflows {
                anyhow::bail!(
                    "Maximum concurrent workflows ({}) exceeded",
                    self.config.max_concurrent_workflows
                );
            }
        }

        // Get workflow definition
        let workflow_definition = {
            let definitions = self.workflow_definitions.read().await;
            definitions
                .get(workflow_id)
                .cloned()
                .with_context(|| format!("Workflow definition not found: {}", workflow_id))?
        };

        // Create execution instance
        let execution_id = Uuid::new_v4().to_string();
        let now = SystemTime::now();

        let mut stage_statuses = HashMap::with_capacity(workflow_definition.stages.len());
        for stage in &workflow_definition.stages {
            stage_statuses.insert(stage.stage_id.clone(), StageStatus::Pending);
        }

        let progress = WorkflowProgress {
            completed_stages: 0,
            total_stages: workflow_definition.stages.len(),
            failed_stages: 0,
            skipped_stages: 0,
            percentage: 0.0,
        };

        let execution = WorkflowExecution {
            execution_id: execution_id.clone(),
            workflow_definition,
            status: WorkflowStatus::Queued,
            stage_statuses,
            stage_results: HashMap::new(),
            started_at: now,
            completed_at: None,
            last_updated: now,
            correlation_id,
            context,
            error_info: None,
            progress,
        };

        info!(
            execution_id = %execution_id,
            workflow_id = %workflow_id,
            stages = execution.workflow_definition.stages.len(),
            "Starting workflow execution"
        );

        // Store execution
        {
            let mut executions = self.active_executions.write().await;
            executions.insert(execution_id.clone(), execution);
        }

        // Update statistics
        {
            let mut stats = self.statistics.write().await;
            stats.total_workflows += 1;
            self.execution_counter.fetch_add(1, Ordering::Relaxed);
        }

        // Start workflow execution
        self.start_workflow_execution(&execution_id).await?;

        Ok(execution_id)
    }

    /// Starts the execution of a queued workflow.
    async fn start_workflow_execution(&self, execution_id: &str) -> Result<()> {
        // Update status to running
        {
            let mut executions = self.active_executions.write().await;
            if let Some(execution) = executions.get_mut(execution_id) {
                execution.status = WorkflowStatus::Running;
                execution.last_updated = SystemTime::now();
            }
        }

        // Start executing stages
        self.execute_ready_stages(execution_id).await?;

        Ok(())
    }

    /// Executes stages that are ready to run (dependencies satisfied).
    async fn execute_ready_stages(&self, execution_id: &str) -> Result<()> {
        let ready_stages = {
            let executions = self.active_executions.read().await;
            let execution = executions
                .get(execution_id)
                .with_context(|| format!("Workflow execution not found: {}", execution_id))?;

            self.find_ready_stages(execution)
        };

        for stage_id in ready_stages {
            self.execute_stage(execution_id, &stage_id).await?;
        }

        Ok(())
    }

    /// Finds stages that are ready to execute (all dependencies completed).
    fn find_ready_stages(&self, execution: &WorkflowExecution) -> Vec<String> {
        let mut ready_stages = Vec::new();

        for stage in &execution.workflow_definition.stages {
            let stage_status = execution
                .stage_statuses
                .get(&stage.stage_id)
                .unwrap_or(&StageStatus::Pending);

            if *stage_status != StageStatus::Pending {
                continue;
            }

            // Check if all dependencies are completed
            let dependencies = execution
                .workflow_definition
                .dependencies
                .get(&stage.stage_id)
                .map(|deps| deps.as_slice())
                .unwrap_or(&[]);

            let dependencies_satisfied = dependencies.iter().all(|dep_id| {
                matches!(
                    execution.stage_statuses.get(dep_id),
                    Some(StageStatus::Completed)
                )
            });

            if dependencies_satisfied {
                ready_stages.push(stage.stage_id.clone());
            }
        }

        ready_stages
    }

    /// Executes a specific stage of the workflow.
    async fn execute_stage(&self, execution_id: &str, stage_id: &str) -> Result<()> {
        let (stage, timeout) = {
            let executions = self.active_executions.read().await;
            let execution = executions
                .get(execution_id)
                .with_context(|| format!("Workflow execution not found: {}", execution_id))?;

            let stage = execution
                .workflow_definition
                .stages
                .iter()
                .find(|s| s.stage_id == stage_id)
                .with_context(|| format!("Stage not found in workflow: {}", stage_id))?
                .clone();

            let timeout = stage.timeout.unwrap_or(self.config.default_stage_timeout);

            (stage, timeout)
        };

        info!(
            execution_id = %execution_id,
            stage_id = %stage_id,
            target_collector = %stage.target_collector,
            analysis_type = ?stage.analysis_type,
            "Executing workflow stage"
        );

        // Update stage status to running
        {
            let mut executions = self.active_executions.write().await;
            if let Some(execution) = executions.get_mut(execution_id) {
                execution
                    .stage_statuses
                    .insert(stage_id.to_string(), StageStatus::Running);
                execution.last_updated = SystemTime::now();
            }
        }

        // Track stage execution for timeout management
        {
            let mut tracker = self.stage_tracker.write().await;
            let stage_key = format!("{}:{}", execution_id, stage_id);
            tracker.insert(
                stage_key,
                StageExecution {
                    execution_id: execution_id.to_string(),
                    stage_id: stage_id.to_string(),
                    started_at: Instant::now(),
                    timeout,
                    retry_count: 0,
                },
            );
        }

        // Create trigger request for the analysis collector
        let trigger_request = TriggerRequest {
            trigger_id: Uuid::new_v4().to_string(),
            target_collector: stage.target_collector.clone(),
            analysis_type: stage.analysis_type.clone(),
            priority: stage.priority.clone(),
            target_pid: None,  // Would be populated from context
            target_path: None, // Would be populated from context
            correlation_id: format!("{}:{}", execution_id, stage_id),
            metadata: stage.config.clone(),
            timestamp: SystemTime::now(),
        };

        // Send trigger request via event bus
        self.send_trigger_request(trigger_request).await?;

        Ok(())
    }

    /// Sends a trigger request to an analysis collector via the event bus.
    async fn send_trigger_request(&self, trigger: TriggerRequest) -> Result<()> {
        let mut event_bus_guard = self.event_bus.write().await;
        let bus = event_bus_guard
            .as_mut()
            .with_context(|| "Event bus not configured for trigger request emission")?;

        let event = CollectionEvent::TriggerRequest(trigger.clone());
        let correlation_metadata =
            crate::event_bus::CorrelationMetadata::new(trigger.correlation_id.clone())
                .with_tag("trigger_type".to_string(), "analysis_chain".to_string());
        bus.publish(event, correlation_metadata)
            .await
            .context("Failed to publish trigger request")?;

        debug!(
            trigger_id = %trigger.trigger_id,
            target_collector = %trigger.target_collector,
            correlation_id = %trigger.correlation_id,
            "Sent trigger request to analysis collector"
        );

        Ok(())
    }

    /// Returns the current status of a workflow execution.
    ///
    /// # Arguments
    ///
    /// * `execution_id` - ID of the workflow execution to query
    ///
    /// # Returns
    ///
    /// Returns the workflow execution if found, None otherwise.
    pub async fn get_workflow_status(&self, execution_id: &str) -> Option<WorkflowExecution> {
        let executions = self.active_executions.read().await;
        executions.get(execution_id).cloned()
    }

    /// Returns statistics about workflow execution.
    pub async fn get_statistics(&self) -> WorkflowStatistics {
        let stats = self.statistics.read().await;
        stats.clone()
    }

    /// Cancels a running workflow execution.
    ///
    /// # Arguments
    ///
    /// * `execution_id` - ID of the workflow execution to cancel
    ///
    /// # Returns
    ///
    /// Returns true if the workflow was cancelled, false if not found or already completed.
    pub async fn cancel_workflow(&self, execution_id: &str) -> bool {
        let mut executions = self.active_executions.write().await;
        if let Some(execution) = executions.get_mut(execution_id) {
            if execution.status == WorkflowStatus::Running {
                execution.status = WorkflowStatus::Cancelled;
                execution.completed_at = Some(SystemTime::now());
                execution.last_updated = SystemTime::now();

                info!(
                    execution_id = %execution_id,
                    workflow_id = %execution.workflow_definition.workflow_id,
                    "Workflow execution cancelled"
                );

                return true;
            }
        }
        false
    }

    /// Initiates graceful shutdown of the coordinator.
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down analysis chain coordinator");

        // Signal shutdown
        self.shutdown_signal.store(true, Ordering::Relaxed);

        // Wait for background tasks to complete
        let mut handles = self.task_handles.lock().await;
        let task_handles = std::mem::take(&mut *handles);

        for handle in task_handles {
            if let Err(e) = handle.await {
                warn!(error = %e, "Background task failed during shutdown");
            }
        }

        info!("Analysis chain coordinator shutdown complete");
        Ok(())
    }

    /// Processes a result event from the event bus and updates stage statuses.
    async fn process_result_event(
        coordinator: &Arc<AnalysisChainCoordinator>,
        active_executions: &Arc<RwLock<HashMap<String, WorkflowExecution>>>,
        completed_executions: &Arc<RwLock<VecDeque<WorkflowExecution>>>,
        statistics: &Arc<RwLock<WorkflowStatistics>>,
        stage_tracker: &Arc<RwLock<HashMap<String, StageExecution>>>,
        bus_event: &BusEvent,
        max_completed: usize,
    ) -> Result<()> {
        // Extract execution and stage IDs from the event's routing metadata
        let execution_id = bus_event.routing_metadata.get("execution_id");
        let stage_id = bus_event.routing_metadata.get("stage_id");

        if let (Some(execution_id), Some(stage_id)) = (execution_id, stage_id) {
            let mut executions = active_executions.write().await;

            if let Some(execution) = executions.get_mut(execution_id) {
                // Update the specific stage status based on the event
                let new_stage_status = match &bus_event.event {
                    CollectionEvent::TriggerRequest(trigger_request) => {
                        // Check if this is a result event (success/failure)
                        if trigger_request.metadata.contains_key("result_status") {
                            match trigger_request
                                .metadata
                                .get("result_status")
                                .expect("result_status should be present when contains_key returns true")
                                .as_str()
                            {
                                "success" => StageStatus::Completed,
                                "failed" => StageStatus::Failed,
                                "cancelled" => StageStatus::Cancelled,
                                "timeout" => StageStatus::TimedOut,
                                _ => StageStatus::Failed,
                            }
                        } else {
                            // Default to completed if no specific status
                            StageStatus::Completed
                        }
                    }
                    _ => {
                        // For other event types, assume success
                        StageStatus::Completed
                    }
                };

                // Update the stage status
                execution
                    .stage_statuses
                    .insert(stage_id.clone(), new_stage_status.clone());
                execution.last_updated = SystemTime::now();

                // Remove finished stages from stage_tracker to prevent timeout sweep issues
                if matches!(
                    new_stage_status,
                    StageStatus::Completed
                        | StageStatus::Failed
                        | StageStatus::Cancelled
                        | StageStatus::TimedOut
                ) {
                    let stage_key = format!("{}:{}", execution_id, stage_id);
                    let mut tracker = stage_tracker.write().await;
                    tracker.remove(&stage_key);
                }

                // If this stage failed/cancelled/timedout, mark dependent stages appropriately
                let is_failing_terminal_state = matches!(
                    new_stage_status,
                    StageStatus::Failed | StageStatus::Cancelled | StageStatus::TimedOut
                );

                if is_failing_terminal_state {
                    // Find all stages that depend on this failed stage
                    let dependent_stage_ids: Vec<String> = execution
                        .workflow_definition
                        .dependencies
                        .iter()
                        .filter(|(_, deps)| deps.contains(stage_id))
                        .map(|(dependent_stage_id, _)| dependent_stage_id.clone())
                        .collect();

                    // Mark dependent stages as Skipped or Failed based on whether they're optional
                    for dependent_stage_id in dependent_stage_ids {
                        let current_status = execution
                            .stage_statuses
                            .get(&dependent_stage_id)
                            .cloned()
                            .unwrap_or(StageStatus::Pending);

                        // Only update if still pending (don't override already-processed stages)
                        if current_status == StageStatus::Pending {
                            // Find the stage definition to check if it's optional
                            let is_optional = execution
                                .workflow_definition
                                .stages
                                .iter()
                                .find(|s| s.stage_id == dependent_stage_id)
                                .map(|s| s.optional)
                                .unwrap_or(false);

                            let new_status = if is_optional {
                                StageStatus::Skipped
                            } else {
                                StageStatus::Failed
                            };

                            execution
                                .stage_statuses
                                .insert(dependent_stage_id.clone(), new_status);

                            // Also remove from stage_tracker if it was queued
                            let dep_stage_key = format!("{}:{}", execution_id, dependent_stage_id);
                            let mut tracker = stage_tracker.write().await;
                            tracker.remove(&dep_stage_key);
                        }
                    }
                }

                // Recalculate progress when stage status changes
                let total_stages = execution.workflow_definition.stages.len();
                execution.progress =
                    Self::calculate_progress(&execution.stage_statuses, total_stages);

                // Release the write lock before calling execute_ready_stages
                let exec_id_for_scheduling = execution_id.clone();
                drop(executions);

                // Trigger scheduler to run newly-ready stages
                // This ensures the workflow continues to progress
                if let Err(e) = coordinator
                    .execute_ready_stages(&exec_id_for_scheduling)
                    .await
                {
                    warn!(
                        execution_id = %exec_id_for_scheduling,
                        error = %e,
                        "Failed to execute ready stages after status update"
                    );
                }

                // Re-acquire lock to check completion status
                let mut executions = active_executions.write().await;
                let Some(execution) = executions.get_mut(execution_id) else {
                    // Execution was removed (possibly completed), nothing more to do
                    return Ok(());
                };

                // Check if all stages are now complete
                let all_stages_complete =
                    execution.workflow_definition.stages.iter().all(|stage| {
                        execution
                            .stage_statuses
                            .get(&stage.stage_id)
                            .map(|status| {
                                matches!(
                                    status,
                                    StageStatus::Completed
                                        | StageStatus::Failed
                                        | StageStatus::Cancelled
                                        | StageStatus::TimedOut
                                )
                            })
                            .unwrap_or(false)
                    });

                if all_stages_complete {
                    // Determine overall workflow status
                    let has_failures = execution.stage_statuses.values().any(|status| {
                        matches!(status, StageStatus::Failed | StageStatus::TimedOut)
                    });

                    let has_cancellations = execution
                        .stage_statuses
                        .values()
                        .any(|status| matches!(status, StageStatus::Cancelled));

                    execution.status = if has_cancellations {
                        WorkflowStatus::Cancelled
                    } else if has_failures {
                        WorkflowStatus::Failed
                    } else {
                        WorkflowStatus::Completed
                    };

                    execution.completed_at = Some(SystemTime::now());

                    // Final progress calculation when all stages complete
                    let total_stages = execution.workflow_definition.stages.len();
                    execution.progress =
                        Self::calculate_progress(&execution.stage_statuses, total_stages);

                    // Move to completed executions
                    let completed_execution = executions
                        .remove(execution_id)
                        .expect("Execution should exist when processing completion");
                    Self::move_to_completed_executions(
                        completed_executions,
                        statistics,
                        completed_execution,
                        max_completed,
                    )
                    .await;
                }
            }
        }

        Ok(())
    }

    /// Reconciles completed workflows (fallback method for state consistency).
    async fn reconcile_completed_workflows(
        active_executions: &Arc<RwLock<HashMap<String, WorkflowExecution>>>,
        completed_executions: &Arc<RwLock<VecDeque<WorkflowExecution>>>,
        statistics: &Arc<RwLock<WorkflowStatistics>>,
        max_completed: usize,
    ) -> Result<()> {
        let mut executions = active_executions.write().await;
        let mut to_remove = Vec::new();

        for (execution_id, execution) in executions.iter() {
            if matches!(
                execution.status,
                WorkflowStatus::Completed
                    | WorkflowStatus::Failed
                    | WorkflowStatus::Cancelled
                    | WorkflowStatus::TimedOut
            ) {
                to_remove.push(execution_id.clone());
            }
        }

        for execution_id in to_remove {
            if let Some(execution) = executions.remove(&execution_id) {
                Self::move_to_completed_executions(
                    completed_executions,
                    statistics,
                    execution,
                    max_completed,
                )
                .await;
            }
        }

        Ok(())
    }

    /// Moves a completed execution to the completed executions list and updates statistics.
    async fn move_to_completed_executions(
        completed_executions: &Arc<RwLock<VecDeque<WorkflowExecution>>>,
        statistics: &Arc<RwLock<WorkflowStatistics>>,
        execution: WorkflowExecution,
        max_completed: usize,
    ) {
        // Update statistics
        {
            let mut stats = statistics.write().await;
            match execution.status {
                WorkflowStatus::Completed => {
                    stats.completed_workflows += 1;
                }
                WorkflowStatus::Failed => {
                    stats.failed_workflows += 1;
                    if let Some(error) = &execution.error_info {
                        *stats
                            .failure_reasons
                            .entry(error.error_type.clone())
                            .or_insert(0) += 1;
                    }
                }
                WorkflowStatus::Cancelled => {
                    stats.cancelled_workflows += 1;
                }
                WorkflowStatus::TimedOut => {
                    stats.timed_out_workflows += 1;
                }
                _ => {}
            }
        }

        // Add to completed list
        {
            let mut completed = completed_executions.write().await;
            completed.push_back(execution);

            // Maintain size limit
            while completed.len() > max_completed {
                completed.pop_front();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workflow_definition_validation() {
        let config = AnalysisChainConfig::default();
        let coordinator = AnalysisChainCoordinator::new(config);

        // Valid workflow
        let valid_workflow = AnalysisWorkflowDefinition {
            workflow_id: "test-workflow".to_string(),
            name: "Test Workflow".to_string(),
            description: "Test workflow description".to_string(),
            version: "1.0".to_string(),
            stages: vec![
                AnalysisStage {
                    stage_id: "stage1".to_string(),
                    name: "Stage 1".to_string(),
                    target_collector: "collector1".to_string(),
                    analysis_type: AnalysisType::BinaryHash,
                    timeout: None,
                    priority: TriggerPriority::Normal,
                    optional: false,
                    config: HashMap::new(),
                    input_requirements: vec![],
                    output_data: vec!["hash".to_string()],
                },
                AnalysisStage {
                    stage_id: "stage2".to_string(),
                    name: "Stage 2".to_string(),
                    target_collector: "collector2".to_string(),
                    analysis_type: AnalysisType::MemoryAnalysis,
                    timeout: None,
                    priority: TriggerPriority::Normal,
                    optional: false,
                    config: HashMap::new(),
                    input_requirements: vec!["hash".to_string()],
                    output_data: vec!["memory_dump".to_string()],
                },
            ],
            dependencies: {
                let mut deps = HashMap::new();
                deps.insert("stage2".to_string(), vec!["stage1".to_string()]);
                deps
            },
            timeout: None,
            priority: TriggerPriority::Normal,
            metadata: HashMap::new(),
        };

        assert!(
            coordinator
                .validate_workflow_definition(&valid_workflow)
                .is_ok()
        );

        // Invalid workflow - empty ID
        let mut invalid_workflow = valid_workflow.clone();
        invalid_workflow.workflow_id = "".to_string();
        assert!(
            coordinator
                .validate_workflow_definition(&invalid_workflow)
                .is_err()
        );

        // Invalid workflow - no stages
        let mut invalid_workflow = valid_workflow.clone();
        invalid_workflow.stages.clear();
        assert!(
            coordinator
                .validate_workflow_definition(&invalid_workflow)
                .is_err()
        );

        // Invalid workflow - duplicate stage IDs
        let mut invalid_workflow = valid_workflow.clone();
        invalid_workflow.stages[1].stage_id = "stage1".to_string();
        assert!(
            coordinator
                .validate_workflow_definition(&invalid_workflow)
                .is_err()
        );
    }

    #[test]
    fn test_circular_dependency_detection() {
        let config = AnalysisChainConfig::default();
        let coordinator = AnalysisChainCoordinator::new(config);

        // Workflow with circular dependency
        let circular_workflow = AnalysisWorkflowDefinition {
            workflow_id: "circular-workflow".to_string(),
            name: "Circular Workflow".to_string(),
            description: "Workflow with circular dependencies".to_string(),
            version: "1.0".to_string(),
            stages: vec![
                AnalysisStage {
                    stage_id: "stage1".to_string(),
                    name: "Stage 1".to_string(),
                    target_collector: "collector1".to_string(),
                    analysis_type: AnalysisType::BinaryHash,
                    timeout: None,
                    priority: TriggerPriority::Normal,
                    optional: false,
                    config: HashMap::new(),
                    input_requirements: vec![],
                    output_data: vec!["hash".to_string()],
                },
                AnalysisStage {
                    stage_id: "stage2".to_string(),
                    name: "Stage 2".to_string(),
                    target_collector: "collector2".to_string(),
                    analysis_type: AnalysisType::MemoryAnalysis,
                    timeout: None,
                    priority: TriggerPriority::Normal,
                    optional: false,
                    config: HashMap::new(),
                    input_requirements: vec!["hash".to_string()],
                    output_data: vec!["memory_dump".to_string()],
                },
            ],
            dependencies: {
                let mut deps = HashMap::new();
                deps.insert("stage1".to_string(), vec!["stage2".to_string()]);
                deps.insert("stage2".to_string(), vec!["stage1".to_string()]);
                deps
            },
            timeout: None,
            priority: TriggerPriority::Normal,
            metadata: HashMap::new(),
        };

        assert!(
            coordinator
                .validate_dependencies(&circular_workflow)
                .is_err()
        );
    }

    #[test]
    fn test_workflow_progress_calculation() {
        let progress = WorkflowProgress {
            completed_stages: 3,
            total_stages: 5,
            failed_stages: 1,
            skipped_stages: 0,
            percentage: 3.0 / 5.0,
        };

        assert_eq!(progress.completed_stages, 3);
        assert_eq!(progress.total_stages, 5);
        assert_eq!(progress.failed_stages, 1);
        assert_eq!(progress.percentage, 0.6);
    }

    #[tokio::test]
    async fn test_coordinator_creation() {
        let config = AnalysisChainConfig::default();
        let coordinator = AnalysisChainCoordinator::new(config);

        let stats = coordinator.get_statistics().await;
        assert_eq!(stats.total_workflows, 0);
        assert_eq!(stats.running_workflows, 0);
        assert_eq!(stats.completed_workflows, 0);
    }

    #[tokio::test]
    async fn test_workflow_registration() {
        let config = AnalysisChainConfig::default();
        let coordinator = AnalysisChainCoordinator::new(config);

        let workflow = AnalysisWorkflowDefinition {
            workflow_id: "test-workflow".to_string(),
            name: "Test Workflow".to_string(),
            description: "Test workflow description".to_string(),
            version: "1.0".to_string(),
            stages: vec![AnalysisStage {
                stage_id: "stage1".to_string(),
                name: "Stage 1".to_string(),
                target_collector: "collector1".to_string(),
                analysis_type: AnalysisType::BinaryHash,
                timeout: None,
                priority: TriggerPriority::Normal,
                optional: false,
                config: HashMap::new(),
                input_requirements: vec![],
                output_data: vec!["hash".to_string()],
            }],
            dependencies: HashMap::new(),
            timeout: None,
            priority: TriggerPriority::Normal,
            metadata: HashMap::new(),
        };

        assert!(coordinator.register_workflow(workflow).await.is_ok());
    }
}
