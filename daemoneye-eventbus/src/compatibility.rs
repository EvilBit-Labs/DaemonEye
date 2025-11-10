//! Version negotiation and backward compatibility for daemoneye-eventbus
//!
//! This module provides version negotiation capabilities and backward compatibility
//! strategies for the EventBus message protocol. It ensures that different versions
//! of collectors and agents can communicate effectively while maintaining protocol
//! evolution capabilities.
//!
//! ## Design Principles
//!
//! - **Semantic Versioning**: Major.Minor.Patch versioning with clear compatibility rules
//! - **Graceful Degradation**: Older clients can work with newer servers with reduced functionality
//! - **Feature Detection**: Capability-based negotiation for optional features
//! - **Migration Support**: Clear migration paths for protocol upgrades
//!
//! ## Compatibility Rules
//!
//! - **Major Version**: Breaking changes, no backward compatibility
//! - **Minor Version**: Backward compatible additions, new optional features
//! - **Patch Version**: Bug fixes, no protocol changes
//!
//! ## Usage Example
//!
//! ```rust,no_run
//! use daemoneye_eventbus::compatibility::{VersionNegotiator, CompatibilityChecker, VersionNegotiationRequest};
//! use daemoneye_eventbus::message::MessageVersion;
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let negotiator = VersionNegotiator::new();
//!     let client_version = MessageVersion::current();
//!     let server_versions = vec![
//!         MessageVersion { major: 1, minor: 0, patch: 0, protocol_version: "v1.0.0".to_string() },
//!         MessageVersion { major: 1, minor: 1, patch: 0, protocol_version: "v1.1.0".to_string() },
//!     ];
//!
//!     let request = VersionNegotiationRequest {
//!         supported_versions: server_versions.clone(),
//!         preferred_version: client_version.clone(),
//!         client_capabilities: vec!["basic_pub_sub".to_string(), "rpc_calls".to_string()],
//!         client_id: "example-client".to_string(),
//!     };
//!
//!     let response = negotiator.negotiate_version(&request)?;
//!     println!("Negotiated version: {:?}", response.negotiated_version);
//!     Ok(())
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

use crate::message::MessageVersion;

/// Version negotiation errors
#[derive(Debug, Error)]
pub enum CompatibilityError {
    /// No compatible version found
    #[error("No compatible version found between client and server")]
    NoCompatibleVersion,
    /// Unsupported major version
    #[error("Unsupported major version: {version}")]
    UnsupportedMajorVersion { version: u32 },
    /// Feature not available in negotiated version
    #[error("Feature '{feature}' not available in version {version:?}")]
    FeatureNotAvailable {
        feature: String,
        version: MessageVersion,
    },
    /// Migration required
    #[error("Migration required from version {from:?} to {to:?}")]
    MigrationRequired {
        from: MessageVersion,
        to: MessageVersion,
    },
}

/// Version negotiation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionNegotiationRequest {
    /// Client supported versions (in preference order)
    pub supported_versions: Vec<MessageVersion>,
    /// Client preferred version
    pub preferred_version: MessageVersion,
    /// Client capabilities
    pub client_capabilities: Vec<String>,
    /// Client identifier for logging
    pub client_id: String,
}

/// Version negotiation response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionNegotiationResponse {
    /// Negotiated version to use
    pub negotiated_version: MessageVersion,
    /// Server capabilities available in negotiated version
    pub server_capabilities: Vec<String>,
    /// Compatibility warnings for client
    pub compatibility_warnings: Vec<String>,
    /// Negotiation success status
    pub negotiation_success: bool,
    /// Error message if negotiation failed
    pub error_message: Option<String>,
}

/// Backward compatibility information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityInfo {
    /// Minimum supported version
    pub min_supported_version: MessageVersion,
    /// Maximum supported version
    pub max_supported_version: MessageVersion,
    /// Deprecated features in current version
    pub deprecated_features: Vec<String>,
    /// Migration instructions for version upgrades
    pub migration_instructions: Option<String>,
    /// Breaking changes by version
    pub breaking_changes: HashMap<String, Vec<String>>,
}

/// Version negotiator for client-server compatibility
#[derive(Debug)]
pub struct VersionNegotiator {
    /// Supported server versions
    supported_versions: Vec<MessageVersion>,
    /// Server capabilities by version
    capabilities_by_version: HashMap<String, Vec<String>>,
    /// Compatibility information
    compatibility_info: CompatibilityInfo,
}

/// Compatibility checker for feature availability
#[derive(Debug)]
pub struct CompatibilityChecker {
    /// Current negotiated version
    negotiated_version: MessageVersion,
    /// Available capabilities
    available_capabilities: Vec<String>,
}

