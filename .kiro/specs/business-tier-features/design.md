# Design Document

## Overview

The Business Tier Features design extends the core DaemonEye architecture with professional-grade capabilities targeting small teams and consultancies. The design maintains the security-first, offline-capable philosophy while adding enterprise integrations, curated content, and centralized management capabilities.

The key architectural addition is the **DaemonEye Security Center**, a new component that provides centralized aggregation, management, and visualization capabilities while preserving the autonomous operation of individual agents.

## Architecture

### Component Overview

```mermaid
graph TB
    subgraph "Business Tier Architecture"
        SC[Security Center Server]
        GUI[Web GUI Frontend]

        subgraph "Agent Node 1"
            PM1[procmond]
            SA1[daemoneye-agent]
            CLI1[daemoneye-cli]
        end

        subgraph "Agent Node 2"
            PM2[procmond]
            SA2[daemoneye-agent]
            CLI2[daemoneye-cli]
        end

        subgraph "External Integrations"
            SPLUNK[Splunk HEC]
            ELASTIC[Elasticsearch]
            KAFKA[Kafka]
        end
    end

    SA1 -.->|mTLS| SC
    SA2 -.->|mTLS| SC
    GUI -->|HTTPS| SC
    SC -->|HTTP/HTTPS| SPLUNK
    SC -->|HTTP/HTTPS| ELASTIC
    SC -->|TCP| KAFKA

    PM1 --> SA1
    PM2 --> SA2
```

### Deployment Patterns

The business tier supports three flexible deployment patterns:

#### Pattern 1: Direct Agent-to-SIEM

```mermaid
graph LR
    subgraph "Agent Nodes"
        SA1[daemoneye-agent-1]
        SA2[daemoneye-agent-2]
        SA3[daemoneye-agent-3]
    end

    subgraph "SIEM Infrastructure"
        SPLUNK[Splunk HEC]
        ELASTIC[Elasticsearch]
        KAFKA[Kafka]
    end

    SA1 -->|Direct| SPLUNK
    SA2 -->|Direct| ELASTIC
    SA3 -->|Direct| KAFKA
```

#### Pattern 2: Centralized Proxy

```mermaid
graph LR
    subgraph "Agent Nodes"
        SA1[daemoneye-agent-1]
        SA2[daemoneye-agent-2]
        SA3[daemoneye-agent-3]
    end

    SC[Security Center]

    subgraph "SIEM Infrastructure"
        SPLUNK[Splunk HEC]
        ELASTIC[Elasticsearch]
        KAFKA[Kafka]
    end

    SA1 -->|mTLS| SC
    SA2 -->|mTLS| SC
    SA3 -->|mTLS| SC
    SC --> SPLUNK
    SC --> ELASTIC
    SC --> KAFKA
```

#### Pattern 3: Hybrid (Recommended)

```mermaid
graph LR
    subgraph "Agent Nodes"
        SA1[daemoneye-agent-1]
        SA2[daemoneye-agent-2]
        SA3[daemoneye-agent-3]
    end

    SC[Security Center]

    subgraph "SIEM Infrastructure"
        SPLUNK[Splunk HEC]
        ELASTIC[Elasticsearch]
        KAFKA[Kafka]
    end

    SA1 -->|Direct| SPLUNK
    SA2 -->|Direct| ELASTIC
    SA3 -->|Direct| KAFKA

    SA1 -.->|Also to SC| SC
    SA2 -.->|Also to SC| SC
    SA3 -.->|Also to SC| SC
```

**Pattern Benefits:**

- **Direct**: Lowest latency, no single point of failure, minimal infrastructure
- **Centralized Proxy**: Simplified SIEM configuration, centralized filtering, bandwidth optimization
- **Hybrid**: Best of both worlds - real-time SIEM integration plus centralized visibility and control

### Security Center Server

The Security Center is a new Rust service that provides:

