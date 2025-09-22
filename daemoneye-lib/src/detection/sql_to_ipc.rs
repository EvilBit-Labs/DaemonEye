//! SQL-to-IPC translation pipeline for capability-aware task routing.
//!
//! This module provides functionality to analyze SQL detection rules and translate
//! them into appropriate IPC tasks based on collector capabilities.

use crate::{
    ipc::client::ResilientIpcClient,
    models::DetectionRule,
    proto::{DetectionTask, MonitoringDomain, ProcessFilter, TaskType},
};
use anyhow::{Context, Result};
use sqlparser::{ast::Statement, dialect::GenericDialect, parser::Parser};
use std::collections::HashSet;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Requirements extracted from SQL analysis
#[derive(Debug, Clone)]
pub struct CollectionRequirements {
    /// Required monitoring domains
    pub required_domains: HashSet<MonitoringDomain>,
    /// Process-specific requirements
    pub process_requirements: Option<ProcessRequirements>,
    /// Network-specific requirements (future)
    pub network_requirements: Option<NetworkRequirements>,
    /// Filesystem-specific requirements (future)
    pub filesystem_requirements: Option<FilesystemRequirements>,
    /// Performance-specific requirements (future)
    pub performance_requirements: Option<PerformanceRequirements>,
    /// Whether real-time monitoring is needed
    pub requires_realtime: bool,
    /// Whether system-wide monitoring is needed
    pub requires_system_wide: bool,
    /// Whether kernel-level access is needed
    pub requires_kernel_level: bool,
}

/// Process monitoring requirements
#[derive(Debug, Clone)]
pub struct ProcessRequirements {
    /// Specific process names to monitor
    pub process_names: Vec<String>,
    /// Specific PIDs to monitor
    pub pids: Vec<u32>,
    /// Executable path patterns
    pub executable_patterns: Vec<String>,
    /// Whether hash verification is needed
    pub needs_hash_verification: bool,
    /// Whether CPU usage monitoring is needed
    pub needs_cpu_monitoring: bool,
    /// Whether memory usage monitoring is needed
    pub needs_memory_monitoring: bool,
}

/// Network monitoring requirements (future implementation)
#[derive(Debug, Clone)]
pub struct NetworkRequirements {
    /// Protocols to monitor
    pub protocols: Vec<String>,
    /// Source addresses/ports
    pub source_addresses: Vec<String>,
    /// Destination addresses/ports
    pub destination_addresses: Vec<String>,
}

/// Filesystem monitoring requirements (future implementation)
#[derive(Debug, Clone)]
pub struct FilesystemRequirements {
    /// Path patterns to monitor
    pub path_patterns: Vec<String>,
    /// Operation types to monitor
    pub operation_types: Vec<String>,
    /// File extensions to monitor
    pub file_extensions: Vec<String>,
}

/// Performance monitoring requirements (future implementation)
#[derive(Debug, Clone)]
pub struct PerformanceRequirements {
    /// Metric names to collect
    pub metric_names: Vec<String>,
    /// System components to monitor
    pub components: Vec<String>,
    /// Threshold values
    pub thresholds: std::collections::HashMap<String, f64>,
}

/// SQL-to-IPC translation engine
pub struct SqlToIpcTranslator {
    client: ResilientIpcClient,
}

impl SqlToIpcTranslator {
    /// Create a new SQL-to-IPC translator
    pub const fn new(client: ResilientIpcClient) -> Self {
        Self { client }
    }

    /// Analyze a SQL detection rule and extract collection requirements
    #[allow(clippy::wildcard_enum_match_arm, clippy::needless_borrowed_reference)]
    pub fn analyze_sql_requirements(&self, rule: &DetectionRule) -> Result<CollectionRequirements> {
        let dialect = GenericDialect {};
        let statements =
            Parser::parse_sql(&dialect, &rule.sql_query).context("Failed to parse SQL query")?;

        if statements.is_empty() {
            anyhow::bail!("No SQL statements found in rule");
        }

        // For now, we'll implement basic analysis for the first statement
        let statement = statements
            .first()
            .ok_or_else(|| anyhow::anyhow!("No statements found"))?;

        match statement {
            &Statement::Query(ref query) => Self::analyze_query_requirements(query, rule),
            _ => {
                anyhow::bail!("Only SELECT queries are supported for detection rules");
            }
        }
    }

