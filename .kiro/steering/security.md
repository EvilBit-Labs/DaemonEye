# DaemonEye Security Guidelines

## Core Security Requirements

- **Principle of Least Privilege**: Components run with minimal required permissions
- **Privilege Separation**: Only procmond runs with elevated privileges when necessary
- **Automatic Privilege Dropping**: Immediate privilege drop after initialization
- **SQL Injection Prevention**: AST validation with sqlparser, prepared statements only
- **Credential Management**: Environment variables or OS keychain, never hardcode secrets
- **Input Validation**: Comprehensive validation with detailed error messages
- **Attack Surface Minimization**: No network listening, outbound-only connections
- **Audit Trail**: Certificate Transparency-style Merkle tree with BLAKE3 cryptographic integrity

## Advanced Security Features (Enterprise Tier)

- **mTLS Authentication**: Certificate chain validation for enterprise components
- **Code Signing**: SLSA Level 3 provenance, Cosign signatures
- **Cryptographic Integrity**: Merkle tree with inclusion proofs and periodic checkpoints
- **Sandboxed Execution**: Read-only database connections for detection engine
- **Query Whitelist**: Only SELECT statements with approved functions allowed

## Data Protection

- **Command-line Redaction**: Optional privacy-preserving command line masking
- **Log Field Masking**: Configurable field masking in structured logs
- **Database Encryption**: Support for sensitive deployments
- **Secure Storage**: OS keychain integration for credentials

## Integer Overflow Protection

- **Release Build Safety**: All builds must enable overflow checks in release mode to prevent silent arithmetic wraparound vulnerabilities
- **Checked Arithmetic**: Use `checked_*`, `saturating_*`, or explicit `wrapping_*` operations for security-sensitive calculations
- **Untrusted Data**: Avoid bare arithmetic on untrusted data without bounds validation
- **Configuration**: Enable overflow checks via Cargo profile configuration:

```toml
[profile.release]
overflow-checks = true
lto = "thin"
codegen-units = 1
```

- **Example Implementation**:

```rust
// Prefer checked/saturating math at trust boundaries
let buffer_size = user_provided_len
    .checked_mul(ENTRY_SIZE)
    .ok_or(SecurityError::ArithmeticOverflow)?;

// Explicit bounds checking for array access
if index >= data.len() {
    return Err(SecurityError::IndexOutOfBounds);
}
```

- **Testing**: Include `#[should_panic]` release-mode tests to catch misconfiguration if overflow checks are disabled

## Safe Concurrency

- **Tokio Runtime**: Use tokio runtime exclusively for async operations with preferred primitives: `tokio::sync::{Semaphore, mpsc, oneshot, watch, Notify}`
- **Lock Scope Minimization**: Avoid awaiting while holding locks; keep lock scope minimal to prevent deadlocks
- **Async vs Sync Locks**: Use `tokio::sync::Mutex/RwLock` for async code paths; reserve `std::sync` locks for synchronous-only code
- **Ownership Transfer**: Prefer channels and ownership transfer over shared mutable state
- **Bounded Concurrency**: Use `Semaphore` to bound concurrency; define capacities consistent with resource budgets and backpressure policies
- **Linting**: Enable `clippy::await_holding_lock = "deny"` in CI to catch lock-holding across await points

```rust
// Bounded concurrency pattern
let semaphore = Arc::new(Semaphore::new(64));
let permit = semaphore.acquire().await?;
// Critical: no .await calls while holding locks
{
    let _guard = shared_state.lock().await;
    // Synchronous work only inside lock scope
    process_data_synchronously(&data);
    // Lock dropped here
}
// Async work after lock is released
perform_async_operation().await?;
drop(permit);
```

- **Concurrency Testing**: Use `loom` for model checking of core synchronization primitives; add `cfg(loom)` test configurations

## Cryptographic Standards

- **No Custom Crypto**: Use only audited, well-established cryptographic libraries; never implement custom cryptographic algorithms
- **Approved Libraries**:
  - **Hashing**: BLAKE3 (already in use) or SHA2 family; never SHA-1
  - **Signatures**: `ed25519-dalek` for digital signatures (already in use)
  - **AEAD**: `chacha20poly1305` or `aes-gcm` (RustCrypto) for authenticated encryption
  - **KDF**: HKDF-SHA256 for key derivation; Argon2id for password-like material
  - **TLS/mTLS**: `rustls` with modern cipher suites; TLS 1.2+ minimum, TLS 1.3 preferred
  - **Entropy**: `getrandom` for secure random number generation; avoid custom RNGs
- **Secret Handling**: Use `secrecy` and `zeroize` crates for handling sensitive data in memory
- **Certificate Verification**: Verify certificate chains and enforce hostname verification for all TLS connections