- **Agent Management**: Secure registration and authentication of daemoneye-agent instances
- **Data Aggregation**: Centralized collection of alerts, process snapshots, and audit logs
- **Configuration Distribution**: Centralized rule management and configuration deployment
- **Integration Hub**: Single point for external SIEM and alerting integrations

### Enhanced Agent Capabilities

Existing daemoneye-agent instances gain:

- **Uplink Communication**: Secure connection to Security Center with fallback to standalone operation
- **Rule Synchronization**: Automatic download and validation of curated rule packs
- **Enhanced Output Formats**: CEF, STIX-lite, and structured JSON export capabilities
- **Flexible Output Routing**: Direct SIEM integration OR proxy through Security Center
- **Dual Logging Mode**: Simultaneous local and centralized logging capabilities

## Components and Interfaces

### Security Center Server

**Technology Stack:**

- **Framework**: Axum web framework with tokio async runtime
- **Database**: PostgreSQL with connection pooling for scalable data storage
- **Authentication**: Mutual TLS (mTLS) for agent connections, JWT for web GUI
- **Configuration**: Same hierarchical config system as core DaemonEye
- **Observability**: OpenTelemetry tracing with Prometheus metrics export
- **Monitoring**: Built-in health checks and performance metrics

**Core Modules:**

```rust
pub mod security_center {
    pub mod agent_registry; // Agent authentication and management
    pub mod data_aggregator; // Central data collection and storage
    pub mod database; // PostgreSQL connection pool and migrations
    pub mod health;
    pub mod integration_hub; // External system connectors
    pub mod observability; // OpenTelemetry tracing and Prometheus metrics
    pub mod rule_distributor; // Rule pack management and distribution
    pub mod web_api; // REST API for GUI frontend // Health checks and system monitoring
}
```

**Database Layer:**

- **Connection Pool**: sqlx::PgPool with configurable min/max connections
- **Migrations**: Embedded SQL migrations using sqlx-migrate
- **Query Builder**: Raw SQL with compile-time verification via sqlx macros
- **Transactions**: ACID compliance for critical operations like rule deployment
- **Monitoring**: Connection pool metrics and query performance tracking

**Observability Layer:**

- **Tracing**: OpenTelemetry with structured spans for request tracing
- **Metrics**: Prometheus metrics for performance and business metrics
- **Health Checks**: Kubernetes-compatible liveness and readiness probes
- **Logging**: Structured JSON logging with correlation IDs

**Database Layer:**

- **Connection Pool**: sqlx::PgPool with configurable min/max connections
- **Migrations**: Embedded SQL migrations using sqlx-migrate
- **Query Builder**: Raw SQL with compile-time verification via sqlx macros
- **Transactions**: ACID compliance for critical operations like rule deployment
- **Monitoring**: Connection pool metrics and query performance tracking

**Agent Registration Flow:**

1. Agent generates client certificate during first startup
2. Agent connects to Security Center with certificate
3. Security Center validates certificate and registers agent
4. Ongoing communication uses established mTLS session

### Curated Rule Packs

**Rule Pack Structure:**

```yaml
# rule-pack-malware-ttps.yaml
metadata:
  name: Malware TTPs
  version: 1.2.0
  description: Common malware tactics, techniques, and procedures
  author: DaemonEye Security Team
  signature: ed25519:base64-signature

rules:
  - id: process-hollowing-detection
    name: Process Hollowing Detection
    description: Detects potential process hollowing attacks
    sql: |
      SELECT * FROM process_snapshots
      WHERE executable_path != mapped_image_path
      AND parent_pid IN (SELECT pid FROM process_snapshots WHERE name = 'explorer.exe')
    severity: high
    tags: [process-hollowing, malware, defense-evasion]
```

**Rule Pack Validation:**

