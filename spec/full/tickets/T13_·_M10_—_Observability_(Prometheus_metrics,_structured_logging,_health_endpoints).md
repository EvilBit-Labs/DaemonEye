# T13 · M10 — Observability (Prometheus metrics, structured logging, health endpoints)

**Milestone:** M10 · **Backlog:** T21 (#60)

## Scope

**In:**

- Prometheus-compatible metrics (collection rate, detection latency, alert delivery, redb write latency) exported via the local CLI/IPC path or Prometheus textfile collector. No HTTP listener ships in v1.0: the CLI `health` path (T10) already covers operator access, and textfile export covers scraping, so neither `hyper` nor `axum` is taken as a dependency.
- Structured JSON logging with correlation IDs; performance metrics embedded in log entries.
- Opt-in localhost-only HTTP health endpoint (disabled by default); resource-utilization + error-rate tracking; scraping-compat tests.

**Out:** Detection tracing metrics already in T6.

## Spec references

- spec/full/specs/Core_Flows\_—\_DaemonEye_Operator_Journeys.md (Flow 7)
- requirements R10.1–R10.4, R10.6

## Key touchpoints

- file:daemoneye-lib/src/telemetry.rs — `TelemetryCollector`/`PerformanceTimer`; extend with Prometheus-compatible metrics (collection rate, detection latency, alert delivery, redb write latency — the storage write path must be instrumented, since T10's health view and the v1.0 success condition both promise it) exported via local CLI/IPC path or textfile.
- Structured JSON logging + correlation IDs via `tracing`/`tracing-subscriber` (already present); embed perf metrics in log entries.
- Prometheus textfile exporter for scrape access. No inbound listener, so the no-inbound-network boundary holds without a new dep.
- Metric names per performance steering (e.g., `daemoneye_processes_collected_total`, `daemoneye_alerts_generated_total{severity=...}`).

## Testing & quality gates

- `cargo clippy --workspace -- -D warnings`, `cargo fmt --all --check` clean.
- Metric-accuracy + Prometheus scrape-compat tests; verify no process binds a listening socket.

## Dependencies

T12 (end-to-end integrated pipeline to instrument).

## Acceptance criteria

- Metrics accurate and Prometheus-scrape-compatible, exported without an inbound listener.
