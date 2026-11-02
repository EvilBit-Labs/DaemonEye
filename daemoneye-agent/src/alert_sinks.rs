//! Builds the agent's alert sinks from `alerting.sinks`.

use anyhow::Context;
use daemoneye_lib::alerting::{
    AlertSink, AlertSinkFactory, CredentialResolver, OutputFormat, StdoutSink,
};
use daemoneye_lib::config::AlertingConfig;
use daemoneye_sinks::NetworkSinkBuilder;

/// Name of the sink used when the configuration lists none.
pub const DEFAULT_SINK_NAME: &str = "stdout";

/// Build every enabled sink, or the default JSON stdout sink when `config.sinks` is empty.
///
/// # Errors
///
/// A sink that does not parse, whose credential variable is unset or empty (the error names the
/// sink and the variable, never the value), or that the factory refuses to build.
pub fn build_sinks(
    config: &AlertingConfig,
    resolver: &CredentialResolver,
) -> anyhow::Result<Vec<Box<dyn AlertSink>>> {
    if config.sinks.is_empty() {
        return Ok(vec![Box::new(StdoutSink::new(
            DEFAULT_SINK_NAME.to_owned(),
            OutputFormat::Json,
        ))]);
    }
    let network = NetworkSinkBuilder;
    config
        .enabled_specs()
        .context("reading alerting.sinks")?
        .iter()
        .map(|spec| {
            let resolved = resolver.resolve(spec)?;
            AlertSinkFactory::build(&resolved, &config.delivery, Some(&network))
                .with_context(|| format!("building alert sink '{}'", spec.name()))
        })
        .collect()
}