- **Cryptographic Signatures**: Ed25519 signatures for rule pack integrity
- **SQL AST Validation**: Parse and validate SQL syntax before deployment
- **Dependency Checking**: Ensure required database schema compatibility
- **Conflict Resolution**: Detect and resolve rule ID conflicts

### Enhanced Output Connectors

#### Splunk HEC Integration

```rust
pub struct SplunkHecConnector {
    endpoint: Url,
    token: SecretString,
    index: Option<String>,
    source_type: String,
    client: reqwest::Client,
}

impl SplunkHecConnector {
    pub async fn send_event(&self, event: &ProcessAlert) -> Result<(), ConnectorError> {
        let hec_event = HecEvent {
            time: event.timestamp.timestamp(),
            host: event.hostname.clone(),
            source: "DaemonEye",
            sourcetype: &self.source_type,
            index: self.index.as_deref(),
            event: serde_json::to_value(event)?,
        };

        let response = self.client
            .post(&self.endpoint)
            .header("Authorization", format!("Splunk {}", self.token.expose_secret()))
            .json(&hec_event)
            .send()
            .await?;

        response.error_for_status()?;
        Ok(())
    }
}
```

#### Elasticsearch Integration

```rust
pub struct ElasticsearchConnector {
    client: elasticsearch::Elasticsearch,
    index_pattern: String,
    pipeline: Option<String>,
}

impl ElasticsearchConnector {
    pub async fn bulk_index(&self, events: &[ProcessAlert]) -> Result<(), ConnectorError> {
        let mut body = Vec::new();

        for event in events {
            let index_name = self.resolve_index_name(&event.timestamp);
            let action = json!({
                "index": {
                    "_index": index_name,
                    "_type": "_doc"
                }
            });
            body.push(action);
            body.push(serde_json::to_value(event)?);
        }

        let response = self.client
            .bulk(BulkParts::None)
            .body(body)
            .send()
            .await?;

        self.handle_bulk_response(response).await
    }
}
```

### Export Format Implementations

#### CEF (Common Event Format)

```rust
pub struct CefFormatter;

impl CefFormatter {
    pub fn format_process_alert(alert: &ProcessAlert) -> String {
        format!(
            "CEF:0|DaemonEye|DaemonEye|1.0|{}|{}|{}|{}",
            alert.rule_id,
            alert.rule_name,
            Self::map_severity(&alert.severity),
            Self::build_extensions(alert)
        )
    }

    fn build_extensions(alert: &ProcessAlert) -> String {
        format!(
            "rt={} src={} suser={} sproc={} cs1Label=Command Line cs1={} cs2Label=Parent Process cs2={}",
            alert.timestamp.timestamp_millis(),
            alert.hostname,
            alert.process.user.unwrap_or_default(),
            alert.process.name,
            alert.process.command_line.unwrap_or_default(),
            alert.process.parent_name.unwrap_or_default()
        )
    }
}
```

#### STIX 2.1 Objects

```rust
pub struct StixExporter;

impl StixExporter {
    pub fn create_process_object(process: &ProcessSnapshot) -> StixProcess {
        StixProcess {
            type_: "process".to_string(),
            spec_version: "2.1".to_string(),
            id: format!("process--{}", Uuid::new_v4()),
            created: process.timestamp,
            modified: process.timestamp,
            pid: process.pid,
            name: process.name.clone(),
            command_line: process.command_line.clone(),
            parent_ref: process
                .parent_pid
                .map(|ppid| format!("process--{}", self.get_parent_uuid(ppid))),
            binary_ref: Some(format!(
                "file--{}",
                self.create_file_object(&process.executable_path).id
            )),
        }
    }
}
```

### Licensing Architecture

**Dual-License Strategy:** The DaemonEye project maintains a dual-license approach to balance open source accessibility with commercial sustainability:

- **Core Components**: Apache 2.0 licensed (procmond, daemoneye-agent, daemoneye-cli, daemoneye-lib)
- **Business Tier Features**: Commercial license required (Security Center, GUI, enhanced connectors, curated rules)

