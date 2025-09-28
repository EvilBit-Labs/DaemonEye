# Core Monitoring Technical Specification

## Overview

The Core Monitoring specification defines the fundamental process monitoring capabilities that form the foundation of DaemonEye. This includes process enumeration, executable integrity verification, SQL-based detection engine, and multi-channel alerting across the three-component architecture.

## Process Collection Architecture

### Cross-Platform Process Enumeration

DaemonEye uses a layered approach to process enumeration, providing a unified interface across different operating systems while allowing platform-specific optimizations.

#### Base Implementation (sysinfo crate)

**Primary Interface**: The `sysinfo` crate provides cross-platform process enumeration with consistent data structures.

```rust
use sysinfo::{Pid, ProcessExt, System, SystemExt};

pub struct ProcessCollector {
    system: System,
    config: CollectorConfig,
    hash_computer: Box<dyn HashComputer>,
}

impl ProcessCollector {
    pub async fn enumerate_processes(&self) -> Result<Vec<ProcessRecord>> {
        self.system.refresh_processes();

        let mut processes = Vec::new();
        let collection_time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;

        for (pid, process) in self.system.processes() {
            let process_record = ProcessRecord {
                id: Uuid::new_v4(),
                scan_id: self.get_current_scan_id(),
                collection_time,
                pid: pid.as_u32(),
                ppid: process.parent().map(|p| p.as_u32()),
                name: process.name().to_string(),
                executable_path: process.exe().map(|p| p.to_path_buf()),
                command_line: process.cmd().to_vec(),
                start_time: process.start_time(),
                cpu_usage: process.cpu_usage(),
                memory_usage: Some(process.memory()),
                executable_hash: self.compute_executable_hash(process.exe()).await?,
                hash_algorithm: Some("sha256".to_string()),
                user_id: self.get_process_user(pid).await?,
                accessible: true,
                file_exists: process.exe().map(|p| p.exists()).unwrap_or(false),
                platform_data: self.get_platform_specific_data(pid).await?,
            };

            processes.push(process_record);
        }

        Ok(processes)
    }
}
```

#### Platform-Specific Enhancements

**Linux eBPF Integration (Enterprise Tier)**:

```rust
#[cfg(target_os = "linux")]
pub struct EbpfProcessCollector {
    base_collector: ProcessCollector,
    ebpf_monitor: Option<EbpfMonitor>,
}

impl EbpfProcessCollector {
    pub async fn enumerate_processes(&self) -> Result<Vec<ProcessRecord>> {
        // Use eBPF for real-time process events if available
        if let Some(ebpf) = &self.ebpf_monitor {
            return self.enumerate_with_ebpf(ebpf).await;
        }

        // Fallback to sysinfo
        self.base_collector.enumerate_processes().await
    }
}
```

**Windows ETW Integration (Enterprise Tier)**:

```rust
#[cfg(target_os = "windows")]
pub struct EtwProcessCollector {
    base_collector: ProcessCollector,
    etw_monitor: Option<EtwMonitor>,
}

impl EtwProcessCollector {
    pub async fn enumerate_processes(&self) -> Result<Vec<ProcessRecord>> {
        // Use ETW for enhanced process monitoring if available
        if let Some(etw) = &self.etw_monitor {
            return self.enumerate_with_etw(etw).await;
        }

        // Fallback to sysinfo
        self.base_collector.enumerate_processes().await
    }
}
```

**macOS EndpointSecurity Integration (Enterprise Tier)**:

```rust
#[cfg(target_os = "macos")]
pub struct EndpointSecurityProcessCollector {
    base_collector: ProcessCollector,
    es_monitor: Option<EndpointSecurityMonitor>,
}

impl EndpointSecurityProcessCollector {
    pub async fn enumerate_processes(&self) -> Result<Vec<ProcessRecord>> {
        // Use EndpointSecurity for real-time monitoring if available
        if let Some(es) = &self.es_monitor {
            return self.enumerate_with_endpoint_security(es).await;
        }

        // Fallback to sysinfo
        self.base_collector.enumerate_processes().await
    }
}
```

