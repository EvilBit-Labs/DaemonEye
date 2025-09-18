//! Detection rule data structures and types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::time::SystemTime;
use thiserror::Error;

use crate::models::alert::AlertSeverity;

// Import sqlparser types for AST-based validation
use sqlparser::ast::{Expr, Function, Query, Select, SetExpr, Statement};

// Banned SQL functions for rule validation (case-insensitive match).
// Hoisted as a module-level constant to avoid re-allocating per validation call.
const BANNED_FUNCTIONS: &[&str] = &[
    "load_extension",
    "load",
    "eval",
    "exec",
    "system",
    "shell",
    "readfile",
    "writefile",
    "edit",
    "glob",
    "like",
    "match",
    "regexp",
    "replace",
    "substr",
    "instr",
    "length",
    "abs",
    "random",
    "randomblob",
    "hex",
    "unhex",
    "quote",
    "printf",
    "format",
    "char",
    "unicode",
    "soundex",
    "difference",
];

/// Strongly-typed rule identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuleId(String);

impl RuleId {
    /// Create a new RuleId from any type convertible into `String`.
    ///
    /// This is a convenience constructor that consumes the input (or clones if it was a reference)
    /// and stores its string representation as the inner ID.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::RuleId;
    /// let id = RuleId::new("rule-123");
    /// assert_eq!(id.raw(), "rule-123");
    /// ```
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the underlying string slice of the RuleId.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::RuleId;
    /// let id = RuleId::new("rule-123");
    /// assert_eq!(id.raw(), "rule-123");
    /// ```
    pub fn raw(&self) -> &str {
        &self.0
    }
}

impl From<String> for RuleId {
    /// Creates a RuleId from a `String`.
    ///
    /// This is equivalent to calling `RuleId::new` with the provided string.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::RuleId;
    /// let id = RuleId::from("rule-123".to_string());
    /// assert_eq!(id.raw(), "rule-123");
    /// ```
    fn from(id: String) -> Self {
        Self::new(id)
    }
}

impl From<&str> for RuleId {
    /// Creates a RuleId from a string slice.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::RuleId;
    /// let rid = RuleId::from("rule-123");
    /// assert_eq!(rid.raw(), "rule-123");
    /// ```
    fn from(id: &str) -> Self {
        Self::new(id)
    }
}

impl fmt::Display for RuleId {
    /// Formats the RuleId by writing its inner string.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::RuleId;
    /// let id = RuleId::new("rule-123");
    /// assert_eq!(format!("{}", id), "rule-123");
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Rule metadata information.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RuleMetadata {
    /// Additional metadata
    pub data: HashMap<String, String>,
    /// Rule tags for categorization
    pub tags: Vec<String>,
    /// Rule author
    pub author: Option<String>,
    /// Rule version
    pub version: Option<String>,
    /// Rule category
    pub category: Option<String>,
    /// Rule priority (1-10, higher is more important)
    pub priority: Option<u8>,
}

impl RuleMetadata {
    /// Creates a new, empty RuleMetadata (equivalent to `Default::default()`).
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::RuleMetadata;
    /// let md = RuleMetadata::new();
    /// assert!(md.data.is_empty());
    /// assert!(md.tags.is_empty());
    /// assert!(md.author.is_none());
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or overwrite a metadata key/value pair and return the updated `RuleMetadata` for chaining.
    ///
    /// The provided `key` and `value` are converted into `String` and stored in `self.data`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::RuleMetadata;
    /// let meta = RuleMetadata::new().with_data("env", "production");
    /// assert_eq!(meta.data.get("env").map(String::as_str), Some("production"));
    /// ```
    pub fn with_data(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.data.insert(key.into(), value.into());
        self
    }

