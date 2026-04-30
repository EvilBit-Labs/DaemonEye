# Security Policy

## Supported Versions

DaemonEye follows semantic versioning (SemVer) with security updates provided for the current major version and the previous major version.

| Version | Supported          | Notes                              |
| ------- | ------------------ | ---------------------------------- |
| 0.1.x   | :white_check_mark: | Current development version        |
| < 0.1   | :x:                | Pre-release versions not supported |

DaemonEye is in pre-1.0 development, so security updates target the latest 0.1.x series. After 1.0, we will support the current major version and one previous major version.

## Security Architecture

DaemonEye is split into three binaries with strict privilege separation, plus a shared library:

- **procmond**: privileged process collector. Runs elevated only as long as it needs to.
- **daemoneye-agent**: user-space orchestrator for detection and alert delivery.
- **daemoneye-cli**: read-only operator interface.
- **daemoneye-lib**: shared library (config, models, storage, detection, crypto). No direct privileges of its own.

### Security Principles

- **Least privilege**: Components run with the minimum permissions they need. procmond is the only component that ever runs elevated, and it drops privileges after collection setup.
- **Privilege separation**: procmond writes only to the audit ledger; daemoneye-agent reads the audit ledger and reads/writes the event store; daemoneye-cli is read-only.
- **Validated IPC**: Inter-process messages use protobuf with CRC32 framing checks. There are no inbound network listeners; alert delivery is outbound-only.
- **Audit trail**: Events are recorded in a BLAKE3 hash-chained ledger. A Certificate Transparency-style Merkle tree with inclusion proofs is in progress.

## Security Features

### Core Security Controls

- **Memory safety**: Built in Rust with `unsafe_code = "forbid"` at the workspace level.
- **Input validation**: External inputs (CLI args, config files, IPC messages, SQL rules) are validated at boundaries and rejected with actionable errors.
- **SQL injection prevention**: AST validation via sqlparser at rule load time [Implemented]. Execution-time enforcement of the SELECT-only/whitelist policy is [Planned]; the current engine uses category-based pattern matching.
- **Credential handling**: Secrets come from environment variables or the OS keychain. Nothing is hardcoded.
- **Attack surface**: No inbound network listeners. Alert delivery is outbound-only.
- **Audit trail**: BLAKE3 hash-chained audit ledger [Implemented]. Certificate Transparency-style Merkle tree inclusion proofs are [In Progress]; the generator currently returns an empty vec in `crypto.rs`.

### Planned Hardening (Community Tier)

- **Cryptographic integrity**: Merkle tree with inclusion proofs and periodic checkpoints. [In Progress]; chain hashing is implemented, inclusion proof generation is stubbed.
- **Code signing**: SLSA Level 3 provenance and Cosign signatures on releases. [Planned]
- **Sandboxed execution**: read-only database connections for the detection engine. [Planned]
- **Query whitelist**: SELECT-only with an approved-function list. [Implemented at rule load time; not yet enforced at execution time]

> Fleet-level transport security (mTLS between host agents and upstream aggregators) is provided by commercial tiers, sold separately, not in this repo.

## Reporting a Vulnerability

### How to Report

To report a security vulnerability in DaemonEye, contact us privately:

- Email: <support@evilbitlabs.io>
- PGP key: [available on our website](https://evilbitlabs.io/security)
- Subject line: `[SECURITY] DaemonEye Vulnerability Report`

### What to Include

In your report, please include:

1. **Description**: what the vulnerability is and which component(s) are affected.
2. **Impact**: what an attacker could do with it, and what data or capabilities are at risk.
3. **Reproduction**: steps to reproduce, if you have them.
4. **Environment**: OS, architecture, and DaemonEye version (`daemoneye-cli --version`).
5. **Timeline**: any disclosure timeline you need from us.
6. **Contact**: how you would like us to follow up.

### Response Timeline

- Initial acknowledgment: within 48 hours of receipt.
- Status updates: weekly while the report is under investigation.
- Resolution target: 30 days for critical issues.
- Disclosure: coordinated with the patch release.

### What to Expect

If we accept the report, you will get credit in the resulting advisory (if you want it) and early access to patches before public release.

If we decline, we will explain why and point you to a more appropriate reporting channel where one exists.

## Security Best Practices

### For Operators

- Run with the minimum privileges DaemonEye needs. procmond will request elevated privileges at startup if it has to and drop them after collection setup.
- Review the audit ledger periodically; tampering is detectable through the BLAKE3 chain.
- Keep DaemonEye updated. Advisories are published in the channels listed below.

### For Developers

- Security-relevant changes get reviewed before merge.
- CI runs `cargo audit` and `cargo deny check` on every commit, with no allow-listed advisories. New advisories fail the build.
- Security-critical components (input parsers, IPC framing, SQL validation) are covered by unit tests; fuzzing of these components is planned.

## Security Advisories

Security advisories are published at:

- **GitHub Security Advisories**: [github.com/EvilBit-Labs/DaemonEye/security/advisories](https://github.com/EvilBit-Labs/DaemonEye/security/advisories)
- **Security Website**: [evilbitlabs.io/security](https://evilbitlabs.io/security)
- **Mailing List**: Subscribe to <support@evilbitlabs.io>

## Responsible Disclosure

We work under coordinated disclosure: report privately first, give us reasonable time to ship a fix, and we will release the advisory together with the patch. Researchers who report in good faith get credit (if they want it) and will not be threatened with legal action.

## Accepted Risks (Dependencies)

We have no outstanding accepted risks.

### Advisory Resolution History

**March 2026.** Resolved RUSTSEC-2023-0089 (atomic-polyfill unmaintained) and RUSTSEC-2025-0141 (bincode unmaintained) by disabling default features on `postcard` (`default-features = false`) and enabling only the required `alloc` feature. This removed `heapless` and `atomic-polyfill` from the dependency tree entirely. Both advisory ignore entries were removed from `deny.toml`.

**January 2025.** Resolved RUSTSEC-2024-0384 (`instant`) and RUSTSEC-2024-0436 (`paste`) by removing the third-party `busrt` broker. The replacement in-house `daemoneye-eventbus` crate eliminated both advisories from the workspace.

### Operational Controls

- CI enforces `cargo audit` and `cargo deny check` with no allow-listed advisories.
- New vulnerabilities or unsound advisories fail CI; we do not silently ignore them.
- If we ever need to accept a risk, the rationale, scope, and removal plan get tracked in a public issue.

## Security Contact

For general security questions or concerns:

- **Email**: <support@evilbitlabs.io>
- **Website**: [evilbitlabs.io/security](https://evilbitlabs.io/security)
- **PGP**: Available on our website for encrypted communication

---

Last updated: March 2026. Next review: March 2027.