    /// Analyze a SQL query and extract requirements
    fn analyze_query_requirements(
        query: &sqlparser::ast::Query,
        rule: &DetectionRule,
    ) -> Result<CollectionRequirements> {
        let mut requirements = CollectionRequirements {
            required_domains: HashSet::new(),
            process_requirements: None,
            network_requirements: None,
            filesystem_requirements: None,
            performance_requirements: None,
            requires_realtime: false,
            requires_system_wide: true, // Default to system-wide for security monitoring
            requires_kernel_level: false,
        };

        // Analyze the query body
        if let sqlparser::ast::SetExpr::Select(ref select) = *query.body {
            // Analyze FROM clause to determine required domains
            for table in &select.from {
                if let sqlparser::ast::TableFactor::Table { ref name, .. } = table.relation {
                    let table_name = name.to_string().to_lowercase();

                    match table_name.as_str() {
                        "processes" | "process" => {
                            requirements
                                .required_domains
                                .insert(MonitoringDomain::Process);
                            requirements.process_requirements =
                                Some(Self::analyze_process_requirements(select, rule));
                        }
                        "network" | "connections" => {
                            requirements
                                .required_domains
                                .insert(MonitoringDomain::Network);
                            // Future: analyze network requirements
                        }
                        "filesystem" | "files" => {
                            requirements
                                .required_domains
                                .insert(MonitoringDomain::Filesystem);
                            // Future: analyze filesystem requirements
                        }
                        "performance" | "metrics" => {
                            requirements
                                .required_domains
                                .insert(MonitoringDomain::Performance);
                            // Future: analyze performance requirements
                        }
                        _ => {
                            warn!(table_name = %table_name, "Unknown table in SQL query, assuming process monitoring");
                            requirements
                                .required_domains
                                .insert(MonitoringDomain::Process);
                            requirements.process_requirements = Some(ProcessRequirements {
                                process_names: vec![],
                                pids: vec![],
                                executable_patterns: vec![],
                                needs_hash_verification: false,
                                needs_cpu_monitoring: false,
                                needs_memory_monitoring: false,
                            });
                        }
                    }
                }
            }

            // Analyze WHERE clause for specific filtering requirements
            if let Some(ref where_clause) = select.selection {
                Self::analyze_where_clause(where_clause, &mut requirements)?;
            }

            // Analyze SELECT clause for required fields
            Self::analyze_select_clause(&select.projection, &mut requirements);
        }

        // If no domains were identified, default to process monitoring
        if requirements.required_domains.is_empty() {
            requirements
                .required_domains
                .insert(MonitoringDomain::Process);
            requirements.process_requirements = Some(ProcessRequirements {
                process_names: vec![],
                pids: vec![],
                executable_patterns: vec![],
                needs_hash_verification: false,
                needs_cpu_monitoring: false,
                needs_memory_monitoring: false,
            });
        }

        debug!(
            rule_id = %rule.id.raw(),
            required_domains = ?requirements.required_domains,
            "Analyzed SQL requirements"
        );

        Ok(requirements)
    }

    /// Analyze process-specific requirements from SQL
    #[allow(clippy::wildcard_enum_match_arm)]
    fn analyze_process_requirements(
        select: &sqlparser::ast::Select,
        _rule: &DetectionRule,
    ) -> ProcessRequirements {
        let mut requirements = ProcessRequirements {
            process_names: vec![],
            pids: vec![],
            executable_patterns: vec![],
            needs_hash_verification: false,
            needs_cpu_monitoring: false,
            needs_memory_monitoring: false,
        };

        // Analyze SELECT clause for required fields
        for projection in &select.projection {
            if let sqlparser::ast::SelectItem::UnnamedExpr(sqlparser::ast::Expr::Identifier(
                ref ident,
            )) = *projection
            {
                match ident.value.to_lowercase().as_str() {
                    "executable_hash" | "hash" => {
                        requirements.needs_hash_verification = true;
                    }
                    "cpu_usage" | "cpu" => {
                        requirements.needs_cpu_monitoring = true;
                    }
                    "memory_usage" | "memory" => {
                        requirements.needs_memory_monitoring = true;
                    }
                    _ => {}
                }
            }
        }

        requirements
    }