impl VersionNegotiator {
    /// Create a new version negotiator
    pub fn new() -> Self {
        let current_version = MessageVersion::current();
        let mut capabilities_by_version = HashMap::new();

        // Define capabilities for each version
        capabilities_by_version.insert(
            "v1.0.0".to_string(),
            vec![
                "basic_pub_sub".to_string(),
                "rpc_calls".to_string(),
                "correlation_tracking".to_string(),
            ],
        );

        capabilities_by_version.insert(
            "v1.1.0".to_string(),
            vec![
                "basic_pub_sub".to_string(),
                "rpc_calls".to_string(),
                "correlation_tracking".to_string(),
                "advanced_routing".to_string(),
                "circuit_breaker".to_string(),
            ],
        );

        Self {
            supported_versions: vec![current_version.clone()],
            capabilities_by_version,
            compatibility_info: CompatibilityInfo {
                min_supported_version: current_version.clone(),
                max_supported_version: current_version,
                deprecated_features: vec![],
                migration_instructions: None,
                breaking_changes: HashMap::new(),
            },
        }
    }

    /// Add a supported version with capabilities
    pub fn add_supported_version(&mut self, version: MessageVersion, capabilities: Vec<String>) {
        let version_key = format!("v{}.{}.{}", version.major, version.minor, version.patch);
        self.capabilities_by_version
            .insert(version_key, capabilities);
        self.supported_versions.push(version);
        self.supported_versions.sort_by(|a, b| {
            a.major
                .cmp(&b.major)
                .then(a.minor.cmp(&b.minor))
                .then(a.patch.cmp(&b.patch))
        });
    }

    /// Negotiate version with a client
    pub fn negotiate_version(
        &self,
        client_request: &VersionNegotiationRequest,
    ) -> Result<VersionNegotiationResponse, CompatibilityError> {
        // Find the best compatible version
        let negotiated_version = self.find_best_compatible_version(
            &client_request.supported_versions,
            &client_request.preferred_version,
        )?;

        // Get server capabilities for negotiated version
        let version_key = format!(
            "v{}.{}.{}",
            negotiated_version.major, negotiated_version.minor, negotiated_version.patch
        );
        let server_capabilities = self
            .capabilities_by_version
            .get(&version_key)
            .cloned()
            .unwrap_or_default();

        // Generate compatibility warnings
        let warnings = self.generate_compatibility_warnings(
            &client_request.preferred_version,
            &negotiated_version,
            &client_request.client_capabilities,
            &server_capabilities,
        );

        Ok(VersionNegotiationResponse {
            negotiated_version,
            server_capabilities,
            compatibility_warnings: warnings,
            negotiation_success: true,
            error_message: None,
        })
    }

    /// Find the best compatible version between client and server
    fn find_best_compatible_version(
        &self,
        client_versions: &[MessageVersion],
        preferred_version: &MessageVersion,
    ) -> Result<MessageVersion, CompatibilityError> {
        // First, try the preferred version if it's supported
        if self.is_version_supported(preferred_version) {
            return Ok(preferred_version.clone());
        }

        // Find the highest compatible version
        for client_version in client_versions.iter().rev() {
            if self.is_version_supported(client_version) {
                return Ok(client_version.clone());
            }
        }

        Err(CompatibilityError::NoCompatibleVersion)
    }

    /// Check if a version is supported by the server
    fn is_version_supported(&self, version: &MessageVersion) -> bool {
        self.supported_versions
            .iter()
            .any(|v| v.major == version.major && v.minor >= version.minor)
    }

    /// Generate compatibility warnings for the negotiated version
    fn generate_compatibility_warnings(
        &self,
        preferred_version: &MessageVersion,
        negotiated_version: &MessageVersion,
        client_capabilities: &[String],
        server_capabilities: &[String],
    ) -> Vec<String> {
        let mut warnings = Vec::new();

        // Warn if negotiated version is different from preferred
        if negotiated_version.major != preferred_version.major
            || negotiated_version.minor != preferred_version.minor
            || negotiated_version.patch != preferred_version.patch
        {
            warnings.push(format!(
                "Using version {}.{}.{} instead of preferred {}.{}.{}",
                negotiated_version.major,
                negotiated_version.minor,
                negotiated_version.patch,
                preferred_version.major,
                preferred_version.minor,
                preferred_version.patch
            ));
        }

        // Warn about missing client capabilities
        for capability in client_capabilities {
            if !server_capabilities.contains(capability) {
                warnings.push(format!(
                    "Client capability '{}' not available in negotiated version",
                    capability
                ));
            }
        }

        // Warn about deprecated features
        for deprecated_feature in &self.compatibility_info.deprecated_features {
            if client_capabilities.contains(deprecated_feature) {
                warnings.push(format!(
                    "Feature '{}' is deprecated and may be removed in future versions",
                    deprecated_feature
                ));
            }
        }

        warnings
    }