### Executable Integrity Verification

**Hash Computation**: SHA-256 hashing of executable files for integrity verification.

```rust
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

pub trait HashComputer {
    fn compute_hash(&self, path: &Path) -> Result<Option<String>>;
    fn get_algorithm(&self) -> &'static str;
}

pub struct Sha256HashComputer {
    buffer_size: usize,
}

impl HashComputer for Sha256HashComputer {
    fn compute_hash(&self, path: &Path) -> Result<Option<String>> {
        if !path.exists() {
            return Ok(None);
        }

        let mut file = std::fs::File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; self.buffer_size];

        loop {
            let bytes_read = file.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);
        }

        let hash = hasher.finalize();
        Ok(Some(format!("{:x}", hash)))
    }

    fn get_algorithm(&self) -> &'static str {
        "sha256"
    }
}
```

**Performance Optimization**: Asynchronous hash computation with configurable buffer sizes.

```rust
impl ProcessCollector {
    async fn compute_executable_hash(&self, path: Option<&Path>) -> Result<Option<String>> {
        let path = match path {
            Some(p) => p,
            None => return Ok(None),
        };

        // Skip hashing for system processes or inaccessible files
        if self.should_skip_hashing(path) {
            return Ok(None);
        }

        // Compute hash asynchronously
        let hash_computer = self.hash_computer.clone();
        let path = path.to_path_buf();

        tokio::task::spawn_blocking(move || hash_computer.compute_hash(&path)).await?
    }

    fn should_skip_hashing(&self, path: &Path) -> bool {
        // Skip hashing for system processes or temporary files
        let path_str = path.to_string_lossy();
        path_str.contains("/proc/")
            || path_str.contains("/sys/")
            || path_str.contains("/tmp/")
            || path_str.contains("\\System32\\")
    }
}
```

## SQL-Based Detection Engine

### SQL Validation and Security

**AST Validation**: Comprehensive SQL parsing and validation to prevent injection attacks.

```rust
use sqlparser::{ast::*, dialect::SQLiteDialect, parser::Parser};

pub struct SqlValidator {
    parser: Parser<SQLiteDialect>,
    allowed_functions: HashSet<String>,
    allowed_operators: HashSet<String>,
}

impl SqlValidator {
    pub fn new() -> Self {
        let dialect = SQLiteDialect {};
        let parser = Parser::new(&dialect);

        Self {
            parser,
            allowed_functions: Self::get_allowed_functions(),
            allowed_operators: Self::get_allowed_operators(),
        }
    }

    pub fn validate_query(&self, sql: &str) -> Result<ValidationResult> {
        let ast = self.parser.parse_sql(sql)?;

        for statement in &ast {
            match statement {
                Statement::Query(query) => self.validate_select_query(query)?,
                _ => return Err(ValidationError::ForbiddenStatement),
            }
        }

        Ok(ValidationResult::Valid)
    }

    fn validate_select_query(&self, query: &Query) -> Result<()> {
        self.validate_select_body(&query.body)?;

        if let Some(selection) = &query.selection {
            self.validate_where_clause(selection)?;
        }

        if let Some(group_by) = &query.group_by {
            self.validate_group_by(group_by)?;
        }

        if let Some(having) = &query.having {
            self.validate_having(having)?;
        }

        Ok(())
    }

    fn validate_select_body(&self, body: &SetExpr) -> Result<()> {
        match body {
            SetExpr::Select(select) => {
                for item in &select.projection {
                    self.validate_projection_item(item)?;
                }

                if let Some(from) = &select.from {
                    self.validate_from_clause(from)?;
                }
            }
            _ => return Err(ValidationError::UnsupportedSetExpr),
        }

        Ok(())
    }

    fn validate_projection_item(&self, item: &SelectItem) -> Result<()> {
        match item {
            SelectItem::UnnamedExpr(expr) => self.validate_expression(expr)?,
            SelectItem::ExprWithAlias { expr, .. } => self.validate_expression(expr)?,
            SelectItem::Wildcard => Ok(()), // Allow SELECT *
            _ => Err(ValidationError::UnsupportedSelectItem),
        }
    }

    fn validate_expression(&self, expr: &Expr) -> Result<()> {
        match expr {
            Expr::Identifier(_) => Ok(()),
            Expr::Literal(_) => Ok(()),
            Expr::BinaryOp { left, op, right } => {
                self.validate_operator(op)?;
                self.validate_expression(left)?;
                self.validate_expression(right)?;
                Ok(())
            }
            Expr::Function { name, args, .. } => {
                self.validate_function(name, args)?;
                Ok(())
            }
            Expr::Cast { expr, .. } => self.validate_expression(expr),
            Expr::Case { .. } => Ok(()), // Allow CASE expressions
            _ => Err(ValidationError::UnsupportedExpression),
        }
    }

    fn validate_function(&self, name: &ObjectName, args: &[FunctionArg]) -> Result<()> {
        let func_name = name.to_string().to_lowercase();

        if !self.allowed_functions.contains(&func_name) {
            return Err(ValidationError::ForbiddenFunction(func_name));
        }

        // Validate function arguments
        for arg in args {
            match arg {
                FunctionArg::Unnamed(FunctionArgExpr::Expr(expr)) => {
                    self.validate_expression(expr)?;
                }
                _ => return Err(ValidationError::UnsupportedFunctionArg),
            }
        }

        Ok(())
    }

    fn get_allowed_functions() -> HashSet<String> {
        [
            "count",
            "sum",
            "avg",
            "min",
            "max",
            "length",
            "substr",
            "upper",
            "lower",
            "datetime",
            "strftime",
            "unixepoch",
            "coalesce",
            "nullif",
            "ifnull",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }
}
```

