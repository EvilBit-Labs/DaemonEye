//! Detection rule data structures and types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::time::SystemTime;
use thiserror::Error;

use crate::models::alert::AlertSeverity;

use crate::config::DetectionConfig;
use crate::detection::rejection::{RegexRejection, SqlRejection};
use crate::detection::sql_validation::validate_detection_sql;

/// Strongly-typed rule identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuleId(String);

impl RuleId {
    /// Create a new `RuleId` from any type convertible into `String`.
    ///
    /// This is a convenience constructor that consumes the input (or clones if it was a reference)
    /// and stores its string representation as the inner ID.
    ///
    /// # Examples
    ///
    /// ```
    /// use daemoneye_lib::models::RuleId;
    /// let id = RuleId::new("rule-123");
    /// assert_eq!(id.raw(), "rule-123");
    /// ```
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the underlying string slice of the `RuleId`.
    ///
    /// # Examples
    ///
    /// ```
    /// use daemoneye_lib::models::RuleId;
    /// let id = RuleId::new("rule-123");
    /// assert_eq!(id.raw(), "rule-123");
    /// ```
    pub fn raw(&self) -> &str {
        &self.0
    }
}

impl From<String> for RuleId {
    /// Creates a `RuleId` from a `String`.
    ///
    /// This is equivalent to calling `RuleId::new` with the provided string.
    ///
    /// # Examples
    ///
    /// ```
    /// use daemoneye_lib::models::RuleId;
    /// let id = RuleId::from("rule-123".to_string());
    /// assert_eq!(id.raw(), "rule-123");
    /// ```
    fn from(id: String) -> Self {
        Self::new(id)
    }
}

impl From<&str> for RuleId {
    /// Creates a `RuleId` from a string slice.
    ///
    /// # Examples
    ///
    /// ```
    /// use daemoneye_lib::models::RuleId;
    /// let rid = RuleId::from("rule-123");
    /// assert_eq!(rid.raw(), "rule-123");
    /// ```
    fn from(id: &str) -> Self {
        Self::new(id)
    }
}

impl fmt::Display for RuleId {
    /// Formats the `RuleId` by writing its inner string.
    ///
    /// # Examples
    ///
    /// ```
    /// use daemoneye_lib::models::RuleId;
    /// let id = RuleId::new("rule-123");
    /// assert_eq!(format!("{}", id), "rule-123");
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Rule metadata information.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
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
    /// Creates a new, empty `RuleMetadata` (equivalent to `Default::default()`).
    ///
    /// # Examples
    ///
    /// ```
    /// use daemoneye_lib::models::RuleMetadata;
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
    /// use daemoneye_lib::models::RuleMetadata;
    /// let meta = RuleMetadata::new().with_data("env", "production");
    /// assert_eq!(meta.data.get("env").map(String::as_str), Some("production"));
    /// ```
    #[must_use]
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
    /// use daemoneye_lib::models::RuleMetadata;
    /// let meta = RuleMetadata::new().with_tag("network").with_tag("suspicious");
    /// assert!(meta.tags.contains(&"network".to_string()));
    /// assert!(meta.tags.contains(&"suspicious".to_string()));
    /// ```
    #[must_use]
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
    /// use daemoneye_lib::models::RuleMetadata;
    /// let meta = RuleMetadata::new().with_author("alice");
    /// assert_eq!(meta.author, Some("alice".to_string()));
    /// ```
    #[must_use]
    pub fn with_author(mut self, author: impl Into<String>) -> Self {
        self.author = Some(author.into());
        self
    }

    /// Sets the metadata version and returns the updated `RuleMetadata` (builder-style).
    ///
    /// # Examples
    ///
    /// ```
    /// use daemoneye_lib::models::RuleMetadata;
    /// let meta = RuleMetadata::new().with_version("1.2.3");
    /// assert_eq!(meta.version.as_deref(), Some("1.2.3"));
    /// ```
    #[must_use]
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
    /// use daemoneye_lib::models::RuleMetadata;
    /// let meta = RuleMetadata::new().with_category("network");
    /// assert_eq!(meta.category.as_deref(), Some("network"));
    /// ```
    #[must_use]
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    /// Set the metadata priority.
    ///
    /// # Examples
    ///
    /// ```
    /// use daemoneye_lib::models::RuleMetadata;
    /// let meta = RuleMetadata::new().with_priority(5);
    /// assert_eq!(meta.priority, Some(5));
    /// ```
    #[must_use]
    pub const fn with_priority(mut self, priority: u8) -> Self {
        self.priority = Some(priority);
        self
    }
}