    /// Get compatibility information
    pub fn get_compatibility_info(&self) -> &CompatibilityInfo {
        &self.compatibility_info
    }
}

impl CompatibilityChecker {
    /// Create a new compatibility checker
    pub fn new(negotiated_version: MessageVersion, available_capabilities: Vec<String>) -> Self {
        Self {
            negotiated_version,
            available_capabilities,
        }
    }

    /// Check if a feature is available in the current version
    pub fn is_feature_available(&self, feature: &str) -> bool {
        self.available_capabilities.contains(&feature.to_string())
    }

    /// Require a feature or return an error
    pub fn require_feature(&self, feature: &str) -> Result<(), CompatibilityError> {
        if self.is_feature_available(feature) {
            Ok(())
        } else {
            Err(CompatibilityError::FeatureNotAvailable {
                feature: feature.to_string(),
                version: self.negotiated_version.clone(),
            })
        }
    }

    /// Get all available capabilities
    pub fn get_available_capabilities(&self) -> &[String] {
        &self.available_capabilities
    }

    /// Get negotiated version
    pub fn get_negotiated_version(&self) -> &MessageVersion {
        &self.negotiated_version
    }

    /// Check if version supports advanced routing
    pub fn supports_advanced_routing(&self) -> bool {
        self.is_feature_available("advanced_routing")
    }

    /// Check if version supports circuit breaker
    pub fn supports_circuit_breaker(&self) -> bool {
        self.is_feature_available("circuit_breaker")
    }

    /// Check if version supports correlation tracking
    pub fn supports_correlation_tracking(&self) -> bool {
        self.is_feature_available("correlation_tracking")
    }
}

impl Default for VersionNegotiator {
    fn default() -> Self {
        Self::new()
    }
}

impl VersionNegotiationRequest {
    /// Create a new version negotiation request
    pub fn new(
        client_id: String,
        supported_versions: Vec<MessageVersion>,
        preferred_version: MessageVersion,
        client_capabilities: Vec<String>,
    ) -> Self {
        Self {
            supported_versions,
            preferred_version,
            client_capabilities,
            client_id,
        }
    }

    /// Create a request for current version
    pub fn for_current_version(client_id: String) -> Self {
        let current_version = MessageVersion::current();
        Self::new(
            client_id,
            vec![current_version.clone()],
            current_version,
            vec![
                "basic_pub_sub".to_string(),
                "rpc_calls".to_string(),
                "correlation_tracking".to_string(),
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_negotiation_success() {
        let negotiator = VersionNegotiator::new();
        let request = VersionNegotiationRequest::for_current_version("test-client".to_string());

        let response = negotiator.negotiate_version(&request);
        assert!(response.is_ok());

        let response = response.unwrap();
        assert!(response.negotiation_success);
        assert_eq!(response.negotiated_version.major, 1);
    }

    #[test]
    fn test_version_negotiation_failure() {
        let negotiator = VersionNegotiator::new();
        let unsupported_version = MessageVersion {
            major: 99,
            minor: 0,
            patch: 0,
            protocol_version: "v99.0.0".to_string(),
        };

        let request = VersionNegotiationRequest::new(
            "test-client".to_string(),
            vec![unsupported_version.clone()],
            unsupported_version,
            vec![],
        );

        let response = negotiator.negotiate_version(&request);
        assert!(response.is_err());
        assert!(matches!(
            response.unwrap_err(),
            CompatibilityError::NoCompatibleVersion
        ));
    }

    #[test]
    fn test_compatibility_checker() {
        let version = MessageVersion::current();
        let capabilities = vec!["basic_pub_sub".to_string(), "rpc_calls".to_string()];
        let checker = CompatibilityChecker::new(version, capabilities);

        assert!(checker.is_feature_available("basic_pub_sub"));
        assert!(!checker.is_feature_available("advanced_routing"));

        assert!(checker.require_feature("rpc_calls").is_ok());
        assert!(checker.require_feature("nonexistent_feature").is_err());
    }

    #[test]
    fn test_version_comparison() {
        let v1 = MessageVersion {
            major: 1,
            minor: 0,
            patch: 0,
            protocol_version: "v1.0.0".to_string(),
        };
        let v2 = MessageVersion {
            major: 1,
            minor: 1,
            patch: 0,
            protocol_version: "v1.1.0".to_string(),
        };

        assert!(v1.is_compatible_with(&v2));
        assert!(v2.is_newer_than(&v1));
        assert!(!v1.is_newer_than(&v2));
    }
}
