//! The locked-down `DataFusion` session: pooled memory, no spill, allowlisted functions only
//! (R4, R6, R7; KTD3, KTD4).

use std::sync::Arc;

use datafusion::error::Result;
use datafusion::execution::context::SessionState;
use datafusion::execution::disk_manager::{DiskManagerBuilder, DiskManagerMode};
use datafusion::execution::memory_pool::GreedyMemoryPool;
use datafusion::execution::runtime_env::{RuntimeEnv, RuntimeEnvBuilder};
use datafusion::execution::session_state::SessionStateBuilder;
use datafusion::prelude::SessionConfig;

use crate::detection::execution::functions::allowlisted_udfs;
use crate::detection::regex_cache::RegexCache;
use crate::detection_bounds::{
    EXECUTOR_BATCH_SIZE, EXECUTOR_MEMORY_POOL_BYTES, EXECUTOR_TARGET_PARTITIONS,
};

pub use crate::detection::execution::regexp::LatencySink;

/// The executor's one `RuntimeEnv`: a bounded `GreedyMemoryPool` and a disabled disk manager.
///
/// Greedy rather than fair because a fair pool divides memory across concurrent spillable
/// operators and this engine plans none. The disk manager is disabled so that a spill is an error
/// rather than a temp file: a security daemon must not scatter files, and a request for one fails
/// as a resource error that the caller turns into a degraded reason.
#[derive(Debug, Clone)]
pub struct ExecutorRuntime {
    env: Arc<RuntimeEnv>,
}

impl ExecutorRuntime {
    /// A runtime with the default pool capacity, [`EXECUTOR_MEMORY_POOL_BYTES`].
    ///
    /// # Errors
    ///
    /// Returns the `DataFusion` error if the runtime cannot be built.
    pub fn new() -> Result<Self> {
        Self::with_pool_bytes(EXECUTOR_MEMORY_POOL_BYTES)
    }

    /// A runtime whose pool holds `pool_bytes`.
    ///
    /// # Errors
    ///
    /// Returns the `DataFusion` error if the runtime cannot be built.
    pub fn with_pool_bytes(pool_bytes: usize) -> Result<Self> {
        let env = RuntimeEnvBuilder::new()
            .with_memory_pool(Arc::new(GreedyMemoryPool::new(pool_bytes)))
            .with_disk_manager_builder(
                DiskManagerBuilder::default().with_mode(DiskManagerMode::Disabled),
            )
            .build_arc()?;
        Ok(Self { env })
    }

    /// The shared `RuntimeEnv`, for [`session_state`].
    pub fn env(&self) -> Arc<RuntimeEnv> {
        Arc::clone(&self.env)
    }
}

/// A session at the default partition count and batch size.
///
/// # Errors
///
/// Returns the `DataFusion` error if the state cannot be built.
pub fn session_state(
    runtime: Arc<RuntimeEnv>,
    sink: Arc<LatencySink>,
    cache: Arc<RegexCache>,
) -> Result<SessionState> {
    session_state_sized(
        runtime,
        sink,
        cache,
        EXECUTOR_TARGET_PARTITIONS,
        EXECUTOR_BATCH_SIZE,
    )
}

/// A session whose only callable functions are the allowlist.
///
/// Built with no `with_default_features()`: the function registry starts empty and receives
/// [`allowlisted_udfs`], so there is nothing to subtract and nothing to forget.
///
/// # Errors
///
/// Returns the `DataFusion` error if the state cannot be built.
pub fn session_state_sized(
    runtime: Arc<RuntimeEnv>,
    sink: Arc<LatencySink>,
    cache: Arc<RegexCache>,
    target_partitions: usize,
    batch_size: usize,
) -> Result<SessionState> {
    let config = SessionConfig::new()
        .with_target_partitions(target_partitions)
        .with_batch_size(batch_size);
    Ok(SessionStateBuilder::new()
        .with_config(config)
        .with_runtime_env(runtime)
        .with_scalar_functions(allowlisted_udfs(cache, sink))
        .build())
}