/// Detection rule with SQL query and metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    /// Create a new `DetectionRule` with sensible defaults.
    ///
    /// The constructor initializes timestamps to now, sets the rule `version` to `"1.0.0"`,
    /// `author` to `"system"`, enables the rule, and populates `metadata` with the provided
    /// `category`, the same version, and author. `id`, `name`, `description`, and `sql_query`
    /// are taken from the provided arguments; `severity` is set as given.
    ///
    /// # Examples
    ///
    /// ```
    /// use daemoneye_lib::models::{DetectionRule, RuleId, AlertSeverity};
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
        let rule_id = id.into();
        let rule_name = name.into();
        let rule_description = description.into();
        let rule_sql_query = sql_query.into();
        let rule_category = category.into();
        Self {
            id: rule_id,
            name: rule_name,
            description: rule_description,
            sql_query: rule_sql_query,
            severity,
            version: "1.0.0".to_owned(),
            author: "system".to_owned(),
            created_at: now,
            updated_at: now,
            enabled: true,
            tags: Vec::new(),
            metadata: RuleMetadata::new()
                .with_category(rule_category)
                .with_version("1.0.0")
                .with_author("system"),
        }
    }

    /// Validate the rule's SQL at the default subquery-depth limit.
    ///
    /// Equivalent to [`DetectionRule::validate_sql_with_depth`] called with
    /// `DetectionConfig::default().max_subquery_depth`. Callers that have an operator's
    /// configuration in hand should pass it rather than relying on this default.
    ///
    /// # Errors
    ///
    /// Returns [`RuleError::SqlRejected`] carrying the [`SqlRejection`] that names the offending
    /// construct and its position.
    ///
    /// # Examples
    ///
    /// ```
    /// # use daemoneye_lib::models::rule::{DetectionRule, RuleId};
    /// # use daemoneye_lib::models::alert::AlertSeverity;
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
        self.validate_sql_with_depth(DetectionConfig::default().max_subquery_depth)
    }

    /// Validate the rule's SQL against the rule-load gate at an explicit subquery-depth limit.
    ///
    /// The gate rejects anything that is not a single `SELECT`, any function outside the
    /// detection allowlist, and any subquery nested deeper than `max_subquery_depth` levels
    /// below the top-level `SELECT`. No rule is executed here.
    ///
    /// # Errors
    ///
    /// Returns [`RuleError::SqlRejected`] carrying the [`SqlRejection`] that names the offending
    /// construct and its position.
    ///
    /// # Examples
    ///
    /// ```
    /// # use daemoneye_lib::models::rule::{DetectionRule, RuleId};
    /// # use daemoneye_lib::models::alert::AlertSeverity;
    /// let rule = DetectionRule::new(
    ///     RuleId::from("r1"),
    ///     "Example",
    ///     "Example rule",
    ///     "SELECT pid FROM processes WHERE pid IN (SELECT pid FROM processes)",
    ///     "example",
    ///     AlertSeverity::Low,
    /// );
    /// assert!(rule.validate_sql_with_depth(1).is_ok());
    /// assert!(rule.validate_sql_with_depth(0).is_err());
    /// ```
    pub fn validate_sql_with_depth(&self, max_subquery_depth: u32) -> Result<(), RuleError> {
        validate_detection_sql(&self.sql_query, max_subquery_depth).map_err(RuleError::SqlRejected)
    }

    /// Update the rule's `updated_at` timestamp to the current system time.
    ///
    /// Sets `updated_at` to `SystemTime::now()`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use daemoneye_lib::models::{DetectionRule, RuleId, AlertSeverity};
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
    /// use daemoneye_lib::models::{DetectionRule, RuleId, AlertSeverity};
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
    /// use daemoneye_lib::models::{DetectionRule, RuleId, AlertSeverity};
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
    /// use daemoneye_lib::models::rule::{DetectionRule, RuleId};
    /// use daemoneye_lib::models::alert::AlertSeverity;
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
    /// use daemoneye_lib::models::{DetectionRule, RuleId, AlertSeverity};
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
    /// # use daemoneye_lib::models::rule::DetectionRule;
    /// # use daemoneye_lib::models::alert::AlertSeverity;
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
    /// use daemoneye_lib::models::{DetectionRule, AlertSeverity};
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
#[non_exhaustive]
pub enum RuleError {
    #[error("SQL rejected at rule load: {0}")]
    SqlRejected(#[from] SqlRejection),
    #[error("regex pattern rejected at rule load: {0}")]
    RegexRejected(#[from] RegexRejection),
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
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::str_to_string
)]
mod tests {
    use super::*;

    /// Assert the rule is rejected and that `predicate` accepts the *specific* rejection.
    ///
    /// Asserting on `RuleError::SqlRejected` alone would prove nothing: every gate in this module
    /// shares that one variant, so a test could sit on a branch it was never meant to cover.
    fn assert_rejected_as(rule: &DetectionRule, predicate: impl Fn(&SqlRejection) -> bool) {
        let err = rule
            .validate_sql()
            .expect_err("expected the rule to be rejected");
        let RuleError::SqlRejected(ref rejection) = err else {
            panic!("expected an SQL rejection, got {err:?}");
        };
        assert!(predicate(rejection), "the wrong gate fired: {rejection:?}");
    }

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
        let json = serde_json::to_string(&rule).expect("Failed to serialize rule");
        let deserialized: DetectionRule =
            serde_json::from_str(&json).expect("Failed to deserialize rule");
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
        assert_rejected_as(
            &invalid_rule,
            |rejection| matches!(*rejection, SqlRejection::NotASelect { ref statement_kind } if statement_kind == "DROP"),
        );

