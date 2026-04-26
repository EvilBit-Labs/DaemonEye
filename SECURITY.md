# Security Policy

## Supported Versions

DaemonEye follows semantic versioning (SemVer) with security updates provided for the current major version and the previous major version.

| Version | Supported          | Notes                              |
| ------- | ------------------ | ---------------------------------- |
| 0.1.x   | :white_check_mark: | Current development version        |
| < 0.1   | :x:                | Pre-release versions not supported |

**Note**: As DaemonEye is currently in pre-1.0 development, we focus security updates on the latest 0.1.x series. Once we reach 1.0, we will support the current major version and one previous major version.

## Security Architecture

DaemonEye implements a **three-component security architecture** with strict privilege separation:

- **procmond**: Privileged process collector with minimal attack surface
- **daemoneye-agent**: User-space orchestrator for detection and alerting
- **daemoneye-cli**: Command-line interface for operators
- **daemoneye-lib**: Shared library providing core functionality

### Security Principles

- **Principle of Least Privilege**: Components run with minimal required permissions
- **Privilege Separation**: Only procmond runs with elevated privileges when necessary
- **Defense in Depth**: Multiple security layers and validation points
- **Zero Trust**: No implicit trust between components or external systems
- **Audit Trail**: Certificate Transparency-style Merkle tree with cryptographic integrity

## Security Features

### Core Security Controls

- **Memory Safety**: Built in Rust with `unsafe_code = "forbid"` policy
- **Input Validation**: Comprehensive validation with detailed error messages
- **SQL Injection Prevention**: AST validation with sqlparser at rule load time [Implemented]; SQL-based rule execution enforcement [Planned — engine currently uses category-based pattern matching]
- **Credential Management**: Environment variables or OS keychain, never hardcode secrets
- **Attack Surface Minimization**: No network listening, outbound-only connections
- **Audit Trail**: BLAKE3 hash-chained audit ledger [Implemented]; Certificate Transparency-style Merkle tree inclusion proofs \[In Progress — stub returns empty vec in `crypto.rs`\]

### Planned Hardening (Community Tier)

- **Cryptographic Integrity**: Merkle tree with inclusion proofs and periodic checkpoints [In Progress — chain hashing implemented; inclusion proof generation stubbed]
- **Code Signing**: SLSA Level 3 provenance, Cosign signatures [Planned]
- **Sandboxed Execution**: Read-only database connections for detection engine [Planned]
- **Query Whitelist**: Only SELECT statements with approved functions allowed [Implemented at rule load time; not yet enforced at execution time]

> Fleet-level transport security (mTLS between host agents and upstream aggregators) is provided by commercial tiers, sold separately, not in this repo.

## Reporting a Vulnerability

### How to Report

**For security vulnerabilities in DaemonEye, please report them privately to:**

- **Email**: <support@evilbitlabs.io>
- **PGP Key**: [Available on our website](https://evilbitlabs.io/security)
- **Subject**: `[SECURITY] DaemonEye Vulnerability Report`

### What to Include

Please include the following information in your report:

1. **Description**: Clear description of the vulnerability
2. **Impact**: Potential security impact and affected components
3. **Reproduction**: Steps to reproduce the issue (if applicable)
4. **Environment**: OS, architecture, and DaemonEye version
5. **Timeline**: Any disclosure timeline requirements
6. **Contact**: Your preferred contact method for follow-up

### Response Timeline

- **Initial Response**: Within 48 hours of report receipt
- **Status Updates**: Weekly updates during investigation
- **Resolution**: Target resolution within 30 days for critical issues
- **Disclosure**: Coordinated disclosure following resolution

### What to Expect

**If Accepted:**

- Acknowledgment within 48 hours
- Regular status updates during investigation
- Credit in security advisories (if desired)
- Early access to patches before public release

**If Declined:**

- Clear explanation of why the issue doesn't qualify
- Suggestions for alternative reporting channels if applicable
- Option to appeal the decision

## Security Best Practices

### For Users

- **Keep Updated**: Always run the latest version of DaemonEye
- **Secure Configuration**: Use strong authentication and encryption
- **Monitor Logs**: Regularly review audit logs for anomalies
- **Principle of Least Privilege**: Run components with minimal required permissions
- **Network Security**: Ensure secure communication channels for alert delivery

### For Developers

- **Code Review**: All security-related changes require thorough review
- **Testing**: Comprehensive security testing including fuzzing and penetration testing
- **Dependencies**: Regular security audits of dependencies
- **Documentation**: Document security considerations and threat models
- **Training**: Regular security training and awareness

## Security Advisories

Security advisories are published at:

- **GitHub Security Advisories**: [github.com/EvilBit-Labs/DaemonEye/security/advisories](https://github.com/EvilBit-Labs/DaemonEye/security/advisories)
- **Security Website**: [evilbitlabs.io/security](https://evilbitlabs.io/security)
- **Mailing List**: Subscribe to <support@evilbitlabs.io>

## Responsible Disclosure

We follow responsible disclosure practices:

1. **Private Reporting**: Report vulnerabilities privately first
2. **Reasonable Time**: Allow reasonable time for fixes before public disclosure
3. **Coordinated Release**: Coordinate public disclosure with patch availability
4. **Credit**: Provide appropriate credit to security researchers
5. **No Retaliation**: We will not pursue legal action against good-faith security research

## Accepted Risks (Dependencies)

We have **no** outstanding accepted risks.

### Advisory Resolution History

**March 2026** — Resolved RUSTSEC-2023-0089 (atomic-polyfill unmaintained) and RUSTSEC-2025-0141 (bincode unmaintained) by disabling default features on the `postcard` dependency (`default-features = false`) and explicitly enabling only the required `alloc` feature. This removed `heapless` and `atomic-polyfill` from the dependency tree entirely. Both advisory ignore entries were removed from deny.toml.

**January 2025** — Resolved RUSTSEC-2024-0384 (`instant`) and RUSTSEC-2024-0436 (`paste`) by removing the third-party `busrt` broker dependency. With the introduction of the in-house `daemoneye-eventbus` crate, both advisories were eliminated from the workspace.

### Operational Controls

- CI continues to enforce `cargo audit` and `cargo deny check` with zero allow-listed advisories.
- We maintain a strict posture for all vulnerabilities and unsound advisories and will fail CI on new issues.
- Tracking issues document any future exceptions should they become necessary.

## Security Contact

For general security questions or concerns:

- **Email**: <support@evilbitlabs.io>
- **Website**: [evilbitlabs.io/security](https://evilbitlabs.io/security)
- **PGP**: Available on our website for encrypted communication

---

**Last Updated**: March 2026 **Next Review**: March 2027