**Feature Gating Implementation:**

```rust
// Compile-time feature gates
#[cfg(feature = "business-tier")]
pub mod security_center;

#[cfg(feature = "business-tier")]
pub mod enhanced_connectors;

// Runtime license validation
pub struct LicenseValidator {
    public_key: ed25519_dalek::PublicKey,
    grace_period: Duration,
}

impl LicenseValidator {
    pub fn validate_license(&self, license: &str) -> Result<LicenseInfo, LicenseError> {
        let license_data: LicenseData = serde_json::from_str(license)?;

        // Verify cryptographic signature
        let signature = ed25519_dalek::Signature::from_bytes(&license_data.signature)?;
        self.public_key
            .verify_strict(&license_data.payload, &signature)?;

        // Validate site restrictions (hostname/domain matching)
        if !self.validate_site_restrictions(&license_data)? {
            return Err(LicenseError::SiteRestriction);
        }

        Ok(LicenseInfo {
            tier: license_data.tier,
            site_id: license_data.site_id,
            features: license_data.features,
        })
    }
}
```

**License Distribution:**

- **Open Source**: GitHub releases with Apache 2.0 license
- **Business Tier**: Separate distribution channel with license keys
- **Hybrid Builds**: Single binary with runtime feature activation based on license

**Graceful Degradation:** When business tier license is invalid or expired:

1. Security Center gracefully shuts down non-essential services
2. Agents continue operating with core functionality
3. Enhanced connectors disable with fallback to basic outputs
4. GUI displays license status and upgrade prompts

### Container and Kubernetes Support

#### DaemonSet Configuration

```yaml
# k8s/daemonset.yaml
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: DaemonEye-agent
  namespace: security
spec:
  selector:
    matchLabels:
      app: DaemonEye-agent
  template:
    metadata:
      labels:
        app: DaemonEye-agent
    spec:
      serviceAccountName: DaemonEye-agent
      hostPID: true
      hostNetwork: true
      containers:
        - name: procmond
          image: DaemonEye/procmond:latest
          securityContext:
            privileged: true
            capabilities:
              add: [SYS_PTRACE]
          volumeMounts:
            - name: proc
              mountPath: /host/proc
              readOnly: true
            - name: data
              mountPath: /var/lib/DaemonEye
        - name: daemoneye-agent
          image: DaemonEye/daemoneye-agent:latest
          securityContext:
            runAsNonRoot: true
            runAsUser: 1000
          volumeMounts:
            - name: data
              mountPath: /var/lib/DaemonEye
            - name: config
              mountPath: /etc/DaemonEye
      volumes:
        - name: proc
          hostPath:
            path: /proc
        - name: data
          hostPath:
            path: /var/lib/DaemonEye
        - name: config
          configMap:
            name: DaemonEye-config
```

### Web GUI Frontend

**Technology Stack:**

- **Frontend**: React with TypeScript for type safety
- **State Management**: React Query for server state management
- **UI Framework**: Tailwind CSS with shadcn/ui components
- **Charts**: Recharts for data visualization
- **Authentication**: JWT tokens with automatic refresh

**Core Features:**

- **Fleet Dashboard**: Real-time view of all connected agents
- **Alert Management**: Filtering, sorting, and export of alerts
- **Rule Management**: Visual rule editor and deployment interface
- **System Health**: Agent connectivity and performance metrics

## Data Models

### Security Center Database Schema

