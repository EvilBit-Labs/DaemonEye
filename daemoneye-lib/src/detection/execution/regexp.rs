//! The `regexp` / `match` SQL function and the sink that records its per-batch latency (R7).
//!
//! `regexp(value, pattern)` is the one allowlisted function whose cost depends on the rule author's
//! input, so it is the measurement site ADR-0011 builds the latency guard on: each invocation is
//! one `RecordBatch`, is timed as a whole, and is recorded against its pattern text.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use datafusion::arrow::array::{ArrayRef, BooleanArray};
use datafusion::arrow::datatypes::DataType;
use datafusion::common::cast::as_string_array;
use datafusion::common::{DataFusionError, Result, ScalarValue};
use datafusion::logical_expr::{ColumnarValue, ScalarFunctionArgs, ScalarUDFImpl, Signature};

use crate::detection::execution::functions::{as_utf8, string_signature};
use crate::detection::regex_cache::RegexCache;

/// Worst observed batch latency per pattern since the last [`drain`](Self::drain).
///
/// Keyed by the pattern text, so `regexp` and its alias `match` share an entry. The mutex is
/// synchronous and never held across an `.await`: a UDF's `invoke_with_args` is synchronous, and
/// every method here takes the guard, finishes, and drops it.
///
/// A sink built [`with_threshold`](Self::with_threshold) also latches a breach: once any recorded
/// batch exceeds the threshold, [`RegexpUdf`] refuses its next invocation with a [`LatencyAbort`].
/// That is ADR-0011's "bounded by one batch" delivered at the scan-batch boundary, where the UDF
/// is called once per scan batch. The in-flight batch is always finished, because `regex` has no
/// cancellation and abandoning a match would keep burning the CPU the budget protects; the next
/// batch is what is refused. A sink from [`Default`] has no threshold and never latches.
#[derive(Debug, Default)]
pub struct LatencySink {
    per_pattern: Mutex<BTreeMap<String, Duration>>,
    threshold: Option<Duration>,
    is_breached: AtomicBool,
}

/// The error `RegexpUdf` returns once its sink has latched a breach.
///
/// Carried as a `DataFusionError::External` and recognised by type, not by message text, through
/// [`is_in`](Self::is_in), so the executor can tell a latency stop from a genuine failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("regexp latency threshold breached; no further batches are evaluated")]
pub struct LatencyAbort;

impl LatencyAbort {
    /// Whether `error`, or the error it wraps, is a [`LatencyAbort`].
    pub fn is_in(error: &DataFusionError) -> bool {
        matches!(error.find_root(), DataFusionError::External(inner) if inner.is::<Self>())
    }
}

impl LatencySink {
    /// A sink that latches a breach when a recorded batch takes longer than `threshold`.
    pub fn with_threshold(threshold: Duration) -> Self {
        Self {
            threshold: Some(threshold),
            ..Self::default()
        }
    }

    /// Whether a recorded batch has exceeded the threshold. Never reset by [`drain`](Self::drain).
    pub fn is_breached(&self) -> bool {
        self.is_breached.load(Ordering::Relaxed)
    }

    /// Record one batch's `elapsed` time for `pattern`, keeping the larger of old and new.
    pub fn record(&self, pattern: &str, elapsed: Duration) {
        if self.threshold.is_some_and(|limit| elapsed > limit) {
            self.is_breached.store(true, Ordering::Relaxed);
        }
        let mut guard = self
            .per_pattern
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard
            .entry(pattern.to_owned())
            .and_modify(|worst| *worst = (*worst).max(elapsed))
            .or_insert(elapsed);
    }

    /// Take everything recorded so far, leaving the sink empty.
    pub fn drain(&self) -> BTreeMap<String, Duration> {
        let mut guard = self
            .per_pattern
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut *guard)
    }
}

/// `regexp(value, pattern)`, also callable as `match`.
///
/// Equality and hashing are by the identity of the cache and sink it was built with, so two
/// instances over different cache or sink are distinct functions to `DataFusion`.
#[derive(Debug)]
pub struct RegexpUdf {
    signature: Signature,
    aliases: Vec<String>,
    cache: Arc<RegexCache>,
    sink: Arc<LatencySink>,
}

impl RegexpUdf {
    /// Build the function over the shared compile cache and latency sink.
    pub fn new(cache: Arc<RegexCache>, sink: Arc<LatencySink>) -> Self {
        Self {
            signature: string_signature(2),
            aliases: vec!["match".to_owned()],
            cache,
            sink,
        }
    }
}

impl PartialEq for RegexpUdf {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.cache, &other.cache) && Arc::ptr_eq(&self.sink, &other.sink)
    }
}

impl Eq for RegexpUdf {}

impl Hash for RegexpUdf {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.cache).hash(state);
        Arc::as_ptr(&self.sink).hash(state);
    }
}

impl ScalarUDFImpl for RegexpUdf {
    #[allow(clippy::unnecessary_literal_bound)] // the trait fixes this signature
    fn name(&self) -> &str {
        "regexp"
    }

    fn aliases(&self) -> &[String] {
        &self.aliases
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _arg_types: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        if self.sink.is_breached() {
            return Err(DataFusionError::External(Box::new(LatencyAbort)));
        }
        // Timed from here, so a cold compile in the batch that first sees a pattern counts.
        let started = Instant::now();
        let (Some(value), Some(pattern)) = (args.args.first(), args.args.get(1)) else {
            return Err(DataFusionError::Internal(
                "regexp expects exactly two arguments".to_owned(),
            ));
        };
        let literal = match *pattern {
            ColumnarValue::Scalar(ref scalar) => scalar,
            ColumnarValue::Array(_) => {
                return Err(DataFusionError::Execution(
                    "regexp requires a literal pattern".to_owned(),
                ));
            }
        };
        let pattern_text = match literal.try_as_str() {
            Some(Some(text)) => text,
            // A NULL pattern matches nothing: every row is NULL, as for any NULL operand.
            Some(None) => return Ok(ColumnarValue::Scalar(ScalarValue::Boolean(None))),
            None if literal.is_null() => {
                return Ok(ColumnarValue::Scalar(ScalarValue::Boolean(None)));
            }
            None => {
                return Err(DataFusionError::Execution(
                    "regexp requires a literal string pattern".to_owned(),
                ));
            }
        };
        let regex = self
            .cache
            .get_or_compile(pattern_text)
            .map_err(|rejection| {
                DataFusionError::Execution(format!("regexp: pattern rejected: {rejection}"))
            })?;
        let values: ArrayRef = as_utf8(&value.to_array(args.number_rows)?)?;
        let matches: BooleanArray = as_string_array(&values)?
            .iter()
            .map(|cell| cell.map(|text| regex.is_match(text)))
            .collect();
        self.sink.record(pattern_text, started.elapsed());
        Ok(ColumnarValue::Array(Arc::new(matches)))
    }
}