        // Test INSERT statement (should fail)
        let insert_rule = DetectionRule::new(
            "rule-003",
            "Insert Rule",
            "Insert description",
            "INSERT INTO processes VALUES (1, 'test')",
            "test",
            AlertSeverity::Low,
        );
        assert_rejected_as(
            &insert_rule,
            |rejection| matches!(*rejection, SqlRejection::NotASelect { ref statement_kind } if statement_kind == "INSERT"),
        );

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
        assert_rejected_as(
            &banned_func_rule,
            |rejection| matches!(*rejection, SqlRejection::FunctionNotAllowed { ref function, .. } if function == "load_extension"),
        );

        // Test too many joins (should fail)
        let many_joins_rule = DetectionRule::new(
            "rule-006",
            "Many Joins Rule",
            "Many joins description",
            "SELECT * FROM processes p1 JOIN processes p2 ON p1.pid = p2.pid JOIN processes p3 ON p1.pid = p3.pid JOIN processes p4 ON p1.pid = p4.pid JOIN processes p5 ON p1.pid = p5.pid JOIN processes p6 ON p1.pid = p6.pid",
            "test",
            AlertSeverity::Low,
        );
        assert_rejected_as(&many_joins_rule, |rejection| {
            matches!(
                *rejection,
                SqlRejection::TooManyOf {
                    construct: "JOIN",
                    found: 5,
                    limit: 4
                }
            )
        });
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

    #[test]
    fn test_function_validation_in_sql_queries() {
        // Test valid SQL without functions (should pass)
        let valid_no_func_rule = DetectionRule::new(
            "rule-func-001",
            "Valid No Function Rule",
            "Uses no functions",
            "SELECT name, pid FROM processes WHERE name IS NOT NULL",
            "test",
            AlertSeverity::Low,
        );
        assert!(valid_no_func_rule.validate_sql().is_ok());

        // Test banned function in SQL (should fail)
        let banned_func_rule = DetectionRule::new(
            "rule-func-002",
            "Banned Function Rule",
            "Uses banned function",
            "SELECT load_extension('test') FROM processes",
            "test",
            AlertSeverity::Low,
        );
        assert_rejected_as(
            &banned_func_rule,
            |rejection| matches!(*rejection, SqlRejection::FunctionNotAllowed { ref function, .. } if function == "load_extension"),
        );

        // Test allowed function (should pass)
        let length_func_rule = DetectionRule::new(
            "rule-func-003",
            "Length Function Rule",
            "Uses allowed LENGTH function",
            "SELECT LENGTH(name) FROM processes",
            "test",
            AlertSeverity::Low,
        );
        assert!(length_func_rule.validate_sql().is_ok());

        // Test allowed hex function (should pass)
        let hex_func_rule = DetectionRule::new(
            "rule-func-004",
            "Hex Function Rule",
            "Uses allowed HEX function for hash analysis",
            "SELECT HEX(executable_hash) FROM processes WHERE executable_hash IS NOT NULL",
            "test",
            AlertSeverity::Low,
        );
        assert!(hex_func_rule.validate_sql().is_ok());

        // Test function with subquery argument (should fail)
        let func_with_subquery_rule = DetectionRule::new(
            "rule-func-005",
            "Function with Subquery Rule",
            "Uses function with subquery",
            "SELECT load_extension((SELECT name FROM processes LIMIT 1)) FROM processes",
            "test",
            AlertSeverity::Low,
        );
        assert_rejected_as(
            &func_with_subquery_rule,
            |rejection| matches!(*rejection, SqlRejection::FunctionNotAllowed { ref function, .. } if function == "load_extension"),
        );

        // Test multiple banned functions (should fail)
        let multiple_banned_rule = DetectionRule::new(
            "rule-func-006",
            "Multiple Banned Functions Rule",
            "Uses multiple banned functions",
            "SELECT load_extension('test'), eval('malicious'), exec('command') FROM processes",
            "test",
            AlertSeverity::Low,
        );
        assert_rejected_as(
            &multiple_banned_rule,
            |rejection| matches!(*rejection, SqlRejection::FunctionNotAllowed { ref function, .. } if function == "load_extension"),
        );

        // Test banned function in WHERE clause (should fail)
        let banned_in_where_rule = DetectionRule::new(
            "rule-func-007",
            "Banned in WHERE Rule",
            "Uses banned function in WHERE",
            "SELECT * FROM processes WHERE load_extension('test') = 1",
            "test",
            AlertSeverity::Low,
        );
        assert_rejected_as(
            &banned_in_where_rule,
            |rejection| matches!(*rejection, SqlRejection::FunctionNotAllowed { ref function, .. } if function == "load_extension"),
        );
    }
}
