# DaemonEye Pricing

> Part of the DaemonEye suite of tools: _Continuous monitoring. Immediate alerts._
>
> **"Auditd without the noise. Osquery without the bloat."**

## DaemonEye Architecture Note

ProcMonD is the privileged process monitoring component of the DaemonEye package. DaemonEye consists of three components:

- **ProcMonD (Collector):** Runs with high privileges, focused solely on process monitoring, with a minimal attack surface and no direct network functionality.
- **Orchestrator:** Operates in user space with very few privileges, receives events from ProcMonD, handles all network communication and alert delivery to log sinks, and communicates with ProcMonD only via secure, memory-only IPC (e.g., Unix sockets).
- **CLI:** Local command-line interface that interacts with the orchestrator for querying data, exporting results, and tuning service configuration. This separation ensures robust security: ProcMonD remains isolated and hardened, while orchestration/network tasks are delegated to a low-privilege process, and all user interaction is handled via the CLI through the orchestrator.

---

## Free / Homelab

**$0 — Always Free** For hackers, homelabbers, and operators who want clean visibility without SaaS strings.

- Full daemon (Rust core)
- SQL rule engine (DIY + community rules)
- Syslog, email, webhook alerts
- Tamper-evident logging
- Cross-platform (Linux, macOS, Windows)
- GitHub Sponsors tip jar if you dig it

_For the lab. For your side projects. For free, forever._

---

## Business

**Flat License — $199/site** For small teams and consultancies who need more polish and integrations. One-time fee, no subscription.

- Everything in Free
- Curated **rule packs** (malware TTPs, suspicious parent/child, process hollowing)
- Output connectors: Splunk HEC, Elastic, Kafka
- Container / K8s DaemonSet deployment
- Export to CEF, JSON, or STIX-lite
- Optional GUI frontend ($19 per seat)
- Signed installers (MSI/DMG, ready to deploy)

_Professional-grade monitoring you can actually run offline._

---

## Enterprise

**Org License — Let's Talk** For SOCs, IR teams, and industrial/government environments where process visibility is non-negotiable. (Pricing starts in the low 4-figures, one-time license. Optional paid update packs.)

- Everything in Business
- **eBPF integration** for kernel-level visibility
- **Central collector** for fleet monitoring
- Advanced SIEM integration (full STIX/TAXII, compliance mappings)
- Hardened builds with **SLSA provenance & Cosign signatures**
- Optional commercial license for enterprises who can't ship Apache 2.0
- Quarterly **Enterprise Rule Packs** with threat intel updates

_When compliance meets detection. Built for enclaves, critical infrastructure, and SOCs that need serious visibility._

---

### Notes

- No subscriptions. No license servers. No hidden telemetry.
- Free tier is fully functional — paid tiers add polish and scale.
- Pricing is a starting point — EvilBit Labs is not a sales shop, we keep it simple.

---

## Feature Comparison

| Feature                 | Free/Homelab | Business | Enterprise      |
| ----------------------- | ------------ | -------- | --------------- |
| **Core Monitoring**     | ✅           | ✅       | ✅              |
| **SQL Rule Engine**     | ✅           | ✅       | ✅              |
| **Basic Alerts**        | ✅           | ✅       | ✅              |
| **Cross-Platform**      | ✅           | ✅       | ✅              |
| **Curated Rule Packs**  | ❌           | ✅       | ✅              |
| **SIEM Connectors**     | ❌           | ✅       | ✅              |
| **Container Support**   | ❌           | ✅       | ✅              |
| **Export Formats**      | Basic        | CEF/STIX | Full STIX/TAXII |
| **GUI Frontend**        | ❌           | Optional | ✅              |
| **Kernel Monitoring**   | ❌           | ❌       | ✅              |
| **Fleet Management**    | ❌           | ❌       | ✅              |
| **Compliance Mappings** | ❌           | ❌       | ✅              |
| **Enterprise Support**  | ❌           | ❌       | ✅              |

## Getting Started

### Free Tier

1. Download from GitHub releases
2. Follow the [Installation Guide](./deployment/installation.md)
3. Start monitoring immediately
4. No registration required

### Business Tier

1. Contact EvilBit Labs for license key
2. Download Business tier build
3. Apply license key during installation
4. Access to curated rule packs and connectors

### Enterprise Tier

1. Contact EvilBit Labs for custom pricing
2. Discuss requirements and deployment scale
3. Receive tailored solution and support
4. Full enterprise features and support

## Support

- **Free Tier**: Community support via GitHub Issues
- **Business Tier**: Email support with 48-hour response
- **Enterprise Tier**: Dedicated support with SLA

## Contact

For Business and Enterprise licensing:

- **Email**: [support@evilbitlabs.com](mailto:support@evilbitlabs.com)
- **GitHub**: [EvilBit-Labs/daemoneye](https://github.com/EvilBit-Labs/daemoneye)
- **Website**: [evilbitlabs.io/daemoneye](https://evilbitlabs.io/daemoneye)

---

_Pricing is subject to change. Contact [EvilBit Labs](support@evilbitlabs.com) for the most current pricing information._
