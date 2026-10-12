//! Builds sinks from resolved configuration.
//!
//! Stdout and file sinks live here; network sinks are supplied by a registered [`SinkBuilder`].

use super::config::{ResolvedSink, SinkSpec};
use super::{AlertSink, AlertingError, FileSink, StdoutSink};
use crate::config::DeliveryConfig;

/// Builds the network sinks (webhook, syslog, email) the factory does not know.
pub trait SinkBuilder: Send + Sync {
    /// Build the sink for `resolved`.
    ///
    /// # Errors
    ///
    /// [`AlertingError::ConfigurationError`] when the spec cannot be built.
    fn build(
        &self,
        resolved: &ResolvedSink,
        delivery: &DeliveryConfig,
    ) -> Result<Box<dyn AlertSink>, AlertingError>;
}

/// Sink construction from [`ResolvedSink`]s.
pub struct AlertSinkFactory;

impl AlertSinkFactory {
    /// Build one sink; `network` handles every spec that leaves the host.
    ///
    /// # Errors
    ///
    /// [`AlertingError::ConfigurationError`] for a network spec with no `network` builder, or
    /// whatever the builder returns.
    #[allow(
        clippy::wildcard_enum_match_arm,
        reason = "SinkSpec is non_exhaustive; every spec but stdout and file leaves the host"
    )]
    pub fn build(
        resolved: &ResolvedSink,
        delivery: &DeliveryConfig,
        network: Option<&dyn SinkBuilder>,
    ) -> Result<Box<dyn AlertSink>, AlertingError> {
        match resolved.spec {
            SinkSpec::Stdout(ref spec) => Ok(Box::new(StdoutSink::new(
                spec.name.clone(),
                spec.format.clone(),
            ))),
            SinkSpec::File(ref spec) => Ok(Box::new(FileSink::new(
                spec.name.clone(),
                spec.path.clone(),
                spec.format.clone(),
            ))),
            _ => network.map_or_else(
                || {
                    Err(AlertingError::ConfigurationError(format!(
                        "no builder registered for network sink {:?}",
                        resolved.spec.name()
                    )))
                },
                |builder| builder.build(resolved, delivery),
            ),
        }
    }
}
