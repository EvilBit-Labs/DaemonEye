# SentinelD Feature Tiers

SentinelD is offered in three distinct tiers, each designed to meet different organizational needs and deployment scales. All tiers maintain the core security-first architecture while adding progressively more advanced capabilities.

## Core Tier (Open Source)

**License**: Apache 2.0 **Target**: Individual users, small teams, proof-of-concept deployments

### Core Components

- **procmond**: Privileged process collector with minimal attack surface
- **sentinelagent**: User-space detection orchestrator with SQL-based rules
- **sentinelcli**: Command-line interface for queries and management
- **sentinel-lib**: Shared library with common functionality

### Key Features

- ✅ **Process Monitoring**: Cross-platform process enumeration and monitoring
- ✅ **SQL Detection Engine**: Flexible rule creation using standard SQL queries
- ✅ **Multi-Channel Alerting**: stdout, syslog, webhook, email, file output
- ✅ **Audit Logging**: Certificate Transparency-style Merkle tree with inclusion proofs
- ✅ **Offline Operation**: Full functionality without internet access
- ✅ **CLI Interface**: Comprehensive command-line management tools
- ✅ **Configuration Management**: Hierarchical configuration system
- ✅ **Cross-Platform Support**: Linux, macOS, Windows

### Performance Characteristics

- **CPU Usage**: \<5% sustained during continuous monitoring
- **Memory Usage**: \<100MB resident under normal operation
- **Process Enumeration**: \<5 seconds for 10,000+ processes
- **Database Operations**: >1,000 records/second write rate
- **Alert Latency**: \<100ms per detection rule execution

### Use Cases

- Individual security researchers and analysts
- Small development teams requiring process monitoring
- Proof-of-concept security deployments
- Educational and training environments
- Airgapped or offline environments

---

## Business Tier (Commercial)

**License**: $199/site (one-time) **Target**: Small to medium teams, consultancies, managed security services

### All Core Tier Features Plus

#### **Security Center Server**

- **Centralized Management**: Single point of control for multiple agents
- **Agent Registration**: Secure mTLS-based agent authentication
- **Data Aggregation**: Centralized collection of alerts and process data
- **Configuration Distribution**: Centralized rule management and deployment
- **Integration Hub**: Single point for external SIEM integrations

#### **Web GUI Frontend**

- **Fleet Dashboard**: Real-time view of all connected agents
- **Alert Management**: Filtering, sorting, and export of alerts
- **Rule Management**: Visual rule editor and deployment interface
- **System Health**: Agent connectivity and performance metrics
- **Data Visualization**: Charts and graphs for security analytics

#### **Enhanced Output Connectors**

- **Splunk HEC**: Native Splunk HTTP Event Collector integration
- **Elasticsearch**: Bulk indexing with index pattern management
- **Kafka**: High-throughput message streaming
- **CEF Format**: Common Event Format for SIEM compatibility
- **STIX 2.1**: Structured Threat Information eXpression export

#### **Curated Rule Packs**

- **Malware TTPs**: Common malware tactics, techniques, and procedures
- **MITRE ATT&CK**: Framework-based detection rules
- **Industry Standards**: CIS, NIST, and other compliance frameworks
- **Cryptographic Signatures**: Ed25519-signed rule packs for integrity
- **Auto-Update**: Automatic rule pack distribution and updates

#### **Container & Kubernetes Support**

- **Docker Images**: Pre-built container images for all components
- **Kubernetes Manifests**: DaemonSet and deployment configurations
- **Helm Charts**: Package management for Kubernetes deployments
- **Service Mesh**: Istio and Linkerd integration support

#### **Deployment Patterns**

- **Direct Agent-to-SIEM**: Agents send directly to configured SIEM systems
- **Centralized Proxy**: All agents route through Security Center
- **Hybrid Mode**: Agents send to both Security Center and direct SIEM (recommended)

### Performance Characteristics

- **Agents per Security Center**: 1,000+ agents
- **Alert Throughput**: 10,000+ alerts per minute
- **Data Retention**: Configurable retention policies
- **Query Performance**: Sub-second queries across agent fleet

### Use Cases

- Security consultancies managing multiple clients
- Managed Security Service Providers (MSSPs)
- Small to medium enterprises with distributed infrastructure
- Organizations requiring centralized security management
- Teams needing enhanced SIEM integration

---

## Enterprise Tier (Commercial)

**License**: $199/site (one-time) **Target**: Large enterprises, government agencies, critical infrastructure

### All Business Tier Features Plus

#### **Kernel-Level Monitoring**

- **Linux eBPF**: Real-time syscall monitoring and process tracking
- **Windows ETW**: Event Tracing for Windows integration
- **macOS EndpointSecurity**: Native security framework integration
- **Container Awareness**: Kubernetes and Docker container monitoring
- **Network Correlation**: Process-to-network activity correlation

#### **Federated Security Centers**

- **Hierarchical Architecture**: Regional and Primary Security Centers
- **Distributed Queries**: Cross-center query execution and aggregation
- **Data Replication**: Automatic data synchronization between centers
- **Failover Support**: Automatic failover and load balancing
- **Geographic Distribution**: Multi-region deployment support

