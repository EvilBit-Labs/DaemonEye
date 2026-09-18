# T7 · M4 — Alerting delivery (network sinks, reliability, enrichment)

**Milestone:** M4 · **Backlog:** T14.2 (#53), T15 (#55), T26.2

## Scope

**In:**

- Network sinks: `WebhookSink` (HTTP POST + auth), `SyslogSink`, `EmailSink` (SMTP + templates) with mock-endpoint tests.
- Delivery reliability: per-sink circuit breakers, exponential backoff + jitter retries, dead-letter queue, success-rate tracking, chaos tests.
- Alert enrichment: process-ancestry/system-state enrichment and forensic metadata on individual alerts. Cross-rule correlation and severity escalation are **out** for v1.0 — the backlog item this draws from (T26.2) cites R15.4/R16.3, which the Priority Tiers table places in the post-v1.0 tier.
- Alerts publish to the embedded broker `alerts` topic; sinks consume (R5 AC6); alerts carry `completeness` from T6.

**Out:** Existing stdout/file sinks + parallel delivery (already done).

## Spec references

- spec/full/specs/Core_Flows\_—\_DaemonEye_Operator_Journeys.md (Flow 5)
- spec/full/specs/Tech_Plan\_—_DaemonEye_Core_Monitoring_(v1.0_priority_areas).md(AlertManager interface)
- requirements R4, R5

## Key touchpoints

- file:daemoneye-lib/src/alerting.rs — existing `AlertSink` trait + `AlertManager` (stdout/file + parallel delivery already done); add `WebhookSink`/`SyslogSink`/`EmailSink`, per-sink circuit breaker, exponential-backoff+jitter retries, dead-letter queue, success-rate tracking, per-alert enrichment.
- Alerts publish to embedded broker `alerts` topic (file:daemoneye-eventbus/), sinks consume (R5 AC6); alerts carry `completeness` from T6.
- `alert_deliveries` table (T3) for delivery audit/tracking.
- New deps (flag for scrutiny, must be rustls-based/offline-friendly): webhook HTTP client (e.g., `reqwest` with `default-features=false`, `rustls-tls`), syslog, SMTP (e.g., `lettre` rustls). No inbound network.
- Sink credentials (bearer tokens, API keys, SMTP passwords) are referenced from environment variables or the OS keychain, never stored inline in sink YAML; held in `secrecy`/`zeroize` wrappers in memory and redacted from `daemoneye-cli config show` output and from any T11 bundle, matching the key-handling discipline T8/T9 already specify.
- Bench: file:daemoneye-lib/benches/alert_processing.rs.

## Testing & quality gates

- `cargo clippy --workspace -- -D warnings`, `cargo fmt --all --check` clean; new deps pass `cargo deny`/`cargo audit`.
- Mock-endpoint integration tests + chaos tests (network failure/partition) for circuit breaker + retry + DLQ; local sinks unaffected when network down.

## Dependencies

T6 (alerts with completeness exist).

## Acceptance criteria

- Multi-sink parallel delivery with circuit breaker + retry + DLQ verified under failure injection; local sinks keep working when network down.
- Per-sink health and DLQ backlog observable; delivery attempts audited.