### Detection Rule Execution

**Sandboxed Execution**: Safe execution of detection rules with resource limits.

```rust
pub struct DetectionEngine {
    db: redb::Database,
    sql_validator: SqlValidator,
    rule_manager: RuleManager,
    alert_manager: AlertManager,
}

impl DetectionEngine {
    pub async fn execute_rules(&self, scan_id: i64) -> Result<Vec<Alert>> {
        let rules = self.rule_manager.load_enabled_rules().await?;
        let mut alerts = Vec::new();

        for rule in rules {
            match self.execute_rule(&rule, scan_id).await {
                Ok(rule_alerts) => alerts.extend(rule_alerts),
                Err(e) => {
                    tracing::error!(
                        rule_id = %rule.id,
                        error = %e,
                        "Failed to execute detection rule"
                    );
                    // Continue with other rules
                }
            }
        }

        Ok(alerts)
    }

    async fn execute_rule(&self, rule: &DetectionRule, scan_id: i64) -> Result<Vec<Alert>> {
        // Validate SQL before execution
        self.sql_validator.validate_query(&rule.sql_query)?;

        // Execute with timeout and resource limits
        let execution_result = tokio::time::timeout(
            Duration::from_secs(30),
            self.execute_sql_query(&rule.sql_query, scan_id),
        )
        .await??;

        // Generate alerts from query results
        let mut alerts = Vec::new();
        for row in execution_result.rows {
            let alert = self
                .alert_manager
                .generate_alert(&rule, &row, scan_id)
                .await?;

            if let Some(alert) = alert {
                alerts.push(alert);
            }
        }

        Ok(alerts)
    }

    async fn execute_sql_query(&self, sql: &str, scan_id: i64) -> Result<QueryResult> {
        // Use read-only connection for security
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(PROCESSES_TABLE)?;

        // Execute prepared statement with parameters
        let mut stmt = self.db.prepare(sql)?;
        stmt.bind((":scan_id", scan_id))?;

        let mut rows = Vec::new();
        while let Some(row) = stmt.next()? {
            rows.push(ProcessRow::from_sqlite_row(row)?);
        }

        Ok(QueryResult { rows })
    }
}
```

## Alert Generation and Management

### Alert Data Model

