# Alert Delivery

An alert the agent raises is normally stored first and delivered from the stored record; if storing it fails, it is sent once to local sinks only and marked `not_persisted` (see Persistence and health). One worker per configured sink retries transient failures, trips a circuit breaker when a sink stays down, parks what it cannot deliver in a dead-letter queue, and records every attempt in the `alert_deliveries` table. The detection cursor never waits for delivery ([ADR-0015](../../adr/0015-delivery-state-is-independent-of-the-detection-cursor.md)).

## Sinks

Sinks are configured as `[[alerting.sinks]]` entries. Every sink has a unique `name`. Unknown fields are rejected at load.

```toml
[[alerting.sinks]]
sink_type = "webhook"
enabled = true
[alerting.sinks.config]
name = "soc-hook"
url = "https://hooks.example.internal/daemoneye"
auth = "bearer"                                  # none | bearer | basic | header
token_env = "DAEMONEYE_HOOK_TOKEN"
timeout_secs = 10
# ca_file = "/etc/daemoneye/private-ca.pem"   # replaces the OS trust store for this sink
# allow_http = true             # plain http, only with auth = "none"
# include_command_line = true   # off by default
# include_user = true           # off by default

[[alerting.sinks]]
sink_type = "syslog"
enabled = true
[alerting.sinks.config]
name = "siem"
transport = "tls"                      # udp | tcp | tls | unix
address = "siem.example.internal:6514"
facility = "daemon"
app_name = "daemoneye"
max_message_bytes = 2048               # 480..=65507
# allow_plaintext = true        # required for udp/tcp to a non-loopback address

[[alerting.sinks]]
sink_type = "email"
enabled = true
[alerting.sinks.config]
name = "oncall"
host = "smtp.example.internal"
port = 587
tls = "starttls"                         # starttls | implicit; opportunistic TLS is never used
username_env = "DAEMONEYE_SMTP_USER"
password_env = "DAEMONEYE_SMTP_PASSWORD"
from = "daemoneye@example.internal"
to = ["soc@example.internal"]
subject_prefix = "[DaemonEye] "

[[alerting.sinks]]
sink_type = "file"
enabled = true
[alerting.sinks.config]
name = "local"
path = "/var/log/daemoneye/alerts.jsonl"
format = "json"
```

Local sinks (`stdout`, `file`) receive the full alert. Network sinks receive an allowlist projection: alert id, timestamp, severity, rule, title, description, completeness, pid, ppid, process name, executable path and hash, start time, host, agent version, ancestry and tags. The command line and the user id are sent only when the sink sets `include_command_line` or `include_user`. The same projection, without the opt-in fields, is what the eventbus carries on `events.alert.new`.

A syslog line is RFC 5424 with the rule, pid, completeness and deduplication key as structured data and a one-line summary as the message. A line over `max_message_bytes` is cut from the end of the message, so the opt-in fields go first, and carries `truncated="1"` in its structured data. An email carries the summary and the JSON projection in a plain-text body.

### Credentials and trust

A credential is never written in configuration. Each `*_env` field names an environment variable, read once when the sink is built. An enabled sink whose variable is missing or empty refuses agent startup with an error naming the sink and the variable; an email sink must set `username_env` and `password_env` together or not at all. Rotating a credential needs a restart.

TLS verification uses the operating system's trust store. A `ca_file` replaces that store for the sink; it must exist and hold at least one PEM certificate. There is no option to skip verification. A certificate failure, an HTTP 401 or 403, or an SMTP 535 marks the sink misconfigured, dead-letters the alert that exposed it and opens the breaker so nothing else is attempted until a probe succeeds.

Plain-text transports need an explicit opt-in: `allow_http = true` for an unauthenticated webhook, `allow_plaintext = true` for UDP or TCP syslog to any address that is not loopback. UDP and TCP syslog send the projection unencrypted; prefer TLS for a remote collector.

## Reliability

```toml
[alerting.delivery]
max_attempts = 3               # 1..=10
initial_backoff_ms = 1000      # 100..=10000
max_backoff_secs = 60          # 1..=60
attempt_timeout_secs = 10      # 1..=120, covers connect, handshake and send
breaker_failure_threshold = 5  # 1..=100 consecutive transient failures
breaker_cooldown_secs = 30     # 1..=3600
dlq_max_rows_per_sink = 10000  # 1..=1000000
max_open_rows_per_sink = 10000
dlq_max_redrives = 5           # 0..=100
```

A transient failure (connection, timeout, HTTP 5xx, 408, 429, SMTP 4xx) is retried with exponential backoff and full jitter, never more than `max_backoff_secs` apart, up to `max_attempts`; a `Retry-After` given in seconds is honoured up to the same cap. A permanent failure (other HTTP 4xx, a rejected recipient) is not retried. After the attempts are spent the row is dead-lettered with its reason.

The breaker is per sink. It counts consecutive transient failures, opens at the threshold, admits one probe after the cool-down, and closes on a successful probe. While it is open no network call is made and new rows wait. Local sinks have no breaker.

Dead letters are bounded per sink by `dlq_max_rows_per_sink`; the lowest-severity, oldest rows are evicted first and the eviction is counted. They are redriven automatically, a hundred at a time, once a tripped breaker has stayed closed for a full cool-down, and once at agent start, up to `dlq_max_redrives` per row.

Open rows (pending or retrying) are capped per sink by `max_open_rows_per_sink`. At the cap a critical or high alert displaces the oldest lowest-severity pending row, which is dead-lettered as `queue_full`; any other alert is dead-lettered as `queue_full` itself. A flood of low-severity alerts therefore cannot keep a critical one off the wire.

The rate limit `alerting.max_alerts_per_minute` paces delivery; it never drops an alert.

## At least once

Delivery is at least once. A crash between a sink accepting an alert and the row being written resends it, and a re-evaluated detection window stores the same logical alert under a new id. Receivers should deduplicate on two keys:

| Sink    | Retry key                           | Re-evaluation key              |
| ------- | ----------------------------------- | ------------------------------ |
| webhook | `X-DaemonEye-Alert-Id` header       | `X-DaemonEye-Dedup-Key` header |
| syslog  | MSGID (alert id without hyphens)    | `dedup` structured-data param  |
| email   | `Message-ID` `<alert-id@daemoneye>` | `X-DaemonEye-Dedup-Key` header |

Within the agent, a duplicate inside `alerting.dedup_window_seconds` of a delivered alert with the same deduplication key is stored with `suppressed_duplicate` rows and not sent; the window is rebuilt from stored alerts at startup.

## Persistence and health

If storing an alert fails, it is delivered once, directly, to local sinks only, marked `not_persisted` in its context, and counted in health. Nothing unpersisted reaches a network sink.

Every delivery row records state, reason, severity, attempts, redrives, the last error class, enqueue and next-attempt times, and the terminal time. Terminal rows older than `database.retention_days` are pruned by an hourly sweep; alert rows are never pruned.

The agent logs delivery health periodically: per sink the breaker state, consecutive failures, attempts, deliveries, a success rate over the last hundred attempts, dead-letter depth, evictions, queue-full drops, corrupt rows, the age of the oldest pending row, the last error class and the last success time; plus store and publish failure counts. The aggregate is `degraded` when network sinks fail and every local sink still delivers, and `unhealthy` when a local sink fails or the store does. The operator CLI surface for this data is a later milestone.