```sql
-- Agent registration and management
CREATE TABLE agents (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    hostname VARCHAR(255) NOT NULL,
    ip_address INET,
    certificate_fingerprint VARCHAR(128) NOT NULL UNIQUE,
    first_seen TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    version VARCHAR(50) NOT NULL,
    status VARCHAR(20) NOT NULL CHECK (status IN ('active', 'inactive', 'error')),
    metadata JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes for performance
CREATE INDEX idx_agents_hostname ON agents(hostname);
CREATE INDEX idx_agents_status ON agents(status);
CREATE INDEX idx_agents_last_seen ON agents(last_seen);

-- Aggregated alerts from all agents
CREATE TABLE aggregated_alerts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    rule_id VARCHAR(100) NOT NULL,
    rule_name VARCHAR(255) NOT NULL,
    severity VARCHAR(20) NOT NULL CHECK (severity IN ('low', 'medium', 'high', 'critical')),
    timestamp TIMESTAMPTZ NOT NULL,
    hostname VARCHAR(255) NOT NULL,
    process_data JSONB NOT NULL,
    metadata JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes for alert queries
CREATE INDEX idx_alerts_agent_id ON aggregated_alerts(agent_id);
CREATE INDEX idx_alerts_timestamp ON aggregated_alerts(timestamp DESC);
CREATE INDEX idx_alerts_severity ON aggregated_alerts(severity);
CREATE INDEX idx_alerts_rule_id ON aggregated_alerts(rule_id);
CREATE INDEX idx_alerts_hostname ON aggregated_alerts(hostname);

-- Rule pack management
CREATE TABLE rule_packs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(255) NOT NULL,
    version VARCHAR(50) NOT NULL,
    description TEXT,
    author VARCHAR(255),
    signature TEXT NOT NULL,
    content BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deployed_at TIMESTAMPTZ,
    UNIQUE(name, version)
);

-- Agent rule pack assignments
CREATE TABLE agent_rule_assignments (
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    rule_pack_id UUID NOT NULL REFERENCES rule_packs(id) ON DELETE CASCADE,
    assigned_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    status VARCHAR(20) NOT NULL CHECK (status IN ('pending', 'deployed', 'failed')),
    error_message TEXT,
    PRIMARY KEY (agent_id, rule_pack_id)
);

-- Performance monitoring table
CREATE TABLE agent_metrics (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id UUID NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    cpu_usage DECIMAL(5,2),
    memory_usage_mb INTEGER,
    disk_usage_mb INTEGER,
    process_count INTEGER,
    alert_count_last_hour INTEGER,
    metadata JSONB
);

-- Partition agent_metrics by time for better performance
CREATE INDEX idx_agent_metrics_timestamp ON agent_metrics(timestamp DESC);
CREATE INDEX idx_agent_metrics_agent_id ON agent_metrics(agent_id);
```

### Enhanced Configuration Schema

```yaml
# Business tier configuration extensions
business_tier:
  license:
    key: business-license-key
    validation_endpoint:       # Offline validation only
  security_center:
    enabled: true
    bind_address: 0.0.0.0:8443
    tls:
      cert_path: /etc/DaemonEye/tls/server.crt
      key_path: /etc/DaemonEye/tls/server.key
      ca_path: /etc/DaemonEye/tls/ca.crt
    database:
      url: 
        postgresql://DaemonEye:${DB_PASSWORD}@localhost:5432/DaemonEye_security_center
      max_connections: 20
      min_connections: 5
      connection_timeout: 30s
      idle_timeout: 10m
      max_lifetime: 1h
  rule_packs:
    auto_update: true
    sources:
      - name: official
        url: https://rules.DaemonEye.com/packs/
        signature_key: ed25519:public-key
  output_connectors:
    # Routing strategy: "direct", "proxy", or "hybrid"
    routing_strategy: hybrid

    # When routing_strategy is "proxy", agents send to Security Center only
    # When "direct", agents send directly to configured sinks
    # When "hybrid", agents send to both Security Center AND direct sinks
    splunk_hec:
      enabled: false
      endpoint: https://splunk.example.com:8088/services/collector
      token: ${SPLUNK_HEC_TOKEN}
      index: DaemonEye
      source_type: DaemonEye:alert
      # Available on both agents and Security Center
    elasticsearch:
      enabled: false
      hosts: [https://elastic.example.com:9200]
      username: ${ELASTIC_USERNAME}
      password: ${ELASTIC_PASSWORD}
      index_pattern: DaemonEye-{YYYY.MM.DD}
      # Available on both agents and Security Center
    kafka:
      enabled: false
      brokers: [kafka.example.com:9092]
      topic: DaemonEye.alerts
      security_protocol: SASL_SSL
      sasl_mechanism: PLAIN
      # Available on both agents and Security Center
  gui:
    enabled: false
    bind_address: 0.0.0.0:8080
    session_timeout: 24h
    max_sessions: 100

  observability:
    tracing:
      enabled: true
      endpoint: http://jaeger:14268/api/traces
      service_name: DaemonEye-security-center
      sample_rate: 0.1

    metrics:
      enabled: true
      bind_address: 0.0.0.0:9090
      path: /metrics
    health_checks:
      enabled: true
      bind_address: 0.0.0.0:8081
      liveness_path: /health/live
      readiness_path: /health/ready
```