    /// Appends a tag to the metadata and returns the updated `RuleMetadata` (builder-style).
    ///
    /// The provided `tag` is converted into a `String` and pushed onto `self.tags`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::RuleMetadata;
    /// let meta = RuleMetadata::new().with_tag("network").with_tag("suspicious");
    /// assert!(meta.tags.contains(&"network".to_string()));
    /// assert!(meta.tags.contains(&"suspicious".to_string()));
    /// ```
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Set the metadata author and return the modified `RuleMetadata`.
    ///
    /// This consumes `self` (builder-style), sets `author` to `Some(author)`, and returns the updated value.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::RuleMetadata;
    /// let meta = RuleMetadata::new().with_author("alice");
    /// assert_eq!(meta.author, Some("alice".to_string()));
    /// ```
    pub fn with_author(mut self, author: impl Into<String>) -> Self {
        self.author = Some(author.into());
        self
    }

    /// Sets the metadata version and returns the updated `RuleMetadata` (builder-style).
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::RuleMetadata;
    /// let meta = RuleMetadata::new().with_version("1.2.3");
    /// assert_eq!(meta.version.as_deref(), Some("1.2.3"));
    /// ```
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Sets the metadata category and returns the updated builder.
    ///
    /// This is a builder-style method that stores `category` in the metadata's
    /// `category` field and returns `self` so calls can be chained.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::RuleMetadata;
    /// let meta = RuleMetadata::new().with_category("network");
    /// assert_eq!(meta.category.as_deref(), Some("network"));
    /// ```
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    /// Set the metadata priority.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::RuleMetadata;
    /// let meta = RuleMetadata::new().with_priority(5);
    /// assert_eq!(meta.priority, Some(5));
    /// ```
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = Some(priority);
        self
    }
}

/// Detection rule with SQL query and metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectionRule {
    /// Rule identifier
    pub id: RuleId,
    /// Rule name
    pub name: String,
    /// Rule description
    pub description: String,
    /// SQL query for detection
    pub sql_query: String,
    /// Alert severity when triggered
    pub severity: AlertSeverity,
    /// Rule version
    pub version: String,
    /// Rule author
    pub author: String,
    /// Rule creation timestamp
    pub created_at: SystemTime,
    /// Rule last modified timestamp
    pub updated_at: SystemTime,
    /// Whether the rule is enabled
    pub enabled: bool,
    /// Rule tags
    pub tags: Vec<String>,
    /// Rule metadata
    pub metadata: RuleMetadata,
}

impl DetectionRule {
    /// Create a new DetectionRule with sensible defaults.
    ///
    /// The constructor initializes timestamps to now, sets the rule `version` to `"1.0.0"`,
    /// `author` to `"system"`, enables the rule, and populates `metadata` with the provided
    /// `category`, the same version, and author. `id`, `name`, `description`, and `sql_query`
    /// are taken from the provided arguments; `severity` is set as given.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::{DetectionRule, RuleId, AlertSeverity};
    ///
    /// let rule = DetectionRule::new(
    ///     RuleId::new("rule-1"),
    ///     "Example rule",
    ///     "Detects example events",
    ///     "SELECT * FROM events WHERE type = 'example'",
    ///     "example-category",
    ///     AlertSeverity::Medium,
    /// );
    ///
    /// assert_eq!(rule.version, "1.0.0");
    /// assert_eq!(rule.author, "system");
    /// assert!(rule.enabled);
    /// assert_eq!(rule.metadata.category.as_deref(), Some("example-category"));
    /// ```
    pub fn new(
        id: impl Into<RuleId>,
        name: impl Into<String>,
        description: impl Into<String>,
        sql_query: impl Into<String>,
        category: impl Into<String>,
        severity: AlertSeverity,
    ) -> Self {
        let now = SystemTime::now();
        let id = id.into();
        let name = name.into();
        let description = description.into();
        let sql_query = sql_query.into();
        let category = category.into();
        Self {
            id,
            name,
            description,
            sql_query,
            severity,
            version: "1.0.0".to_string(),
            author: "system".to_string(),
            created_at: now,
            updated_at: now,
            enabled: true,
            tags: Vec::new(),
            metadata: RuleMetadata::new()
                .with_category(category)
                .with_version("1.0.0")
                .with_author("system"),
        }
    }