    /// Analyze WHERE clause for filtering requirements
    #[allow(clippy::wildcard_enum_match_arm)]
    fn analyze_where_clause(
        where_clause: &sqlparser::ast::Expr,
        requirements: &mut CollectionRequirements,
    ) -> Result<()> {
        // This is a simplified analysis - a full implementation would recursively
        // analyze the entire WHERE clause AST
        match *where_clause {
            sqlparser::ast::Expr::BinaryOp {
                ref left,
                op: _,
                ref right,
            } => {
                // Analyze both sides of binary operations
                Self::analyze_where_clause(left, requirements)?;
                Self::analyze_where_clause(right, requirements)?;
            }
            sqlparser::ast::Expr::Identifier(ref ident) => {
                // Check for fields that require specific capabilities
                match ident.value.to_lowercase().as_str() {
                    "executable_hash" | "hash" => {
                        if let Some(ref mut proc_req) = requirements.process_requirements {
                            proc_req.needs_hash_verification = true;
                        }
                    }
                    "cpu_usage" | "cpu" => {
                        if let Some(ref mut proc_req) = requirements.process_requirements {
                            proc_req.needs_cpu_monitoring = true;
                        }
                    }
                    "memory_usage" | "memory" => {
                        if let Some(ref mut proc_req) = requirements.process_requirements {
                            proc_req.needs_memory_monitoring = true;
                        }
                    }
                    _ => {}
                }
            }
            _ => {
                // Handle other expression types as needed
            }
        }

        Ok(())
    }

    /// Analyze SELECT clause for required fields
    #[allow(clippy::wildcard_enum_match_arm)]
    fn analyze_select_clause(
        projections: &[sqlparser::ast::SelectItem],
        requirements: &mut CollectionRequirements,
    ) {
        for projection in projections {
            match *projection {
                sqlparser::ast::SelectItem::UnnamedExpr(sqlparser::ast::Expr::Identifier(
                    ref ident,
                )) => match ident.value.to_lowercase().as_str() {
                    "executable_hash" | "hash" => {
                        if let Some(ref mut proc_req) = requirements.process_requirements {
                            proc_req.needs_hash_verification = true;
                        }
                    }
                    "cpu_usage" | "cpu" => {
                        if let Some(ref mut proc_req) = requirements.process_requirements {
                            proc_req.needs_cpu_monitoring = true;
                        }
                    }
                    "memory_usage" | "memory" => {
                        if let Some(ref mut proc_req) = requirements.process_requirements {
                            proc_req.needs_memory_monitoring = true;
                        }
                    }
                    _ => {}
                },
                sqlparser::ast::SelectItem::Wildcard(_) => {
                    // SELECT * requires all available fields
                    if let Some(ref mut proc_req) = requirements.process_requirements {
                        proc_req.needs_hash_verification = true;
                        proc_req.needs_cpu_monitoring = true;
                        proc_req.needs_memory_monitoring = true;
                    }
                }
                _ => {
                    // Handle other projection types as needed
                }
            }
        }
    }

    /// Translate collection requirements into IPC tasks
    pub fn translate_to_tasks(
        &self,
        requirements: &CollectionRequirements,
    ) -> Result<Vec<DetectionTask>> {
        let mut tasks = Vec::new();

        // Generate tasks for each required domain
        for domain in &requirements.required_domains {
            match *domain {
                MonitoringDomain::Process => {
                    if let Some(ref proc_req) = requirements.process_requirements {
                        tasks.extend(Self::create_process_tasks(proc_req));
                    }
                }
                MonitoringDomain::Network => {
                    // Future: create network tasks
                    info!("Network monitoring tasks not yet implemented");
                }
                MonitoringDomain::Filesystem => {
                    // Future: create filesystem tasks
                    info!("Filesystem monitoring tasks not yet implemented");
                }
                MonitoringDomain::Performance => {
                    // Future: create performance tasks
                    info!("Performance monitoring tasks not yet implemented");
                }
            }
        }

        if tasks.is_empty() {
            // Create a default process enumeration task
            tasks.push(DetectionTask {
                task_id: format!("default-process-enum-{}", Uuid::new_v4()),
                task_type: i32::from(TaskType::EnumerateProcesses),
                process_filter: None,
                hash_check: None,
                metadata: Some("Default process enumeration".to_owned()),
                network_filter: None,
                filesystem_filter: None,
                performance_filter: None,
            });
        }

        debug!(
            task_count = tasks.len(),
            "Generated IPC tasks from requirements"
        );
        Ok(tasks)
    }