**Structured Alerts**: Comprehensive alert structure with full context.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: Uuid,
    pub alert_time: i64,
    pub rule_id: String,
    pub title: String,
    pub description: String,
    pub severity: AlertSeverity,
    pub scan_id: Option<i64>,
    pub affected_processes: Vec<u32>,
    pub process_count: i32,
    pub alert_data: serde_json::Value,
    pub rule_execution_time_ms: Option<i64>,
    pub dedupe_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertSeverity {
    Low,
    Medium,
    High,
    Critical,
}

impl Alert {
    pub fn new(rule: &DetectionRule, process_data: &ProcessRow, scan_id: Option<i64>) -> Self {
        let alert_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;

        let dedupe_key = self.generate_dedupe_key(rule, process_data);

        Self {
            id: Uuid::new_v4(),
            alert_time,
            rule_id: rule.id.clone(),
            title: rule.name.clone(),
            description: rule.description.clone().unwrap_or_default(),
            severity: rule.severity.clone(),
            scan_id,
            affected_processes: vec![process_data.pid],
            process_count: 1,
            alert_data: serde_json::to_value(process_data).unwrap(),
            rule_execution_time_ms: None,
            dedupe_key,
        }
    }

    fn generate_dedupe_key(&self, rule: &DetectionRule, process_data: &ProcessRow) -> String {
        // Generate deduplication key based on rule and process characteristics
        format!(
            "{}:{}:{}:{}",
            rule.id,
            process_data.pid,
            process_data.name,
            process_data.executable_path.as_deref().unwrap_or("")
        )
    }
}
```

### Alert Deduplication

**Intelligent Deduplication**: Prevent alert spam while maintaining security visibility.

```rust
pub struct AlertManager {
    db: redb::Database,
    dedupe_cache: Arc<Mutex<HashMap<String, Instant>>>,
    dedupe_window: Duration,
}

impl AlertManager {
    pub async fn generate_alert(
        &self,
        rule: &DetectionRule,
        process_data: &ProcessRow,
        scan_id: Option<i64>,
    ) -> Result<Option<Alert>> {
        let alert = Alert::new(rule, process_data, scan_id);

        // Check for deduplication
        if self.is_duplicate(&alert).await? {
            return Ok(None);
        }

        // Store alert in database
        self.store_alert(&alert).await?;

        // Update deduplication cache
        self.update_dedupe_cache(&alert).await?;

        Ok(Some(alert))
    }

    async fn is_duplicate(&self, alert: &Alert) -> Result<bool> {
        let mut cache = self.dedupe_cache.lock().await;

        if let Some(last_seen) = cache.get(&alert.dedupe_key) {
            if last_seen.elapsed() < self.dedupe_window {
                return Ok(true);
            }
        }

        Ok(false)
    }

    async fn update_dedupe_cache(&self, alert: &Alert) -> Result<()> {
        let mut cache = self.dedupe_cache.lock().await;
        cache.insert(alert.dedupe_key.clone(), Instant::now());
        Ok(())
    }
}
```

## Multi-Channel Alert Delivery

### Alert Sink Architecture

**Pluggable Sinks**: Flexible alert delivery through multiple channels.

```rust
#[async_trait]
pub trait AlertSink: Send + Sync {
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult>;
    async fn health_check(&self) -> HealthStatus;
    fn name(&self) -> &str;
}

pub struct AlertDeliveryManager {
    sinks: Vec<Box<dyn AlertSink>>,
    retry_policy: RetryPolicy,
    circuit_breaker: CircuitBreaker,
}

impl AlertDeliveryManager {
    pub async fn deliver_alert(&self, alert: &Alert) -> Result<Vec<DeliveryResult>> {
        let mut results = Vec::new();

        // Deliver to all sinks in parallel
        let sink_tasks: Vec<_> = self
            .sinks
            .iter()
            .map(|sink| {
                let alert = alert.clone();
                let sink = sink.as_ref();
                tokio::spawn(async move { self.deliver_to_sink(sink, &alert).await })
            })
            .collect();

        for task in sink_tasks {
            match task.await {
                Ok(result) => results.push(result),
                Err(e) => {
                    tracing::error!("Alert delivery task failed: {}", e);
                    results.push(DeliveryResult::Failed(e.to_string()));
                }
            }
        }

        Ok(results)
    }