```rust
use secrecy::SecretString;
use zeroize::Zeroize;

/// Secure handling of API keys with automatic zeroing
#[derive(Debug)]
pub struct ApiKey(SecretString);

impl ApiKey {
    pub fn new(key: String) -> Self {
        Self(SecretString::new(key))
    }
}

impl Drop for ApiKey {
    fn drop(&mut self) {
        // Ensure sensitive data is zeroed from memory
        let mut bytes = self.0.expose_secret().as_bytes().to_vec();
        bytes.zeroize();
    }
}
```

- **Enterprise/FIPS**: Document constraints and approved modules when FIPS-validated cryptography is required
- **Algorithm Deprecation**: Maintain awareness of cryptographic algorithm lifecycle; plan migration away from deprecated algorithms

## Input Validation Patterns

- **Trust Boundaries**: Treat all external inputs as untrusted (CLI arguments, configuration files, IPC messages, database content, network requests)
- **Early Validation**: Validate shape, bounds, and constraints first; reject malformed input with actionable error messages
- **Typed Parsers**: Prefer strongly-typed parsers over regular expressions; when using regex, anchor patterns appropriately
- **Parser Integration**: Use `clap` value_parser with built-in type validation; leverage `serde_with` for advanced deserialization patterns

```rust
// CLI argument validation with bounds
#[derive(Parser)]
struct Args {
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..=65535))]
    port: u16,

    #[arg(long, value_parser = parse_url)]
    endpoint: url::Url,
}

fn parse_url(s: &str) -> Result<url::Url, String> {
    s.parse().map_err(|e| format!("Invalid URL: {}", e))
}

// Configuration with typed duration parsing
#[derive(serde::Deserialize)]
struct Config {
    #[serde(with = "humantime_serde")]
    scan_interval: std::time::Duration,

    #[serde(deserialize_with = "bounded_string")]
    description: String,
}

fn bounded_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    if s.len() <= 1024 {
        Ok(s)
    } else {
        Err(serde::de::Error::custom(
            "String exceeds 1024 character limit",
        ))
    }
}
```

- **SQL Safety**: Continue AST validation with `sqlparser` and prepared statements only for database queries
- **Canonicalization**: Normalize input where appropriate (case folding, path normalization, Unicode normalization)
- **Length Limits**: Enforce reasonable bounds on all variable-length inputs to prevent resource exhaustion

## Newtype Safety

- **Domain Constraints**: Use newtypes to encode domain-specific constraints and prevent unit confusion (ports vs PIDs vs timestamps)
- **NonZero Types**: Prefer `NonZero*` types where zero values are invalid; provide smart constructors for validation
- **Serialization Compatibility**: Ensure `#[serde(transparent)]` for wire format compatibility while maintaining type safety
- **Smart Constructors**: Implement `TryFrom` and validation methods to ensure invariants are maintained

```rust
/// Type-safe port representation
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Port(std::num::NonZeroU16);

impl TryFrom<u16> for Port {
    type Error = ValidationError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        std::num::NonZeroU16::new(value)
            .map(Port)
            .ok_or(ValidationError::InvalidPort)
    }
}

/// Duration with overflow-safe arithmetic
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Millis(u64);

impl Millis {
    pub fn checked_add(self, other: Millis) -> Option<Millis> {
        self.0.checked_add(other.0).map(Millis)
    }

    pub fn saturating_mul(self, factor: u64) -> Millis {
        Millis(self.0.saturating_mul(factor))
    }
}
```

## Dependency Security

- **Lock File Management**: Always commit `Cargo.lock` to ensure reproducible builds across environments
- **Dependency Resolution**: Use `resolver = "3"` for improved dependency resolution and security
- **Version Constraints**: Avoid wildcard versions; pin security-critical dependencies to specific versions
- **Feature Minimization**: Prefer `default-features = false` and explicitly enable only required features
- **Git Dependencies**: Pin git dependencies to specific commit SHAs rather than branch names
- **Supply Chain Tools**: Use `cargo audit`, `cargo deny`, and consider `cargo vet` for supply chain attestation
- **Airgapped Deployment**: Support `cargo vendor` for offline/airgapped environments

```toml
[workspace.package]
resolver = "3"

[dependencies.reqwest]
version = "0.12.0"                # Specific version, no wildcards
default-features = false
features = ["rustls-tls", "json"]

[dependencies.custom-lib]
git = "https://github.com/example/custom-lib"
rev = "a1b2c3d4"                              # Specific commit SHA
```

- **Security Scanning**: Integrate security tools in development workflow:
  - `audit-deps`: `cargo audit`
  - `deny-deps`: `cargo deny check`
  - `security-scan`: Composed recipe combining lint, audit, and deny checks