    /// Create process monitoring tasks
    fn create_process_tasks(requirements: &ProcessRequirements) -> Vec<DetectionTask> {
        let mut tasks = Vec::new();

        // Create basic process enumeration task
        let process_filter = (!requirements.process_names.is_empty()
            || !requirements.pids.is_empty())
        .then(|| ProcessFilter {
            process_names: requirements.process_names.clone(),
            pids: requirements.pids.clone(),
            executable_pattern: requirements.executable_patterns.first().cloned(),
        });

        tasks.push(DetectionTask {
            task_id: format!("process-enum-{}", Uuid::new_v4()),
            task_type: i32::from(TaskType::EnumerateProcesses),
            process_filter: process_filter.clone(),
            hash_check: None,
            metadata: Some("Process enumeration for detection rule".to_owned()),
            network_filter: None,
            filesystem_filter: None,
            performance_filter: None,
        });

        // Add hash verification task if needed
        if requirements.needs_hash_verification {
            tasks.push(DetectionTask {
                task_id: format!("hash-verify-{}", Uuid::new_v4()),
                task_type: i32::from(TaskType::VerifyExecutable),
                process_filter,
                hash_check: None, // Will be populated with specific hash checks
                metadata: Some("Hash verification for detection rule".to_owned()),
                network_filter: None,
                filesystem_filter: None,
                performance_filter: None,
            });
        }

        tasks
    }

    /// Execute a detection rule with capability-aware task routing
    pub async fn execute_rule_with_routing(
        &self,
        rule: &DetectionRule,
    ) -> Result<Vec<DetectionTask>> {
        info!(rule_id = %rule.id.raw(), "Executing rule with capability-aware routing");

        // Analyze SQL requirements
        let requirements = self
            .analyze_sql_requirements(rule)
            .context("Failed to analyze SQL requirements")?;

        // Check if we have compatible endpoints
        for domain in &requirements.required_domains {
            let task_type = match *domain {
                MonitoringDomain::Process => TaskType::EnumerateProcesses,
                MonitoringDomain::Network => TaskType::MonitorNetworkConnections,
                MonitoringDomain::Filesystem => TaskType::TrackFileOperations,
                MonitoringDomain::Performance => TaskType::CollectPerformanceMetrics,
            };

            let compatible_endpoint = self.client.select_endpoint_for_task(task_type).await;
            if compatible_endpoint.is_none() {
                warn!(
                    rule_id = %rule.id.raw(),
                    required_domain = ?domain,
                    "No compatible endpoints found for required monitoring domain"
                );
            }
        }

        // Translate requirements to tasks
        let tasks = self
            .translate_to_tasks(&requirements)
            .context("Failed to translate requirements to tasks")?;

        info!(
            rule_id = %rule.id.raw(),
            task_count = tasks.len(),
            "Generated tasks for rule execution"
        );

        Ok(tasks)
    }
}