    async fn deliver_to_sink(&self, sink: &dyn AlertSink, alert: &Alert) -> DeliveryResult {
        // Apply circuit breaker
        if self.circuit_breaker.is_open(sink.name()) {
            return DeliveryResult::CircuitBreakerOpen;
        }

        // Retry with exponential backoff
        let mut attempt = 0;
        let mut delay = Duration::from_millis(100);

        loop {
            match sink.send(alert).await {
                Ok(result) => {
                    self.circuit_breaker.record_success(sink.name());
                    return result;
                }
                Err(e) => {
                    attempt += 1;
                    if attempt >= self.retry_policy.max_attempts {
                        self.circuit_breaker.record_failure(sink.name());
                        return DeliveryResult::Failed(e.to_string());
                    }

                    tokio::time::sleep(delay).await;
                    delay = std::cmp::min(delay * 2, Duration::from_secs(60));
                }
            }
        }
    }
}
```

### Specific Sink Implementations

**Stdout Sink**:

```rust
pub struct StdoutSink {
    format: OutputFormat,
}

#[async_trait]
impl AlertSink for StdoutSink {
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult> {
        let output = match self.format {
            OutputFormat::Json => serde_json::to_string_pretty(alert)?,
            OutputFormat::Text => self.format_text(alert),
            OutputFormat::Csv => self.format_csv(alert),
        };

        println!("{}", output);
        Ok(DeliveryResult::Success)
    }

    async fn health_check(&self) -> HealthStatus {
        HealthStatus::Healthy
    }

    fn name(&self) -> &str {
        "stdout"
    }
}
```

**Syslog Sink**:

```rust
pub struct SyslogSink {
    facility: SyslogFacility,
    tag: String,
    socket: UnixDatagram,
}

#[async_trait]
impl AlertSink for SyslogSink {
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult> {
        let priority = self.map_severity_to_priority(&alert.severity);
        let timestamp = self.format_timestamp(alert.alert_time);
        let message = format!(
            "<{}>{} {} {}: {}",
            priority, timestamp, self.tag, alert.title, alert.description
        );

        self.socket.send(message.as_bytes()).await?;
        Ok(DeliveryResult::Success)
    }

    async fn health_check(&self) -> HealthStatus {
        // Check if syslog socket is accessible
        HealthStatus::Healthy
    }

    fn name(&self) -> &str {
        "syslog"
    }
}
```

**Webhook Sink**:

```rust
pub struct WebhookSink {
    url: Url,
    client: reqwest::Client,
    headers: HeaderMap,
    timeout: Duration,
}

#[async_trait]
impl AlertSink for WebhookSink {
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult> {
        let payload = serde_json::to_value(alert)?;

        let response = self
            .client
            .post(self.url.clone())
            .headers(self.headers.clone())
            .json(&payload)
            .timeout(self.timeout)
            .send()
            .await?;

        if response.status().is_success() {
            Ok(DeliveryResult::Success)
        } else {
            Err(AlertDeliveryError::HttpError(response.status()))
        }
    }