## Error Handling

### Connector Resilience Patterns

**Circuit Breaker Implementation:**

```rust
pub struct CircuitBreaker {
    state: Arc<Mutex<CircuitState>>,
    failure_threshold: usize,
    recovery_timeout: Duration,
    half_open_max_calls: usize,
}

#[derive(Debug, Clone)]
enum CircuitState {
    Closed { failure_count: usize },
    Open { opened_at: Instant },
    HalfOpen { success_count: usize, failure_count: usize },
}

impl CircuitBreaker {
    pub async fn call<F, T, E>(&self, f: F) -> Result<T, CircuitBreakerError<E>>
    where
        F: Future<Output = Result<T, E>>,
    {
        match self.current_state() {
            CircuitState::Open { .. } => Err(CircuitBreakerError::Open),
            _ => {
                match f.await {
                    Ok(result) => {
                        self.on_success();
                        Ok(result)
                    }
                    Err(error) => {
                        self.on_failure();
                        Err(CircuitBreakerError::Failure(error))
                    }
                }
            }
        }
    }
}
```

**Retry Logic with Exponential Backoff:**

```rust
pub struct RetryPolicy {
    max_attempts: usize,
    base_delay: Duration,
    max_delay: Duration,
    backoff_multiplier: f64,
}

impl RetryPolicy {
    pub async fn execute<F, T, E>(&self, mut f: F) -> Result<T, E>
    where
        F: FnMut() -> Pin<Box<dyn Future<Output = Result<T, E>> + Send>>,
        E: std::fmt::Debug,
    {
        let mut attempt = 0;
        let mut delay = self.base_delay;

        loop {
            attempt += 1;

            match f().await {
                Ok(result) => return Ok(result),
                Err(error) if attempt >= self.max_attempts => return Err(error),
                Err(error) => {
                    tracing::warn!(
                        attempt = attempt,
                        max_attempts = self.max_attempts,
                        delay_ms = delay.as_millis(),
                        error = ?error,
                        "Retry attempt failed, backing off"
                    );

                    tokio::time::sleep(delay).await;
                    delay = std::cmp::min(
                        Duration::from_millis(
                            (delay.as_millis() as f64 * self.backoff_multiplier) as u64
                        ),
                        self.max_delay
                    );
                }
            }
        }
    }
}
```

### Graceful Degradation

**Agent Autonomy Preservation:**

- Agents continue full operation when Security Center is unavailable
- Local rule caching ensures detection continues during network outages
- Buffered alert delivery with persistent queue for reliability

**Resource Exhaustion Handling:**

- Memory pressure detection with automatic buffer size reduction
- Disk space monitoring with log rotation and cleanup
- CPU throttling under high load conditions

## Testing Strategy

### Integration Testing Approach

**Security Center Testing:**

