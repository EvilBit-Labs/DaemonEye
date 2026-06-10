# T13 · M10 — Observability (Prometheus metrics, structured logging, health endpoints)

**Milestone:** M10 · **Backlog:** T21 (#60)

## Scope

**In:**

- Prometheus-compatible metrics (collection rate, detection latency, alert delivery) via local CLI/IPC path or textfile export by default; any HTTP listener opt-in, loopback-only.
- Structured JSON logging with correlation IDs; performance metrics embedded in log entries.
- Opt-in localhost-only HTTP health endpoint (disabled by default); resource-utilization + error-rate tracking; scraping-compat tests.

**Out:** Detection tracing metrics already in T6.

## Spec references

- spec:.../811199cd-23dc-4ded-9932-735252ec2438 (Flow 7)
- requirements R10.1–R10.4, R10.6

## Key touchpoints

- file:daemoneye-lib/src/telemetry.rs — `TelemetryCollector`/`PerformanceTimer`; extend with Prometheus-compatible metrics (collection rate, detection latency, alert delivery) exported via local CLI/IPC path or textfile by default.
- Structured JSON logging + correlation IDs via `tracing`/`tracing-subscriber` (already present); embed perf metrics in log entries.
- Opt-in, loopback-only HTTP health/metrics listener (disabled by default) — new dep flagged for scrutiny (e.g., minimal `hyper`/`axum` rustls, or a Prometheus textfile exporter to avoid an inbound listener entirely); honor the no-inbound-network boundary.
- Metric names per performance steering (e.g., `daemoneye_processes_collected_total`, `daemoneye_alerts_generated_total{severity=...}`).

## Testing & quality gates

- `cargo clippy --workspace -- -D warnings`, `cargo fmt --all --check` clean; any new listener dep passes `cargo deny`/`cargo audit`.
- Metric-accuracy + Prometheus scrape-compat tests; verify HTTP listeners are off by default.

## Dependencies

T

## Acceptance criteria

- Metrics accurate and Prometheus-scrape-compatible; HTTP listeners off by default and documented as a stealth/attack-surface tradeoff.