#[cfg(test)]
#[allow(clippy::str_to_string, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{
        ipc::{IpcConfig, TransportType},
        models::{AlertSeverity, DetectionRule},
    };

    fn create_test_client() -> ResilientIpcClient {
        let config = IpcConfig {
            transport: TransportType::Interprocess,
            endpoint_path: "/tmp/test.sock".to_string(),
            max_frame_bytes: 1024 * 1024,
            accept_timeout_ms: 1000,
            read_timeout_ms: 5000,
            write_timeout_ms: 5000,
            max_connections: 10,
            panic_strategy: crate::ipc::PanicStrategy::Unwind,
        };
        ResilientIpcClient::new(&config)
    }

    #[tokio::test]
    async fn test_basic_process_query_analysis() {
        let client = create_test_client();
        let translator = SqlToIpcTranslator::new(client);

        let rule = DetectionRule::new(
            "test-rule".to_string(),
            "Test Rule".to_string(),
            "Test process monitoring rule".to_string(),
            "SELECT pid, name, executable_path FROM processes WHERE name = 'suspicious'"
                .to_string(),
            "test".to_string(),
            AlertSeverity::Medium,
        );

        let requirements = translator.analyze_sql_requirements(&rule).unwrap();

        assert!(
            requirements
                .required_domains
                .contains(&MonitoringDomain::Process)
        );
        assert!(requirements.process_requirements.is_some());
        assert!(!requirements.requires_kernel_level);
        assert!(requirements.requires_system_wide);
    }

    #[tokio::test]
    async fn test_hash_verification_requirement() {
        let client = create_test_client();
        let translator = SqlToIpcTranslator::new(client);

        let rule = DetectionRule::new(
            "hash-rule".to_string(),
            "Hash Verification Rule".to_string(),
            "Rule requiring hash verification".to_string(),
            "SELECT pid, name, executable_hash FROM processes WHERE executable_hash IS NOT NULL"
                .to_string(),
            "test".to_string(),
            AlertSeverity::High,
        );

        let requirements = translator.analyze_sql_requirements(&rule).unwrap();

        assert!(
            requirements
                .required_domains
                .contains(&MonitoringDomain::Process)
        );
        let proc_req = requirements.process_requirements.unwrap();
        assert!(proc_req.needs_hash_verification);
    }

    #[tokio::test]
    async fn test_cpu_monitoring_requirement() {
        let client = create_test_client();
        let translator = SqlToIpcTranslator::new(client);

        let rule = DetectionRule::new(
            "cpu-rule".to_string(),
            "CPU Monitoring Rule".to_string(),
            "Rule requiring CPU monitoring".to_string(),
            "SELECT pid, name, cpu_usage FROM processes WHERE cpu_usage > 80".to_string(),
            "test".to_string(),
            AlertSeverity::Medium,
        );

        let requirements = translator.analyze_sql_requirements(&rule).unwrap();

        assert!(
            requirements
                .required_domains
                .contains(&MonitoringDomain::Process)
        );
        let proc_req = requirements.process_requirements.unwrap();
        assert!(proc_req.needs_cpu_monitoring);
    }

    #[tokio::test]
    async fn test_wildcard_select_requirements() {
        let client = create_test_client();
        let translator = SqlToIpcTranslator::new(client);

        let rule = DetectionRule::new(
            "wildcard-rule".to_string(),
            "Wildcard Rule".to_string(),
            "Rule with wildcard select".to_string(),
            "SELECT * FROM processes WHERE name LIKE '%malware%'".to_string(),
            "test".to_string(),
            AlertSeverity::Critical,
        );

        let requirements = translator.analyze_sql_requirements(&rule).unwrap();

        assert!(
            requirements
                .required_domains
                .contains(&MonitoringDomain::Process)
        );
        let proc_req = requirements.process_requirements.unwrap();
        assert!(proc_req.needs_hash_verification);
        assert!(proc_req.needs_cpu_monitoring);
        assert!(proc_req.needs_memory_monitoring);
    }

    #[tokio::test]
    async fn test_task_generation() {
        let client = create_test_client();
        let translator = SqlToIpcTranslator::new(client);

        let requirements = CollectionRequirements {
            required_domains: {
                let mut domains = HashSet::new();
                domains.insert(MonitoringDomain::Process);
                domains
            },
            process_requirements: Some(ProcessRequirements {
                process_names: vec!["test".to_string()],
                pids: vec![],
                executable_patterns: vec![],
                needs_hash_verification: true,
                needs_cpu_monitoring: false,
                needs_memory_monitoring: false,
            }),
            network_requirements: None,
            filesystem_requirements: None,
            performance_requirements: None,
            requires_realtime: false,
            requires_system_wide: true,
            requires_kernel_level: false,
        };

        let tasks = translator.translate_to_tasks(&requirements).unwrap();

        assert_eq!(tasks.len(), 2); // Process enumeration + hash verification
        assert!(
            tasks
                .iter()
                .any(|t| t.task_type == i32::from(TaskType::EnumerateProcesses))
        );
        assert!(
            tasks
                .iter()
                .any(|t| t.task_type == i32::from(TaskType::VerifyExecutable))
        );
    }

    #[tokio::test]
    async fn test_invalid_sql_handling() {
        let client = create_test_client();
        let translator = SqlToIpcTranslator::new(client);

        let rule = DetectionRule::new(
            "invalid-rule".to_string(),
            "Invalid Rule".to_string(),
            "Rule with invalid SQL".to_string(),
            "INVALID SQL QUERY".to_string(),
            "test".to_string(),
            AlertSeverity::Medium,
        );

        let result = translator.analyze_sql_requirements(&rule);
        assert!(result.is_err());
    }
}