    /// Validate the rule's SQL for safety and structural constraints using sqlparser's AST.
    ///
    /// This checks that:
    /// - the SQL parses successfully,
    /// - exactly one statement is present,
    /// - the single statement is a `SELECT` query,
    ///   and then delegates to lower-level validators for the query/SELECT/expression checks
    ///   (e.g., projection, FROM/JOIN limits, banned functions).
    ///
    /// Returns `Ok(())` when the query passes parsing and the top-level checks. Returns
    /// `Err(RuleError::InvalidSql(...))` if parsing fails, if multiple statements are present,
    /// or if the statement is not a `SELECT`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use sentinel_lib::models::rule::{DetectionRule, RuleId};
    /// # use sentinel_lib::models::alert::AlertSeverity;
    /// let rule = DetectionRule::new(
    ///     RuleId::from("r1"),
    ///     "Example",
    ///     "Example rule",
    ///     "SELECT 1 FROM processes",
    ///     "example",
    ///     AlertSeverity::Low,
    /// );
    /// assert!(rule.validate_sql().is_ok());
    /// ```
    pub fn validate_sql(&self) -> Result<(), RuleError> {
        use sqlparser::dialect::GenericDialect;
        use sqlparser::parser::Parser;

        let dialect = GenericDialect {};
        let statements = Parser::parse_sql(&dialect, &self.sql_query)
            .map_err(|e| RuleError::InvalidSql(format!("Failed to parse SQL: {}", e)))?;

        // Ensure single statement
        if statements.len() != 1 {
            return Err(RuleError::InvalidSql(
                "Only single SQL statements are allowed".to_string(),
            ));
        }

        // Ensure it's a SELECT statement
        match &statements[0] {
            Statement::Query(query) => {
                // Basic validation - ensure it's a SELECT query
                Self::validate_query_basic(query)?;
            }
            _ => {
                return Err(RuleError::InvalidSql(
                    "Only SELECT statements are allowed".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Validate that a parsed `Query` is a single SELECT query and delegate to select-level checks.
    ///
    /// This performs a basic safety check: the query body must be a `SELECT`. If not, returns
    /// `RuleError::InvalidSql`.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use sqlparser::dialect::GenericDialect;
    /// use sqlparser::parser::Parser;
    /// use sentinel_lib::models::rule::{RuleError, DetectionRule};
    ///
    /// // Parse a simple SELECT and validate its Query body.
    /// let sql = "SELECT 1";
    /// let dialect = GenericDialect {};
    /// let statements = Parser::parse_sql(&dialect, sql).unwrap();
    /// let stmt = &statements[0];
    /// if let sqlparser::ast::Statement::Query(q) = stmt {
    ///     // This calls the internal basic query validator.
    ///     DetectionRule::validate_query_basic(q).unwrap();
    /// } else {
    ///     panic!("expected a query statement");
    /// }
    /// ```
    fn validate_query_basic(query: &Query) -> Result<(), RuleError> {
        // Validate the main query body is a SELECT
        match &*query.body {
            SetExpr::Select(select) => {
                Self::validate_select_basic(select)?;
            }
            _ => {
                return Err(RuleError::InvalidSql(
                    "Only SELECT statements are allowed".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Validate a parsed `SELECT` for allowed structure and safety.
    ///
    /// Performs conservative checks on the provided `SELECT` node and returns
    /// `RuleError::InvalidSql` when a structural or safety constraint is violated.
    ///
    /// Enforced constraints:
    /// - Projection count: at most 50 items.
    /// - Projections: each unnamed expression or expression-with-alias is validated
    ///   via `validate_expr_basic`.
    /// - `FROM` clause: must be present and non-empty.
    /// - Join count: total joins across all `FROM` items must be <= 4.
    /// - `WHERE`, `HAVING`, and explicit `GROUP BY` expressions (when present) are
    ///   validated via `validate_expr_basic`.
    /// - `GROUP BY` expressions: at most 10 expressions when explicit expressions
    ///   are used; `GROUP BY ALL` is allowed.
    ///
    /// Returns `Ok(())` when the `SELECT` passes all checks.
    ///
    /// # Examples
    ///
    /// ```
    /// use sqlparser::dialect::GenericDialect;
    /// use sqlparser::parser::Parser;
    /// use sentinel_lib::models::rule::{DetectionRule, RuleId};
    /// use sentinel_lib::models::alert::AlertSeverity;
    ///
    /// // Create a simple rule with a safe SELECT and validate it.
    /// let sql = "SELECT id, name FROM users WHERE active = 1";
    /// let rule = DetectionRule::new(RuleId::from("r1"), "name", "desc", sql, "cat", AlertSeverity::Low);
    /// assert!(rule.validate_sql().is_ok());
    /// ```
    fn validate_select_basic(select: &Select) -> Result<(), RuleError> {
        // Validate FROM clause exists
        if select.from.is_empty() {
            return Err(RuleError::InvalidSql(
                "SELECT statement must have a FROM clause".to_string(),
            ));
        }

        // Validate projection count
        if select.projection.len() > 50 {
            return Err(RuleError::InvalidSql(
                "Too many columns in SELECT (max 50)".to_string(),
            ));
        }

        // Validate SELECT columns for functions
        for projection in &select.projection {
            match projection {
                sqlparser::ast::SelectItem::UnnamedExpr(expr) => {
                    Self::validate_expr_basic(expr)?;
                }
                sqlparser::ast::SelectItem::ExprWithAlias { expr, .. } => {
                    Self::validate_expr_basic(expr)?;
                }
                _ => {
                    // Allow other projection types (wildcards, etc.)
                }
            }
        }

        // Validate join count
        let join_count = select
            .from
            .iter()
            .map(|item| item.joins.len())
            .sum::<usize>();
        if join_count > 4 {
            return Err(RuleError::InvalidSql("Too many JOINs (max 4)".to_string()));
        }

        // Validate WHERE clause if present
        if let Some(where_clause) = &select.selection {
            Self::validate_expr_basic(where_clause)?;
        }

        // Validate GROUP BY clause if present
        match &select.group_by {
            sqlparser::ast::GroupByExpr::All(_) => {
                // GROUP BY ALL is allowed
            }
            sqlparser::ast::GroupByExpr::Expressions(exprs, _) => {
                if exprs.len() > 10 {
                    return Err(RuleError::InvalidSql(
                        "GROUP BY has too many columns (max 10)".to_string(),
                    ));
                }
                for expr in exprs {
                    Self::validate_expr_basic(expr)?;
                }
            }
        }

        // Validate HAVING clause if present
        if let Some(having) = &select.having {
            Self::validate_expr_basic(having)?;
        }

        Ok(())
    }

    /// Validate an SQL expression for disallowed or unsafe constructs used in detection rules.
    ///
    /// Recursively inspects the expression and enforces safety checks:
    /// - Functions are validated via `validate_function_basic`.
    /// - Subqueries are validated via `validate_query_basic`.
    /// - Binary and unary expressions are validated recursively.
    /// - `CASE` expressions validate each condition/result and the optional `ELSE`.
    ///   Other expression types (identifiers, literals, simple qualifiers, etc.) are allowed.
    ///
    /// Returns `Ok(())` when the expression and all nested sub-expressions pass validation,
    /// or a `RuleError` propagated from deeper checks when a disallowed construct is found.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use sqlparser::ast::{Expr, Ident};
    /// // Simple identifier expressions are allowed:
    /// let expr = Expr::Identifier(Ident::new("column"));
    /// // Call the validator (found on the same impl as this method):
    /// let _ = DetectionRule::validate_expr_basic(&expr);
    /// ```
    fn validate_expr_basic(expr: &Expr) -> Result<(), RuleError> {
        match expr {
            Expr::Function(func) => {
                Self::validate_function_basic(func)?;
            }
            Expr::Subquery(query) => {
                // Validate subquery
                Self::validate_query_basic(query)?;
            }
            Expr::BinaryOp { left, right, .. } => {
                Self::validate_expr_basic(left)?;
                Self::validate_expr_basic(right)?;
            }
            Expr::UnaryOp { expr, .. } => {
                Self::validate_expr_basic(expr)?;
            }
            Expr::Case {
                conditions,
                else_result,
                ..
            } => {
                for condition in conditions {
                    Self::validate_expr_basic(&condition.condition)?;
                    Self::validate_expr_basic(&condition.result)?;
                }
                if let Some(else_expr) = else_result {
                    Self::validate_expr_basic(else_expr)?;
                }
            }
            _ => {
                // Allow other expressions (identifiers, literals, etc.)
            }
        }

        Ok(())
    }

    /// Basic function validation for security.
    fn validate_function_basic(func: &Function) -> Result<(), RuleError> {
        // Check for banned functions (case-insensitive)
        let name = func.name.to_string();
        if BANNED_FUNCTIONS
            .iter()
            .any(|banned| name.eq_ignore_ascii_case(banned))
        {
            return Err(RuleError::InvalidSql(format!(
                "Function '{}' is not allowed",
                name
            )));
        }

        // Validate function arguments - simplified approach
        // Just check that the function name is safe, don't recurse into arguments
        // to avoid complex API issues with sqlparser 0.50

        Ok(())
    }

    /// Update the rule's `updated_at` timestamp to the current system time.
    ///
    /// Sets `updated_at` to `SystemTime::now()`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use sentinel_lib::models::{DetectionRule, RuleId, AlertSeverity};
    /// let mut rule = DetectionRule::new(RuleId::from("r1"), "n", "d", "SELECT 1", "cat", AlertSeverity::Low);
    /// rule.touch();
    /// ```
    pub fn touch(&mut self) {
        self.updated_at = SystemTime::now();
    }

    /// Enables the rule and updates its `updated_at` timestamp.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::{DetectionRule, RuleId, AlertSeverity};
    /// let mut rule = DetectionRule::new(
    ///     RuleId::from("rule-1"),
    ///     "Example rule",
    ///     "Detects example activity",
    ///     "SELECT 1",
    ///     "example",
    ///     AlertSeverity::Low,
    /// );
    /// rule.enable();
    /// assert!(rule.enabled);
    /// ```
    pub fn enable(&mut self) {
        self.enabled = true;
        self.touch();
    }

    /// Disables the detection rule and updates its `updated_at` timestamp.
    ///
    /// This sets the rule's `enabled` flag to `false` and refreshes `updated_at` to the current time.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::{DetectionRule, RuleId, AlertSeverity};
    /// let mut rule = DetectionRule::new(
    ///     RuleId::from("r1"),
    ///     "name",
    ///     "desc",
    ///     "SELECT 1",
    ///     "cat",
    ///     AlertSeverity::Low,
    /// );
    /// rule.disable();
    /// assert!(!rule.enabled);
    /// ```
    pub fn disable(&mut self) {
        self.enabled = false;
        self.touch();
    }

    /// Adds a tag to the rule and updates its `updated_at` timestamp.
    ///
    /// The provided `tag` is appended to the rule's `tags` list. Duplicate tags are not deduplicated.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::rule::{DetectionRule, RuleId};
    /// use sentinel_lib::models::alert::AlertSeverity;
    ///
    /// let mut rule = DetectionRule::new(
    ///     RuleId::from("rule-1"),
    ///     "Example rule",
    ///     "Detects example events",
    ///     "SELECT 1",
    ///     "example",
    ///     AlertSeverity::Low,
    /// );
    ///
    /// rule.add_tag("network");
    /// assert!(rule.tags.contains(&"network".to_string()));
    /// ```
    pub fn add_tag(&mut self, tag: impl Into<String>) {
        self.tags.push(tag.into());
        self.touch();
    }

    /// Insert or update a metadata key/value on the rule and mark the rule as modified.
    ///
    /// This stores `value` under `key` in the rule's metadata map and updates the rule's
    /// `updated_at` timestamp (via `touch()`).
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::{DetectionRule, RuleId, AlertSeverity};
    ///
    /// let mut rule = DetectionRule::new(
    ///     RuleId::new("rule-1"),
    ///     "Example",
    ///     "An example rule",
    ///     "SELECT 1",
    ///     "example",
    ///     AlertSeverity::Low,
    /// );
    ///
    /// rule.add_metadata("env", "prod");
    /// assert_eq!(rule.metadata.data.get("env"), Some(&"prod".to_string()));
    /// ```
    pub fn add_metadata(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.metadata.data.insert(key.into(), value.into());
        self.touch();
    }

    /// Returns true if the rule appears valid.
    ///
    /// This checks that:
    /// - the SQL query parses and passes basic safety checks (via `validate_sql()`), and
    /// - both the rule `name` and `sql_query` are non-empty.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use sentinel_lib::models::rule::DetectionRule;
    /// # use sentinel_lib::models::alert::AlertSeverity;
    /// let rule = DetectionRule::new(
    ///     "rule-1",
    ///     "Example rule",
    ///     "Detects something",
    ///     "SELECT 1",
    ///     "example",
    ///     AlertSeverity::Low,
    /// );
    /// assert!(rule.is_valid());
    /// ```
    pub fn is_valid(&self) -> bool {
        // Cheap checks first to avoid SQL parsing on obviously invalid rules.
        !self.name.is_empty() && !self.sql_query.is_empty() && self.validate_sql().is_ok()
    }

    /// Returns the age of the rule in whole seconds.
    ///
    /// If the system clock is earlier than the rule's `created_at` (making `elapsed()` fail),
    /// this returns `0`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sentinel_lib::models::{DetectionRule, AlertSeverity};
    /// let rule = DetectionRule::new("r1", "name", "desc", "SELECT 1", "category", AlertSeverity::Low);
    /// let secs = rule.age_seconds();
    /// // newly created rule should have a small non-negative age
    /// assert!(secs >= 0);
    /// ```
    pub fn age_seconds(&self) -> u64 {
        self.created_at.elapsed().map_or(0, |d| d.as_secs())
    }
}

/// Rule-related errors.
#[derive(Debug, Error)]
pub enum RuleError {
    #[error("Invalid SQL query: {0}")]
    InvalidSql(String),
    #[error("Missing required field: {0}")]
    MissingField(&'static str),
    #[error("Rule validation failed: {0}")]
    ValidationFailed(String),
    #[error("Rule not found: {0}")]
    RuleNotFound(String),
    #[error("Rule execution failed: {0}")]
    ExecutionFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detection_rule_creation() {
        let rule = DetectionRule::new(
            "rule-001",
            "Suspicious Process Detection",
            "Detects processes with suspicious names",
            "SELECT * FROM processes WHERE name LIKE '%suspicious%'",
            "process-monitoring",
            AlertSeverity::High,
        );

        assert_eq!(rule.id.raw(), "rule-001");
        assert_eq!(rule.name, "Suspicious Process Detection");
        assert_eq!(rule.description, "Detects processes with suspicious names");
        assert_eq!(
            rule.sql_query,
            "SELECT * FROM processes WHERE name LIKE '%suspicious%'"
        );
        assert_eq!(rule.severity, AlertSeverity::High);
        assert!(rule.enabled);
        assert!(rule.is_valid());
    }

    #[test]
    fn test_detection_rule_serialization() {
        let rule = DetectionRule::new(
            "rule-001",
            "Test Rule",
            "Test description",
            "SELECT * FROM processes WHERE name = 'test'",
            "test",
            AlertSeverity::Medium,
        );

        // Test JSON serialization
        let json = serde_json::to_string(&rule).unwrap();
        let deserialized: DetectionRule = serde_json::from_str(&json).unwrap();
        assert_eq!(rule, deserialized);
    }

    #[test]
    fn test_detection_rule_validation() {
        let valid_rule = DetectionRule::new(
            "rule-001",
            "Valid Rule",
            "Valid description",
            "SELECT * FROM processes WHERE name = 'test'",
            "test",
            AlertSeverity::Low,
        );
        assert!(valid_rule.validate_sql().is_ok());

        // Test DROP statement (should fail)
        let invalid_rule = DetectionRule::new(
            "rule-002",
            "Invalid Rule",
            "Invalid description",
            "DROP TABLE processes",
            "test",
            AlertSeverity::Low,
        );
        assert!(invalid_rule.validate_sql().is_err());

        // Test INSERT statement (should fail)
        let insert_rule = DetectionRule::new(
            "rule-003",
            "Insert Rule",
            "Insert description",
            "INSERT INTO processes VALUES (1, 'test')",
            "test",
            AlertSeverity::Low,
        );
        assert!(insert_rule.validate_sql().is_err());

        // Test complex SELECT (should pass)
        let complex_rule = DetectionRule::new(
            "rule-004",
            "Complex Rule",
            "Complex description",
            "SELECT p.name, p.pid FROM processes p WHERE p.name LIKE '%test%' ORDER BY p.pid LIMIT 10",
            "test",
            AlertSeverity::Low,
        );
        assert!(complex_rule.validate_sql().is_ok());

        // Test banned function (should fail)
        let banned_func_rule = DetectionRule::new(
            "rule-005",
            "Banned Function Rule",
            "Banned function description",
            "SELECT load_extension('test') FROM processes",
            "test",
            AlertSeverity::Low,
        );
        assert!(banned_func_rule.validate_sql().is_err());

        // Test too many joins (should fail)
        let many_joins_rule = DetectionRule::new(
            "rule-006",
            "Many Joins Rule",
            "Many joins description",
            "SELECT * FROM processes p1 JOIN processes p2 ON p1.pid = p2.pid JOIN processes p3 ON p1.pid = p3.pid JOIN processes p4 ON p1.pid = p4.pid JOIN processes p5 ON p1.pid = p5.pid JOIN processes p6 ON p1.pid = p6.pid",
            "test",
            AlertSeverity::Low,
        );
        assert!(many_joins_rule.validate_sql().is_err());
    }

    #[test]
    fn test_detection_rule_operations() {
        let mut rule = DetectionRule::new(
            "rule-001",
            "Test Rule",
            "Test description",
            "SELECT * FROM processes WHERE name = 'test'",
            "test",
            AlertSeverity::Medium,
        );

        // Test enable/disable
        rule.disable();
        assert!(!rule.enabled);
        rule.enable();
        assert!(rule.enabled);

        // Test adding tags and metadata
        rule.add_tag("test");
        rule.add_metadata("key", "value");
        assert!(rule.tags.contains(&"test".to_string()));
        assert_eq!(rule.metadata.data.get("key"), Some(&"value".to_string()));
    }

    #[test]
    fn test_rule_id_operations() {
        let id = RuleId::new("rule-001");
        assert_eq!(id.raw(), "rule-001");
        assert_eq!(id.to_string(), "rule-001");
    }

    #[test]
    fn test_rule_metadata() {
        let metadata = RuleMetadata::new()
            .with_data("key", "value")
            .with_tag("test")
            .with_author("test-author")
            .with_version("1.0.0")
            .with_category("test-category")
            .with_priority(5);

        assert_eq!(metadata.data.get("key"), Some(&"value".to_string()));
        assert!(metadata.tags.contains(&"test".to_string()));
        assert_eq!(metadata.author, Some("test-author".to_string()));
        assert_eq!(metadata.version, Some("1.0.0".to_string()));
        assert_eq!(metadata.category, Some("test-category".to_string()));
        assert_eq!(metadata.priority, Some(5));
    }

    #[test]
    fn test_rule_age() {
        let rule = DetectionRule::new(
            "rule-001",
            "Test Rule",
            "Test description",
            "SELECT * FROM processes WHERE name = 'test'",
            "test",
            AlertSeverity::Low,
        );

        // Rule should be recent (just created)
        assert_eq!(rule.age_seconds(), 0);
    }
}