```rust
#[tokio::test]
async fn test_agent_registration_flow() {
    let security_center = test_security_center().await;
    let agent_cert = generate_test_certificate();

    // Test agent registration
    let registration_result = security_center
        .register_agent(agent_cert.clone())
        .await;

    assert!(registration_result.is_ok());

    // Test subsequent authentication
    let auth_result = security_center
        .authenticate_agent(&agent_cert.fingerprint())
        .await;

    assert!(auth_result.is_ok());
}

#[tokio::test]
async fn test_rule_pack_distribution() {
    let (security_center, mut agent) = test_setup().await;

    // Deploy rule pack to Security Center
    let rule_pack = create_test_rule_pack();
    security_center.deploy_rule_pack(rule_pack.clone()).await.unwrap();

    // Verify agent receives rule pack
    let received_rules = agent.wait_for_rule_update().await;
    assert_eq!(received_rules.len(), rule_pack.rules.len());
}
```

**Connector Testing:**

```rust
#[tokio::test]
async fn test_splunk_hec_connector_with_retry() {
    let mock_server = MockServer::start().await;

    // Configure mock to fail first two requests, succeed on third
    Mock::given(method("POST"))
        .and(path("/services/collector"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/services/collector"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let connector = SplunkHecConnector::new(
        mock_server.uri().parse().unwrap(),
        "test-token".into(),
        RetryPolicy::default()
    );

    let alert = create_test_alert();
    let result = connector.send_event(&alert).await;

    assert!(result.is_ok());
}
```

**Performance Testing:**

```rust
#[tokio::test]
async fn test_security_center_load_handling() {
    let security_center = test_security_center().await;

    // Simulate 100 concurrent agents sending alerts
    let tasks: Vec<_> = (0..100)
        .map(|i| {
            let sc = security_center.clone();
            tokio::spawn(async move {
                let agent_id = format!("agent-{}", i);
                for j in 0..1000 {
                    let alert = create_test_alert_for_agent(&agent_id, j);
                    sc.receive_alert(alert).await.unwrap();
                }
            })
        })
        .collect();

    // Wait for all tasks to complete
    for task in tasks {
        task.await.unwrap();
    }

    // Verify all alerts were processed
    let total_alerts = security_center.get_alert_count().await;
    assert_eq!(total_alerts, 100_000);
}
```

### Observability Implementation

**Prometheus Metrics:**

```rust
use prometheus::{Counter, Gauge, Histogram, Registry};

pub struct SecurityCenterMetrics {
    pub agents_connected: Gauge,
    pub alerts_received_total: Counter,
    pub alerts_processed_total: Counter,
    pub rule_deployments_total: Counter,
    pub database_query_duration: Histogram,
    pub connector_send_duration: Histogram,
    pub connector_failures_total: Counter,
}

impl SecurityCenterMetrics {
    pub fn new(registry: &Registry) -> Self {
        let metrics = Self {
            agents_connected: Gauge::new(
                "DaemonEye_agents_connected",
                "Number of currently connected agents",
            )
            .unwrap(),
            alerts_received_total: Counter::new(
                "DaemonEye_alerts_received_total",
                "Total number of alerts received from agents",
            )
            .unwrap(),
            // ... other metrics
        };

        registry
            .register(Box::new(metrics.agents_connected.clone()))
            .unwrap();
        registry
            .register(Box::new(metrics.alerts_received_total.clone()))
            .unwrap();

        metrics
    }
}
```

**OpenTelemetry Tracing:**

```rust
use tracing::{info, instrument, Span};
use opentelemetry::trace::TraceContextExt;

#[instrument(skip(self, alert), fields(agent_id = %alert.agent_id, rule_id = %alert.rule_id))]
pub async fn process_alert(&self, alert: Alert) -> Result<(), ProcessingError> {
    let span = Span::current();
    span.set_attribute("alert.severity", alert.severity.to_string());
    span.set_attribute("alert.hostname", alert.hostname.clone());

    info!("Processing alert from agent");

    // Store in database with tracing
    self.store_alert(&alert).await?;

    // Forward to connectors with tracing
    self.forward_to_connectors(&alert).await?;

    self.metrics.alerts_processed_total.inc();

    Ok(())
}
```

