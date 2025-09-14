# Security Policy

## Supported Versions

SentinelD follows semantic versioning (SemVer) with security updates provided for the current major version and the previous major version.

| Version | Supported          | Notes                                    |
| ------- | ------------------ | ---------------------------------------- |
| 0.1.x   | :white_check_mark: | Current development version              |
| < 0.1   | :x:                | Pre-release versions not supported       |

**Note**: As SentinelD is currently in pre-1.0 development, we focus security updates on the latest 0.1.x series. Once we reach 1.0, we will support the current major version and one previous major version.

## Security Architecture

SentinelD implements a **three-component security architecture** with strict privilege separation:

- **procmond**: Privileged process collector with minimal attack surface
- **sentinelagent**: User-space orchestrator for detection and alerting
- **sentinelcli**: Command-line interface for operators
- **sentinel-lib**: Shared library providing core functionality

### Security Principles

- **Principle of Least Privilege**: Components run with minimal required permissions
- **Privilege Separation**: Only procmond runs with elevated privileges when necessary
- **Defense in Depth**: Multiple security layers and validation points
- **Zero Trust**: No implicit trust between components or external systems
- **Audit Trail**: Comprehensive logging and tamper-evident audit chains

## Security Features

### Core Security Controls

- **Memory Safety**: Built in Rust with `unsafe_code = "forbid"` policy
- **Input Validation**: Comprehensive validation with detailed error messages
- **SQL Injection Prevention**: AST validation with sqlparser, prepared statements only
- **Credential Management**: Environment variables or OS keychain, never hardcode secrets
- **Attack Surface Minimization**: No network listening, outbound-only connections
- **Audit Trail**: Tamper-evident audit logging with cryptographic integrity

### Advanced Security Features (Enterprise Tier)

- **mTLS Authentication**: Certificate chain validation for enterprise components
- **Code Signing**: SLSA Level 3 provenance, Cosign signatures
- **Cryptographic Integrity**: BLAKE3 hashing for audit chains
- **Sandboxed Execution**: Read-only database connections for detection engine
- **Query Whitelist**: Only SELECT statements with approved functions allowed

## Reporting a Vulnerability

### How to Report

**For security vulnerabilities in SentinelD, please report them privately to:**

- **Email**: <support@evilbitlabs.io>
- **PGP Key**: [Available on our website](https://evilbitlabs.io/security)
- **Subject**: `[SECURITY] SentinelD Vulnerability Report`

### What to Include

Please include the following information in your report:

1. **Description**: Clear description of the vulnerability
2. **Impact**: Potential security impact and affected components
3. **Reproduction**: Steps to reproduce the issue (if applicable)
4. **Environment**: OS, architecture, and SentinelD version
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

- **Keep Updated**: Always run the latest version of SentinelD
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

- **GitHub Security Advisories**: [github.com/EvilBit-Labs/SentinelD/security/advisories](https://github.com/EvilBit-Labs/SentinelD/security/advisories)
- **Security Website**: [evilbitlabs.io/security](https://evilbitlabs.io/security)
- **Mailing List**: Subscribe to <support@evilbitlabs.io>

## Responsible Disclosure

We follow responsible disclosure practices:

1. **Private Reporting**: Report vulnerabilities privately first
2. **Reasonable Time**: Allow reasonable time for fixes before public disclosure
3. **Coordinated Release**: Coordinate public disclosure with patch availability
4. **Credit**: Provide appropriate credit to security researchers
5. **No Retaliation**: We will not pursue legal action against good-faith security research

## Security Contact

For general security questions or concerns:

- **Email**: <support@evilbitlabs.io>
- **Website**: [evilbitlabs.io/security](https://evilbitlabs.io/security)
- **PGP**: Available on our website for encrypted communication

---

**Last Updated**: September 2025
**Next Review**: September 2026
