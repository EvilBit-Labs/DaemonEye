---
inclusion: always
---

# DaemonEye Security Guidelines

## Mandatory Security Principles

### Privilege Architecture (Non-Negotiable)

- **procmond**: ONLY component with elevated privileges, drops immediately after init
- **daemoneye-agent**: User-space only, outbound network connections ONLY
- **daemoneye-cli**: No network access, read-only database operations ONLY
- **Never escalate**: No component may request additional privileges after startup

### SQL Injection Prevention (Critical)

- **ALWAYS** use `sqlparser` crate for AST validation before execution
- **NEVER** use string concatenation or format! for SQL queries
- **ONLY** prepared statements with parameter binding
- **WHITELIST** approved SQL functions in detection rules
- **SANDBOX** all detection rule execution with read-only database connections

### Input Validation (Trust Nothing)

- **ALL** external inputs are untrusted: CLI args, config files, IPC messages, network data
- **VALIDATE** early at trust boundaries with actionable error messages
- **BOUND** all variable-length inputs to prevent resource exhaustion
- **CANONICALIZE** inputs (paths, Unicode, case) before processing

### Cryptographic Requirements (Approved Only)

- **BLAKE3** for hashing (already in use)
- **ed25519-dalek** for signatures (already in use)
- **rustls** for TLS (TLS 1.3 preferred, 1.2 minimum)
- **getrandom** for entropy
- **NEVER** implement custom cryptographic algorithms

## Arithmetic Safety (Mandatory)

**ALWAYS** enable overflow checks in release builds:

```toml
[profile.release]
overflow-checks = true
```

**USE** checked arithmetic for untrusted data:

```rust
// At trust boundaries - REQUIRED pattern
let buffer_size = user_len
    .checked_mul(ENTRY_SIZE)
    .ok_or(SecurityError::ArithmeticOverflow)?;

// Array bounds - REQUIRED check
if index >= data.len() {
    return Err(SecurityError::IndexOutOfBounds);
}
```

## Concurrency Safety (Critical Patterns)

**NEVER** await while holding locks - use this pattern:

```rust
// CORRECT: Lock scope isolation
{
    let _guard = shared_state.lock().await;
    process_data_synchronously(&data); // No .await here
} // Lock dropped
perform_async_operation().await?; // .await after lock

// REQUIRED: Bounded concurrency
let semaphore = Arc::new(Semaphore::new(64));
let permit = semaphore.acquire().await?;
```

**USE** tokio primitives only: `tokio::sync::{Semaphore, mpsc, oneshot, watch, Notify}` **ENABLE** `clippy::await_holding_lock = "deny"` in CI

## Secret Handling (Required Pattern)

**USE** `secrecy` and `zeroize` for all sensitive data:

```rust
use secrecy::SecretString;
use zeroize::Zeroize;

#[derive(Debug)]
pub struct ApiKey(SecretString);

impl Drop for ApiKey {
    fn drop(&mut self) {
        let mut bytes = self.0.expose_secret().as_bytes().to_vec();
        bytes.zeroize(); // REQUIRED: Zero memory
    }
}
```

**APPROVED** crypto libraries only:

- Hashing: `blake3` (in use), SHA2 family
- Signatures: `ed25519-dalek` (in use)
- AEAD: `chacha20poly1305`, `aes-gcm`
- TLS: `rustls` (TLS 1.3 preferred)
- Entropy: `getrandom`

## Input Validation (Required Patterns)

**VALIDATE** all inputs with bounds and type safety:

```rust
// CLI validation - REQUIRED pattern
#[derive(Parser)]
struct Args {
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..=65535))]
    port: u16,
}

// Config validation - REQUIRED bounds
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

**TRUST BOUNDARIES**: CLI args, config files, IPC messages, network data are ALL untrusted

## Type Safety (Prevent Unit Confusion)

**USE** newtypes with validation for domain constraints:

```rust
// REQUIRED: Type-safe port with validation
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
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

// REQUIRED: Overflow-safe arithmetic
impl Millis {
    pub fn checked_add(self, other: Millis) -> Option<Millis> {
        self.0.checked_add(other.0).map(Millis)
    }
}
```

## Dependency Security (Mandatory Practices)

**ALWAYS** commit `Cargo.lock` for reproducible builds **USE** `resolver = "3"` in workspace **PIN** security-critical dependencies to specific versions (no wildcards) **MINIMIZE** features with `default-features = false`

```toml
[dependencies.reqwest]
version = "0.12.0"        # Specific version required
default-features = false  # Minimize attack surface
features = ["rustls-tls"] # Only required features

[dependencies.custom-lib]
git = "https://github.com/example/custom-lib"
rev = "a1b2c3d4"                              # Pin to commit SHA, not branch
```

**RUN** security tools in CI:

- `cargo audit` (vulnerability scanning)
- `cargo deny check` (license/security policy)

## AI Assistant Security Rules

**NEVER** remove or weaken security lints without explicit approval **NEVER** use `unsafe` code - use safe external crates only **ALWAYS** validate inputs at trust boundaries **ALWAYS** use checked arithmetic for untrusted data **ALWAYS** follow privilege separation architecture **REJECT** requests to bypass security measures