#### **Advanced Threat Intelligence**

- **STIX/TAXII Integration**: Automated threat intelligence ingestion
- **Indicator Conversion**: STIX indicators to detection rules
- **Threat Feed Management**: Multiple threat intelligence sources
- **IOC Matching**: Indicator of Compromise correlation
- **Threat Hunting**: Advanced query capabilities for threat hunting

#### **Enterprise Analytics**

- **Distributed Analytics**: Cross-fleet security analytics
- **Machine Learning**: Anomaly detection and behavioral analysis
- **Risk Scoring**: Dynamic risk assessment and prioritization
- **Compliance Reporting**: Automated compliance and audit reporting
- **Custom Dashboards**: Configurable security dashboards

#### **Advanced Security Features**

- **Zero Trust Architecture**: Comprehensive zero trust implementation
- **Identity Integration**: Active Directory and LDAP integration
- **Role-Based Access Control**: Granular permission management
- **Audit Trail**: Comprehensive audit logging and compliance
- **Data Encryption**: End-to-end encryption for all data flows

#### **High Availability & Scalability**

- **Clustering**: Multi-node Security Center clusters
- **Load Balancing**: Automatic load distribution
- **Disaster Recovery**: Backup and recovery procedures
- **Horizontal Scaling**: Scale-out architecture support
- **Performance Optimization**: Advanced caching and optimization

### Performance Characteristics

- **Agents per Federation**: 10,000+ agents
- **Regional Centers**: 100+ regional centers per federation
- **Query Latency**: \<100ms for distributed queries
- **Data Volume**: Petabyte-scale data processing
- **Uptime**: 99.99% availability SLA

### Use Cases

- Large enterprises with global infrastructure
- Government agencies and critical infrastructure
- Financial services and healthcare organizations
- Organizations requiring compliance (SOX, HIPAA, PCI-DSS)
- Multi-tenant service providers

---

## Feature Comparison Matrix

| Feature                    | Core | Business | Enterprise |
| -------------------------- | ---- | -------- | ---------- |
| **Process Monitoring**     | ✅   | ✅       | ✅         |
| **SQL Detection Engine**   | ✅   | ✅       | ✅         |
| **Multi-Channel Alerting** | ✅   | ✅       | ✅         |
| **Audit Logging**          | ✅   | ✅       | ✅         |
| **Offline Operation**      | ✅   | ✅       | ✅         |
| **CLI Interface**          | ✅   | ✅       | ✅         |
| **Security Center**        | ❌   | ✅       | ✅         |
| **Web GUI**                | ❌   | ✅       | ✅         |
| **Enhanced Connectors**    | ❌   | ✅       | ✅         |
| **Curated Rule Packs**     | ❌   | ✅       | ✅         |
| **Container Support**      | ❌   | ✅       | ✅         |
| **Kernel Monitoring**      | ❌   | ❌       | ✅         |
| **Federation**             | ❌   | ❌       | ✅         |
| **STIX/TAXII**             | ❌   | ❌       | ✅         |
| **Advanced Analytics**     | ❌   | ❌       | ✅         |
| **Zero Trust**             | ❌   | ❌       | ✅         |
| **High Availability**      | ❌   | ❌       | ✅         |

## Licensing Architecture

### **Dual-License Strategy**

The SentinelD project maintains a dual-license approach to balance open source accessibility with commercial sustainability:

- **Core Components**: Apache 2.0 licensed (procmond, sentinelagent, sentinelcli, sentinel-lib)
- **Business Tier Features**: $199/site one-time license (Security Center, GUI, enhanced connectors, curated rules)
- **Enterprise Tier Features**: Custom pricing (kernel monitoring, federation, STIX/TAXII integration)

### **Feature Gating Implementation**

```rust
// Compile-time feature gates
#[cfg(feature = "business-tier")]
pub mod security_center;

#[cfg(feature = "business-tier")]
pub mod enhanced_connectors;

#[cfg(feature = "enterprise-tier")]
pub mod kernel_monitoring;

#[cfg(feature = "enterprise-tier")]
pub mod federation;
```

### **Runtime License Validation**

- **Cryptographic Signatures**: Ed25519 signatures for license validation
- **Site Restrictions**: Hostname/domain matching for license compliance
- **Feature Activation**: Runtime feature activation based on license
- **Graceful Degradation**: Fallback to lower tier when license is invalid

### **License Distribution**

- **Core Tier**: GitHub releases with Apache 2.0 license
- **Business Tier**: Separate distribution channel with license keys
- **Enterprise Tier**: Enterprise distribution with support and SLA
- **Hybrid Builds**: Single binary with runtime feature activation

## Migration Path

### **Core → Business**

- Install Security Center server
- Configure agent uplink connections
- Deploy curated rule packs
- Set up enhanced connectors

### **Business → Enterprise**

- Enable kernel-level monitoring
- Deploy federated Security Centers
- Integrate STIX/TAXII feeds
- Configure advanced analytics

### **Backward Compatibility**

- All tiers maintain API compatibility
- Configuration migration tools provided
- Data export/import capabilities
- Gradual feature activation

---

*Choose the tier that best fits your organization's needs, with the flexibility to upgrade as requirements grow and evolve.*