**Health Check Implementation:**

```rust
#[derive(serde::Serialize)]
pub struct HealthStatus {
    pub status: String,
    pub database: ComponentHealth,
    pub connectors: Vec<ConnectorHealth>,
    pub agents: AgentHealth,
}

pub async fn readiness_check(&self) -> HealthStatus {
    HealthStatus {
        status: "ready".to_string(),
        database: self.check_database_health().await,
        connectors: self.check_connector_health().await,
        agents: self.check_agent_health().await,
    }
}
```

### Security Testing

**Certificate Validation Testing:**

```rust
#[tokio::test]
async fn test_invalid_certificate_rejection() {
    let security_center = test_security_center().await;
    let invalid_cert = generate_invalid_certificate();

    let result = security_center
        .register_agent(invalid_cert)
        .await;

    assert!(matches!(result, Err(SecurityCenterError::InvalidCertificate(_))));
}

#[tokio::test]
async fn test_rule_pack_signature_validation() {
    let security_center = test_security_center().await;
    let mut rule_pack = create_test_rule_pack();

    // Tamper with rule pack content
    rule_pack.content = "malicious content".to_string();

    let result = security_center
        .deploy_rule_pack(rule_pack)
        .await;

    assert!(matches!(result, Err(SecurityCenterError::InvalidSignature)));
}
```

## Licensing Architecture

**Dual-License Strategy:** The DaemonEye project maintains a dual-license approach to balance open source accessibility with commercial sustainability:

- **Core Components**: Apache 2.0 licensed (procmond, daemoneye-agent, daemoneye-cli, daemoneye-lib)
- **Business Tier Features**: Commercial license required (Security Center, GUI, enhanced connectors, curated rules)

**Feature Gating Implementation:**

```rust
// Compile-time feature gates
#[cfg(feature = "business-tier")]
pub mod security_center;

#[cfg(feature = "business-tier")]
pub mod enhanced_connectors;

// Runtime license validation
pub struct LicenseValidator {
    public_key: ed25519_dalek::PublicKey,
}

impl LicenseValidator {
    pub fn validate_license(&self, license: &str) -> Result<LicenseInfo, LicenseError> {
        let license_data: LicenseData = serde_json::from_str(license)?;

        // Verify cryptographic signature
        let signature = ed25519_dalek::Signature::from_bytes(&license_data.signature)?;
        self.public_key
            .verify_strict(&license_data.payload, &signature)?;

        // Validate site restrictions (hostname/domain matching)
        if !self.validate_site_restrictions(&license_data)? {
            return Err(LicenseError::SiteRestriction);
        }

        Ok(LicenseInfo {
            tier: license_data.tier,
            site_id: license_data.site_id,
            features: license_data.features,
        })
    }
}
```

**License Structure:**

```json
{
  "tier": "business",
  "site_id": "customer-domain.com",
  "issued_at": "2024-01-15T00:00:00Z",
  "features": [
    "security_center",
    "enhanced_connectors",
    "curated_rules",
    "gui_frontend",
    "container_deployment"
  ],
  "signature": "ed25519_signature_bytes"
}
```

**License Distribution:**

- **Open Source**: GitHub releases with Apache 2.0 license
- **Business Tier**: Separate distribution channel with license keys
- **Hybrid Builds**: Single binary with runtime feature activation based on license

**Graceful Degradation:** When business tier license is invalid or missing:

1. Security Center features are disabled (compile-time gated out)
2. Agents continue operating with core Apache 2.0 functionality
3. Enhanced connectors fall back to basic syslog/webhook outputs
4. CLI displays license status and purchase information