    async fn health_check(&self) -> HealthStatus {
        // Perform health check by sending a test request
        match self
            .client
            .get(self.url.clone())
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => HealthStatus::Healthy,
            _ => HealthStatus::Unhealthy,
        }
    }

    fn name(&self) -> &str {
        "webhook"
    }
}
```

## Performance Requirements and Optimization

### Process Collection Performance

**Target Metrics**:

- **Process Enumeration**: \<5 seconds for 10,000+ processes
- **CPU Usage**: \<5% sustained during continuous monitoring
- **Memory Usage**: \<100MB resident under normal operation
- **Hash Computation**: Complete within enumeration time

**Optimization Strategies**:

```rust
impl ProcessCollector {
    async fn enumerate_processes_optimized(&self) -> Result<Vec<ProcessRecord>> {
        let start_time = Instant::now();

        // Use parallel processing for hash computation
        let (processes, hash_tasks): (Vec<_>, Vec<_>) = self
            .collect_basic_process_data()
            .into_iter()
            .partition(|p| p.executable_path.is_none());

        // Compute hashes in parallel
        let hash_results =
            futures::future::join_all(hash_tasks.into_iter().map(|p| self.compute_hash_async(p)))
                .await;

        let mut all_processes = processes;
        all_processes.extend(hash_results.into_iter().flatten());

        let duration = start_time.elapsed();
        tracing::info!(
            process_count = all_processes.len(),
            duration_ms = duration.as_millis(),
            "Process enumeration completed"
        );

        Ok(all_processes)
    }
}
```

### Detection Engine Performance

**Target Metrics**:

- **Rule Execution**: \<100ms per detection rule
- **SQL Validation**: \<10ms per query
- **Resource Limits**: 30-second timeout, memory limits
- **Concurrent Execution**: Parallel rule processing

**Optimization Strategies**:

```rust
impl DetectionEngine {
    async fn execute_rules_optimized(&self, scan_id: i64) -> Result<Vec<Alert>> {
        let rules = self.rule_manager.load_enabled_rules().await?;

        // Group rules by complexity for optimal scheduling
        let (simple_rules, complex_rules) = self.categorize_rules(rules);

        // Execute simple rules in parallel
        let simple_alerts = futures::future::join_all(
            simple_rules
                .into_iter()
                .map(|rule| self.execute_rule(rule, scan_id)),
        )
        .await;

        // Execute complex rules sequentially to avoid resource contention
        let mut complex_alerts = Vec::new();
        for rule in complex_rules {
            let alerts = self.execute_rule(rule, scan_id).await?;
            complex_alerts.extend(alerts);
        }

        // Combine results
        let mut all_alerts = Vec::new();
        for result in simple_alerts {
            all_alerts.extend(result?);
        }
        all_alerts.extend(complex_alerts);

        Ok(all_alerts)
    }
}
```

## Error Handling and Recovery

### Graceful Degradation

**Process Collection Failures**:

```rust
impl ProcessCollector {
    async fn enumerate_processes_with_fallback(&self) -> Result<Vec<ProcessRecord>> {
        match self.enumerate_processes_enhanced().await {
            Ok(processes) => Ok(processes),
            Err(e) => {
                tracing::warn!("Enhanced enumeration failed, falling back to basic: {}", e);
                self.enumerate_processes_basic().await
            }
        }
    }
}
```

**Detection Engine Failures**:

```rust
impl DetectionEngine {
    async fn execute_rule_with_recovery(
        &self,
        rule: &DetectionRule,
        scan_id: i64,
    ) -> Result<Vec<Alert>> {
        match self.execute_rule(rule, scan_id).await {
            Ok(alerts) => Ok(alerts),
            Err(e) => {
                tracing::error!(
                    rule_id = %rule.id,
                    error = %e,
                    "Rule execution failed, marking as disabled"
                );

                // Disable problematic rules to prevent repeated failures
                self.rule_manager.disable_rule(&rule.id).await?;
                Ok(Vec::new())
            }
        }
    }
}
```

### Resource Management

**Memory Pressure Handling**:

```rust
impl ProcessCollector {
    async fn handle_memory_pressure(&self) -> Result<()> {
        let memory_usage = self.get_memory_usage()?;

        if memory_usage > self.config.memory_threshold {
            tracing::warn!("Memory pressure detected, reducing batch size");

            // Reduce batch size for hash computation
            self.hash_computer
                .set_buffer_size(self.hash_computer.buffer_size() / 2);

            // Trigger garbage collection
            tokio::task::yield_now().await;
        }

        Ok(())
    }
}
```

---

*This core monitoring specification provides the foundation for DaemonEye's process monitoring capabilities, ensuring high performance, security, and reliability across all supported platforms.*
