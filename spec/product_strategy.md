# Product Strategy

## Overview

### Naming Glossary

- Product: DaemonEye
- Components:
  - procmond (Privileged Process Collector)
  - daemoneye-agent (Detection Orchestrator)
  - daemoneye-cli (Operator CLI)
  - daemoneye-lib (Shared library)
  - collector-core (Collector framework)

DaemonEye is an agent-centric system monitoring tool that operates autonomously on each host to collect and analyze process information locally. The daemoneye-agent component runs independently on each system, collecting process data and executing detection rules without requiring a central server for core functionality.

For enterprise deployments, an optional central server can be deployed to aggregate data from multiple agents, provide centralized management, and enable fleet-wide analysis. However, the core monitoring and detection capabilities function entirely within the local agent, ensuring the system remains operational even in airgapped or isolated environments.

## Core Functionality

Setting aside the high-level mission statement, the core functionality of the product is to allow administrators to run queries against the running system to detect suspicious activity. This is done by using a SQL-like query language that is translated into a set of detection tasks for "monitoring" collectors that run on the system and watch processes, files, network connections, and other system resources. When these tasks detect the portion of the "query" they are responsible for, they will trigger additional enrichment by "triggerable" collectors that will run in parallel and provide additional context for the detection. Combining these multiple collectors to return results in the form of a virtual "table" that can be queried against using the same SQL-like language is the core of the system functionality.

The daemoneye-agent is responsible for taking the "SQL queries" and turning them into "detection tasks" that are then sent to the appropriate collectors, and then collecting the results and maintaining the virtual database structure.

## Prioritization

Given the core functionality, the most important features are:

- A robust SQL-like query language that is easy to learn and use and allows for complex queries with as much cross-platform consistency as possible
- A set of "monitoring" collectors that are responsible for watching the system and detecting suspicious activity, collecting as much as possible while remaining as small and efficient as possible
- A set of "triggerable" collectors that are responsible for providing additional context for the detection, gathering as much as possible while being consistent and easy to write queries against
- A set of "alerting" sinks that are responsible for delivering the results to the administrator

From a more practical standpoint, that means that the only real way to truly enhance the project core is to continue to standardize procmond, the collector-core framework, and the agent. While we can always add more sinks, more central monitoring and management, and easier deployment, those features are generally not core to the project's success and are geared more for the business and enterprise customers. The emphesis for this repository is define an extensive and robust collector-core framework that can be used to build a wide variety of collectors and agents, and then to build a wide variety of agents that can be used to detect suspicious activity on the system. While each platform does offer an open metadata value, which can contain platform-specific data, the goal is to find the best features for each platform and try to offer them in a consistent way across all platforms. We will be calling these "collector profiles" and they will be defined similar to database schemas.

### Collector Types

- `monitor`: These collectors are responsible for monitoring the processes through either subscription or polling. They will be responsible for collecting the most basic metadata about the process and then triggering the triggerable collectors to collect more detailed metadata. They will run as daemons, with their lifecycle managed by the daemoneye-agent, and will typically run for the entire time that the daemoneye-agent is running. They will only poll if they have alert tasks or if they do not have the ability to subscribe to the process events.
- `triggerable`: These collectors are responsible for collecting more detailed metadata about the process. They do not poll or subscribe to events and will only examine data when provided a task from the daemoneye-agent from a monitor collector. They will populate their own virtual tables in the agent's virtual database on demand. They will run as daemons, with their lifecycle managed by the daemoneye-agent, and will remain idle awaiting a task provided over their IPC channel. Their tasking is generally setup using the special DaemonEye SQL dialect syntax `AUTO JOIN` to automatically collect the data they need (see `daemon_eye_spec_sql_to_ipc_detection_architecture.md` for more details).

### Collector Roadmap

The collector framework exposes virtual tables that can be queried using SQL-like syntax. Each collector provides specific data schemas that represent system resources and analysis results.

#### Composite Primary Key

- All process-scoped virtual tables MUST use a composite primary key of: (pid, \<table_primary_time_field>, process_instance_id)
- process_instance_id is a deterministic UUIDv5 computed as:
  - Namespace: "daemoneye/process"
  - Name string: "{boot_id}:{pid}:{process_start_time_ns}"
- Boot ID source (platform-specific):
  - Linux: /proc/sys/kernel/random/boot_id (primary); fallback: deterministic UUIDv5 from normalized btime timestamp
  - macOS: kern.bootsessionuuid (primary); fallback: deterministic UUIDv5 from normalized kern.boottime
  - Windows: Win32_OperatingSystem.LastBootUpTime (WMI) rendered as RFC 3339 UTC; no fallback (avoid clock arithmetic)
  - FreeBSD: deterministic UUIDv5 from normalized kern.boottime timestamp
- Timestamps:
  - All times MUST be stored in UTC using RFC 3339 format
  - Nanosecond precision MUST be used when available; otherwise, use the highest precision provided by the OS
  - **Mixed-precision handling**: Normalize all timestamps to RFC 3339 with nanoseconds padded/truncated to consistent canonical form
  - Record precision metadata when true nanoseconds are unavailable (e.g., `precision: "microseconds"`)
- Table requirements:
  - Every virtual table MUST include process_instance_id
  - Each table MUST explicitly specify its primary time field used in the composite key
- **Primary Time Field Mapping:**
  - `processes`: `start_time`
  - `network_connections`: `created_time`
  - `file_events`: `event_time`
  - `memory_patterns`: `scan_time`
  - `heap_analysis`: `analysis_time`
  - `exploit_detection`: `analysis_time`
  - `protocol_analysis`: `analysis_time`

Triggerable collectors are not allowed to trigger other triggerable collectors. The daemoneye-agent will be responsible for triggering the appropriate collectors based on the SQL query and, when all data is returned, the daemoneye-agent will then trigger the alerting sinks to deliver the results. The alerting sinks will be responsible for delivering the results to the administrator. This decoupling allows the individual components to be more focused and have a reduced attack surface.

#### Monitor Collectors (Continuous System Monitoring)

##### Processes Collector (procmond)

Status: Core framework implemented, cross-platform optimizations in progress

**Virtual Table: `processes`**

Core fields available across all platforms (provided by `sysinfo`):

- `pid` (integer): Process ID (effective PK)
- `ppid` (integer): Parent Process ID
- `name` (string): Process name
- `executable_path` (string): Full path to executable
- `command_line` (string): Complete command line arguments
- `state` (string): Process state (running, sleeping, stopped, zombie)
- `cpu_usage` (float): CPU usage percentage
- `accumulated_cpu_time` (float): Total accumulated CPU time in milliseconds
- `memory_rss` (integer): Resident Set Size in bytes
- `memory_vms` (integer): Virtual Memory Size in bytes
- `start_time` (timestamp): Process start time
- `run_time` (integer): Process run time in seconds
- `user_id` (integer): User ID of process owner
- `effective_user_id` (integer): Effective user ID of process owner
- `group_id` (integer): Group ID of process owner
- `effective_group_id` (integer): Effective group ID of process owner
- `working_directory` (string): Current working directory
- `environment_variables` (json): Process environment variables
- `disk_usage` (json): Disk usage statistics (read/write bytes)
- `thread_count` (integer): Number of threads in the process
- `priority` (integer): Process priority
- `nice_value` (integer): Process nice value
- `parent_process` (string): Parent process name
- `root_directory` (string): Process root directory
- `session_id` (integer): Process session ID

**Table primary time field:** `start_time` (used in composite primary key: pid + start_time + process_instance_id)

**Sensitive Data Handling:**

Fields likely to contain PII/secrets (marked with `sensitive: true`):

- `environment_variables` (json): Process environment variables (sensitive: true)
- `command_line` (string): Complete command line arguments (sensitive: true)
- `working_directory` (string): Current working directory (sensitive: true)
- `root_directory` (string): Process root directory (sensitive: true)

**Privacy Controls:**

- **Redaction Rules**: Mask API keys, tokens, passwords using regex patterns (e.g., `(?i)(password|token|key|secret)\s*[:=]\s*\S+`)
- **Encryption-at-Rest**: All sensitive fields must be encrypted in database storage
- **Access Controls**: Sensitive fields require elevated permissions and audit logging
- **Retention Policy**: Sensitive data retention limited to 30 days with automated purge
- **Compliance Mapping**: GDPR (right to erasure), CCPA (data minimization), HIPAA (PHI protection), SOC2 (data handling)

**Platform-Specific Extensions:**

Linux extensions (provided by `sysinfo` and `procfs` crates):

- `memory_maps` (json): Memory mapping information from /proc/[pid]/maps
- `file_descriptors` (json): Open file descriptors from /proc/[pid]/fd
- `network_connections` (json): Network connections from /proc/[pid]/net
- `namespaces` (json): Process namespace information
- `capabilities` (json): Linux capability sets
- `security_context` (string): SELinux/AppArmor security context
- `io_statistics` (json): I/O statistics from /proc/[pid]/io
- `limits` (json): Process resource limits from /proc/[pid]/limits
- `oom_score` (integer): Out-of-memory score
- `oom_score_adj` (integer): OOM score adjustment
- `cgroup` (json): Control group information
- `autogroup` (json): Process autogroup information
- `clear_refs` (json): Clear references information
- `coredump_filter` (json): Core dump filter settings
- `mountinfo` (json): Mount information
- `mountstats` (json): Mount statistics
- `smaps` (json): Detailed memory mapping from /proc/[pid]/smaps
- `smaps_rollup` (json): Rolled-up memory mapping summary
- `wchan` (string): Wait channel information
- `task_stat` (json): Per-task statistics from /proc/[pid]/task/[tid]/stat
- `task_status` (json): Per-task status from /proc/[pid]/task/[tid]/status
- `task_io` (json): Per-task I/O statistics from /proc/[pid]/task/[tid]/io
- `task_children` (json): Child processes from /proc/[pid]/task/[tid]/children

macOS extensions (provided by `sysinfo` and `mach2` crates):

- `code_signing` (json): Code signing information and entitlements
- `bundle_info` (json): Application bundle metadata
- `sip_protected` (boolean): System Integrity Protection status
- `sandboxed` (boolean): Sandboxed process detection
- `integrity_level` (string): Process integrity level
- `task_info` (json): Mach task information including virtual memory statistics
- `thread_info` (json): Thread information and scheduling details
- `vm_region` (json): Virtual memory region information
- `vm_statistics` (json): Virtual memory statistics
- `thread_act` (json): Thread activation information
- `thread_policy` (json): Thread scheduling policy
- `thread_status` (json): Thread status and registers
- `port_info` (json): Mach port information
- `semaphore_info` (json): Semaphore information
- `clock_info` (json): Clock and timing information
- `exception_info` (json): Exception handling information
- `dyld_info` (json): Dynamic linker information
- `loader_info` (json): Mach-O loader information

Windows extensions (provided by `sysinfo` and `windows-rs` crates):

- `process_token` (json): Process security token information
- `protected_process` (boolean): Protected process detection
- `system_process` (boolean): System process identification
- `uac_elevated` (boolean): UAC elevation status
- `windows_service` (boolean): Windows service detection
- `session_id` (integer): Terminal session ID
- `integrity_level` (string): Process integrity level
- `handle_count` (integer): Number of open handles
- `page_faults` (integer): Page fault count
- `working_set` (integer): Working set size
- `peak_working_set` (integer): Peak working set size
- `gpu_usage` (float): GPU usage percentage
- `gpu_memory_used` (integer): GPU memory usage in bytes
- `thread_count` (integer): Number of threads
- `priority_class` (string): Process priority class
- `affinity_mask` (integer): CPU affinity mask
- `creation_time` (timestamp): Process creation time
- `exit_time` (timestamp): Process exit time
- `kernel_time` (integer): Kernel mode time
- `user_time` (integer): User mode time
- `security_descriptor` (json): Security descriptor information
- `job_object` (json): Job object information
- `window_station` (string): Window station name
- `desktop` (string): Desktop name
- `window_title` (string): Main window title
- `command_line` (string): Full command line
- `environment_variables` (json): Environment variables
- `dll_list` (json): Loaded DLL information
- `registry_access` (json): Registry access information
- `file_access` (json): File access information
- `network_access` (json): Network access information

##### Network Collector (netmond)

Status: Planned

**Virtual Table: `network_connections`**

Core fields available across all platforms (provided by `sysinfo`):

- `pid` (integer): Process ID owning the connection (effective PK)
- `connection_id` (string): Unique connection identifier (RFC4122 v5 UUID derived from 5-tuple: protocol + local_address + local_port + remote_address + remote_port + socket_inode + created_time_ms)
- `protocol` (string): Protocol (tcp, udp, unix)
- `local_address` (string): Local IP address
- `local_port` (integer): Local port number
- `remote_address` (string): Remote IP address
- `remote_port` (integer): Remote port number
- `state` (string): Connection state (established, listening, time_wait, etc.)
- `process_name` (string): Name of the owning process
- `created_time` (timestamp): Connection creation time
- `last_activity` (timestamp): Last activity time
- `inode` (integer): Socket inode number
- `socket_type` (string): Socket type (stream, dgram, raw, etc.)
- `family` (string): Address family (inet, inet6, unix)
- `flags` (integer): Socket flags
- `backlog` (integer): Listen backlog queue length
- `receive_queue` (integer): Receive queue length
- `send_queue` (integer): Send queue length
- `uid` (integer): User ID of socket owner
- `gid` (integer): Group ID of socket owner

**Table primary time field:** `created_time` (used in composite primary key: pid + connection_id + created_time)

**Virtual Table: `network_interfaces`**

Core fields available across all platforms (provided by `sysinfo`):

- `interface_name` (string): Network interface name
- `interface_type` (string): Interface type (ethernet, wifi, loopback, etc.)
- `status` (string): Interface status (up, down, unknown)
- `bytes_sent` (integer): Total bytes sent
- `bytes_received` (integer): Total bytes received
- `packets_sent` (integer): Total packets sent
- `packets_received` (integer): Total packets received
- `errors_sent` (integer): Send errors
- `errors_received` (integer): Receive errors
- `drops_sent` (integer): Send drops
- `drops_received` (integer): Receive drops
- `mac_address` (string): MAC address of the interface
- `ip_networks` (json): IP networks associated with the interface
- `mtu` (integer): Maximum Transmission Unit
- `speed` (integer): Interface speed in Mbps
- `duplex` (string): Duplex mode (full, half, unknown)
- `carrier` (boolean): Carrier detection status
- `operstate` (string): Operational state
- `link_mode` (string): Link mode
- `address` (string): Interface address
- `broadcast` (string): Broadcast address
- `netmask` (string): Network mask
- `rx_bytes` (integer): Bytes received since last refresh
- `tx_bytes` (integer): Bytes transmitted since last refresh
- `rx_packets` (integer): Packets received since last refresh
- `tx_packets` (integer): Packets transmitted since last refresh
- `rx_errors` (integer): Receive errors since last refresh
- `tx_errors` (integer): Transmit errors since last refresh
- `rx_dropped` (integer): Receive drops since last refresh
- `tx_dropped` (integer): Transmit drops since last refresh

**Platform-Specific Extensions:**

Linux extensions (provided by `sysinfo` and `procfs` crates):

- `namespace_id` (integer): Network namespace ID
- `container_id` (string): Container identifier
- `vlan_id` (integer): VLAN ID
- `bond_master` (string): Bonding master interface
- `mtu` (integer): Maximum Transmission Unit
- `arp_table` (json): ARP table entries from /proc/net/arp
- `route_table` (json): Routing table from /proc/net/route
- `tcp_connections` (json): TCP connections from /proc/net/tcp
- `udp_connections` (json): UDP connections from /proc/net/udp
- `unix_sockets` (json): Unix domain sockets from /proc/net/unix
- `snmp_stats` (json): SNMP statistics from /proc/net/snmp
- `snmp6_stats` (json): SNMP6 statistics from /proc/net/snmp6
- `net_dev_stats` (json): Network device statistics from /proc/net/dev
- `tcp_mem` (json): TCP memory usage from /proc/net/tcp_mem
- `tcp_congestion` (string): TCP congestion control algorithm
- `tcp_window_scaling` (boolean): TCP window scaling enabled
- `tcp_timestamps` (boolean): TCP timestamps enabled
- `tcp_sack` (boolean): TCP SACK enabled
- `tcp_fack` (boolean): TCP FACK enabled
- `tcp_dsack` (boolean): TCP DSACK enabled
- `tcp_ecn` (boolean): TCP ECN enabled
- `tcp_abc` (boolean): TCP ABC enabled
- `tcp_syncookies` (boolean): TCP SYN cookies enabled
- `tcp_fastopen` (boolean): TCP Fast Open enabled
- `tcp_autocorking` (boolean): TCP auto corking enabled
- `tcp_no_delay_ack` (boolean): TCP no delay ACK enabled
- `tcp_thin_linear_timeouts` (boolean): TCP thin linear timeouts enabled
- `tcp_thin_dupack` (boolean): TCP thin duplicate ACK enabled
- `tcp_early_retrans` (boolean): TCP early retransmission enabled
- `tcp_reordering` (integer): TCP reordering threshold
- `tcp_retrans_collapse` (boolean): TCP retrans collapse enabled
- `tcp_keepalive_time` (integer): TCP keepalive time
- `tcp_keepalive_probes` (integer): TCP keepalive probes
- `tcp_keepalive_intvl` (integer): TCP keepalive interval
- `tcp_retries1` (integer): TCP retries 1
- `tcp_retries2` (integer): TCP retries 2
- `tcp_orphan_retries` (integer): TCP orphan retries
- `tcp_tw_reuse` (boolean): TCP TIME_WAIT socket reuse
- `tcp_fin_timeout` (integer): TCP FIN timeout
- `tcp_tw_recycle` (boolean): TCP TIME_WAIT socket recycling
- `tcp_max_tw_buckets` (integer): TCP maximum TIME_WAIT buckets
- `tcp_max_syn_backlog` (integer): TCP maximum SYN backlog
- `tcp_syn_retries` (integer): TCP SYN retries
- `tcp_synack_retries` (integer): TCP SYN-ACK retries
- `tcp_abort_on_overflow` (boolean): TCP abort on overflow
- `tcp_stdurg` (boolean): TCP strict URG
- `tcp_rfc1337` (boolean): TCP RFC 1337

macOS extensions (provided by `sysinfo` and `mach2` crates):

- `service_name` (string): Network service name
- `bonjour_services` (json): Bonjour service discovery
- `airport_info` (json): WiFi airport information
- `energy_impact` (float): Network energy impact
- `network_service_order` (json): Network service order configuration
- `dns_configuration` (json): DNS configuration
- `proxy_settings` (json): Proxy settings
- `firewall_status` (string): Firewall status
- `stealth_mode` (boolean): Stealth mode enabled
- `block_all_incoming` (boolean): Block all incoming connections
- `application_firewall` (json): Application firewall rules
- `network_location` (string): Current network location
- `vpn_connections` (json): VPN connection information
- `wifi_networks` (json): Available WiFi networks
- `bluetooth_devices` (json): Bluetooth device information
- `network_interfaces_detailed` (json): Detailed interface information
- `routing_table` (json): Routing table information
- `arp_cache` (json): ARP cache entries
- `network_statistics` (json): Network statistics
- `socket_statistics` (json): Socket statistics
- `tcp_statistics` (json): TCP statistics
- `udp_statistics` (json): UDP statistics
- `icmp_statistics` (json): ICMP statistics
- `ip_statistics` (json): IP statistics
- `interface_statistics` (json): Interface statistics
- `network_quality` (json): Network quality metrics
- `bandwidth_usage` (json): Bandwidth usage statistics
- `connection_history` (json): Connection history
- `network_diagnostics` (json): Network diagnostics information

Windows extensions (provided by `sysinfo` and `windows-rs` crates):

- `adapter_guid` (string): Network adapter GUID
- `dhcp_enabled` (boolean): DHCP enabled status
- `dns_servers` (json): DNS server configuration
- `firewall_status` (string): Windows Firewall status
- `hyper_v_vswitch` (string): Hyper-V virtual switch
- `network_adapter_info` (json): Network adapter information
- `ip_configuration` (json): IP configuration
- `routing_table` (json): Routing table
- `arp_table` (json): ARP table
- `netstat_connections` (json): Netstat connection information
- `tcp_connections` (json): TCP connections
- `udp_connections` (json): UDP connections
- `network_interfaces` (json): Network interfaces
- `network_profiles` (json): Network profiles
- `wifi_profiles` (json): WiFi profiles
- `bluetooth_devices` (json): Bluetooth devices
- `vpn_connections` (json): VPN connections
- `network_bridge` (json): Network bridge information
- `network_team` (json): Network team information
- `network_lbfo` (json): Network LBFO information
- `network_qos` (json): Network QoS information
- `network_security` (json): Network security settings
- `network_monitoring` (json): Network monitoring information
- `network_diagnostics` (json): Network diagnostics
- `network_performance` (json): Network performance metrics
- `network_usage` (json): Network usage statistics
- `network_events` (json): Network events
- `network_logs` (json): Network logs
- `network_traces` (json): Network traces
- `network_captures` (json): Network captures
- `network_analysis` (json): Network analysis
- `network_forensics` (json): Network forensics
- `network_compliance` (json): Network compliance
- `network_audit` (json): Network audit information

##### Filesystem Collector (fsmond)

Status: Planned

**Virtual Table: `file_events`**

Core fields available across all platforms (provided by `sysinfo` and platform-specific crates):

- `process_pid` (integer): Process ID that triggered the event (effective PK)
- `event_id` (string): Unique event identifier
- `canonical_id` (string): Cross-platform canonical identifier (foreign key to `file_metadata.canonical_id`)
- `event_type` (string): Event type (create, modify, delete, access, move)
- `file_path` (string): Full path to the file (foreign key to `file_metadata.file_path` on Windows)
- `file_name` (string): File name only
- `directory` (string): Directory containing the file
- `file_size` (integer): File size in bytes
- `file_type` (string): File type (regular, directory, symlink, etc.)
- `permissions` (string): File permissions (octal or symbolic)
- `owner_user` (string): File owner username
- `owner_group` (string): File owner group
- `created_time` (timestamp): File creation time
- `modified_time` (timestamp): File modification time
- `accessed_time` (timestamp): File access time
- `event_time` (timestamp): When the event occurred
- `process_name` (string): Process name that triggered the event
- `mount_point` (string): Mount point containing the file
- `filesystem_type` (string): Type of filesystem
- `device_id` (integer): Device ID containing the file
- `inode` (integer): Inode number (Linux/Unix) (foreign key to `file_metadata.inode` on Unix systems)
- `hard_links` (integer): Number of hard links
- `block_size` (integer): Filesystem block size
- `blocks_allocated` (integer): Number of blocks allocated
- `file_mode` (integer): File mode bits
- `file_uid` (integer): File owner UID
- `file_gid` (integer): File owner GID
- `file_flags` (integer): File flags (Linux)
- `file_generation` (integer): File generation number
- `file_version` (integer): File version
- `file_attributes` (integer): File attributes
- `file_creation_time` (timestamp): File creation time (Windows)
- `file_last_write_time` (timestamp): Last write time (Windows)
- `file_last_access_time` (timestamp): Last access time (Windows)
- `file_change_time` (timestamp): Last change time (Linux/Unix)

**Join Logic for file_events ↔ file_metadata:**

- **Cross-platform**: Use `file_events.canonical_id = file_metadata.canonical_id`
- **Unix/Linux**: Use `file_events.inode = file_metadata.inode AND file_events.device_id = file_metadata.device_id`
- **Windows**: Use `file_events.file_path = file_metadata.file_path`

**Virtual Table: `file_metadata`**

> [!NOTE]
> `file_metadata` is system-scoped (not process-scoped). Primary keys are platform-specific:
>
> - **Unix/Linux**: Primary key = `inode` + `device_id`
> - **Windows**: Primary key = `file_path`
> - **Cross-platform**: Use `canonical_id` (computed hash) for joins

Core fields available across all platforms (provided by `sysinfo` and platform-specific crates):

- `canonical_id` (string): Cross-platform canonical identifier (computed hash for joins)
- `file_path` (string): Full path to the file
- `file_name` (string): File name only
- `directory` (string): Directory containing the file
- `file_size` (integer): File size in bytes
- `file_type` (string): File type (regular, directory, symlink, etc.)
- `permissions` (string): File permissions
- `owner_user` (string): File owner username
- `owner_group` (string): File owner group
- `created_time` (timestamp): File creation time
- `modified_time` (timestamp): File modification time
- `accessed_time` (timestamp): File access time
- `sha256_hash` (string): SHA-256 hash of file contents
- `mount_point` (string): Mount point containing the file
- `filesystem_type` (string): Type of filesystem
- `device_id` (integer): Device ID containing the file
- `inode` (integer): Inode number (Linux/Unix) (effective primary key in file_metadata on Unix systems; referenced by file_events.inode as a foreign key)
- `hard_links` (integer): Number of hard links
- `block_size` (integer): Filesystem block size
- `blocks_allocated` (integer): Number of blocks allocated
- `file_mode` (integer): File mode bits
- `file_uid` (integer): File owner UID
- `file_gid` (integer): File owner GID
- `file_flags` (integer): File flags (Linux)
- `file_generation` (integer): File generation number
- `file_version` (integer): File version
- `file_attributes` (integer): File attributes
- `file_creation_time` (timestamp): File creation time (Windows)
- `file_last_write_time` (timestamp): Last write time (Windows)
- `file_last_access_time` (timestamp): Last access time (Windows)
- `file_change_time` (timestamp): Last change time (Linux/Unix)
- `symlink_target` (string): Symbolic link target (if applicable)
- `file_extension` (string): File extension
- `mime_type` (string): MIME type of the file
- `file_encoding` (string): File encoding
- `file_compression` (string): Compression type
- `file_encryption` (string): Encryption status
- `file_backup` (boolean): Backup status
- `file_archive` (boolean): Archive status
- `file_hidden` (boolean): Hidden file status
- `file_system` (boolean): System file status
- `file_readonly` (boolean): Read-only status
- `file_executable` (boolean): Executable status
- `file_directory` (boolean): Directory status
- `file_symlink` (boolean): Symbolic link status
- `file_socket` (boolean): Socket status
- `file_pipe` (boolean): Named pipe status
- `file_device` (boolean): Device file status
- `file_special` (boolean): Special file status
- `file_system_events` (json): Real-time file system events (via `notify` crate)
- `file_watcher_events` (json): File watcher events (via `notify` crate)
- `directory_changes` (json): Directory change notifications (via `notify` crate)
- `file_creation_events` (json): File creation events (via `notify` crate)
- `file_modification_events` (json): File modification events (via `notify` crate)
- `file_deletion_events` (json): File deletion events (via `notify` crate)
- `file_rename_events` (json): File rename events (via `notify` crate)
- `file_access_events` (json): File access events (via `notify` crate)
- `file_permission_events` (json): File permission change events (via `notify` crate)
- `file_ownership_events` (json): File ownership change events (via `notify` crate)
- `file_size_events` (json): File size change events (via `notify` crate)
- `file_content_events` (json): File content change events (via `notify` crate)
- `file_metadata_events` (json): File metadata change events (via `notify` crate)
- `file_symlink_events` (json): Symbolic link events (via `notify` crate)
- `file_hardlink_events` (json): Hard link events (via `notify` crate)
- `file_special_events` (json): Special file events (via `notify` crate)
- `file_device_events` (json): Device file events (via `notify` crate)
- `file_socket_events` (json): Socket file events (via `notify` crate)
- `file_pipe_events` (json): Named pipe events (via `notify` crate)
- `file_fifo_events` (json): FIFO events (via `notify` crate)
- `file_block_events` (json): Block device events (via `notify` crate)
- `file_character_events` (json): Character device events (via `notify` crate)
- `file_directory_events` (json): Directory events (via `notify` crate)
- `file_regular_events` (json): Regular file events (via `notify` crate)

**Platform-Specific Extensions:**

Linux extensions (provided by `sysinfo` and `procfs` crates):

- `extended_attributes` (json): Extended attributes (xattr)
- `acl_entries` (json): Access Control List entries
- `capabilities` (json): Linux capabilities
- `inode` (integer): Inode number
- `device` (integer): Device ID
- `hard_links` (integer): Number of hard links
- `symlink_target` (string): Symbolic link target
- `mount_info` (json): Mount information from /proc/mounts
- `mount_stats` (json): Mount statistics from /proc/mountstats
- `disk_stats` (json): Disk statistics from /proc/diskstats
- `file_locks` (json): File locks from /proc/locks
- `dentry_state` (json): Dentry state from /proc/sys/fs/dentry-state
- `inode_state` (json): Inode state from /proc/sys/fs/inode-state
- `file_nr` (json): File descriptor usage from /proc/sys/fs/file-nr
- `file_max` (integer): Maximum file descriptors from /proc/sys/fs/file-max
- `inode_max` (integer): Maximum inodes from /proc/sys/fs/inode-max
- `inode_nr` (json): Current inode usage from /proc/sys/fs/inode-nr
- `super_max` (integer): Maximum superblocks from /proc/sys/fs/super-max
- `super_nr` (integer): Current superblocks from /proc/sys/fs/super-nr
- `dquot_max` (integer): Maximum disk quotas from /proc/sys/fs/dquot-max
- `dquot_nr` (json): Current disk quotas from /proc/sys/fs/dquot-nr
- `lease_break_time` (integer): Lease break time from /proc/sys/fs/lease-break-time
- `leases_enable` (boolean): Leases enabled from /proc/sys/fs/leases-enable
- `dir_notify_enable` (boolean): Directory notifications from /proc/sys/fs/dir-notify-enable
- `inotify_max_user_watches` (integer): Inotify max user watches
- `inotify_max_user_instances` (integer): Inotify max user instances
- `inotify_max_queued_events` (integer): Inotify max queued events
- `pipe_max_size` (integer): Maximum pipe size from /proc/sys/fs/pipe-max-size
- `pipe_user_pages_hard` (integer): Pipe user pages hard limit
- `pipe_user_pages_soft` (integer): Pipe user pages soft limit
- `protected_hardlinks` (boolean): Protected hardlinks from /proc/sys/fs/protected_hardlinks
- `protected_symlinks` (boolean): Protected symlinks from /proc/sys/fs/protected_symlinks
- `suid_dumpable` (integer): SUID dumpable from /proc/sys/fs/suid_dumpable
- `overflowgid` (integer): Overflow GID from /proc/sys/fs/overflowgid
- `overflowuid` (integer): Overflow UID from /proc/sys/fs/overflowuid
- `nr_open` (integer): Maximum open files from /proc/sys/fs/nr_open
- `mount_max` (integer): Maximum mounts from /proc/sys/fs/mount-max
- `binfmt_misc` (json): Binary format misc from /proc/sys/fs/binfmt_misc
- `epoll` (json): Epoll configuration from /proc/sys/fs/epoll

macOS extensions (provided by `sysinfo`, `notify`, and platform-specific crates):

- `spotlight_metadata` (json): Spotlight metadata (via `notify` crate with FSEvents API)
- `bundle_info` (json): Application bundle information (via `plist` crate for parsing .plist files)
- `quarantine_flags` (json): Quarantine flags (via `xattr` crate for extended attributes)
- `extended_attributes` (json): macOS extended attributes (via `xattr` crate)
- `time_machine_backup` (boolean): Time Machine backup status (via `notify` crate)

Windows extensions (provided by `sysinfo`, `notify`, and platform-specific crates):

- `file_attributes` (json): Windows file attributes (via `notify` crate with ReadDirectoryChangesW API)
- `alternate_data_streams` (json): NTFS alternate data streams (via `notify` crate)
- `security_descriptor` (json): Windows security descriptor (via `notify` crate)
- `volume_serial` (string): Volume serial number (via `notify` crate)
- `file_index` (integer): NTFS file index (via `notify` crate)
- `reparse_point` (boolean): Reparse point status (via `notify` crate)
- `file_archive_events` (json): Archive file events (via `notify` crate)
- `file_compressed_events` (json): Compressed file events (via `notify` crate)
- `file_encrypted_events` (json): Encrypted file events (via `notify` crate)
- `file_hidden_events` (json): Hidden file events (via `notify` crate)
- `file_system_events` (json): System file events (via `notify` crate)
- `file_readonly_events` (json): Read-only file events (via `notify` crate)
- `file_temporary_events` (json): Temporary file events (via `notify` crate)
- `file_offline_events` (json): Offline file events (via `notify` crate)
- `file_sparse_events` (json): Sparse file events (via `notify` crate)
- `file_reparse_events` (json): Reparse point events (via `notify` crate)
- `file_integrity_events` (json): Integrity file events (via `notify` crate)
- `file_virtual_events` (json): Virtual file events (via `notify` crate)
- `file_compression_events` (json): Compression events (via `notify` crate)
- `file_encryption_events` (json): Encryption events (via `notify` crate)
- `file_backup_events` (json): Backup events (via `notify` crate)
- `file_index_events` (json): Index events (via `notify` crate)
- `file_content_indexed_events` (json): Content indexed events (via `notify` crate)
- `file_not_content_indexed_events` (json): Not content indexed events (via `notify` crate)
- `file_recall_on_data_access_events` (json): Recall on data access events (via `notify` crate)
- `file_recall_on_open_events` (json): Recall on open events (via `notify` crate)
- `file_pin_events` (json): Pin events (via `notify` crate)
- `file_unpin_events` (json): Unpin events (via `notify` crate)

##### Performance Collector (perfmond)

Status: Planned

**Virtual Table: `process_performance`**

Core fields available across all platforms (provided by `sysinfo` crate):

- `timestamp` (timestamp): Performance measurement time
- `pid` (integer): Process ID (effective PK)
- `process_name` (string): Process name
- `cpu_usage_percent` (float): Process CPU usage percentage
- `memory_rss` (integer): Resident Set Size in bytes
- `memory_vms` (integer): Virtual Memory Size in bytes
- `memory_usage_percent` (float): Memory usage percentage relative to system
- `thread_count` (integer): Number of threads
- `context_switches` (integer): Context switches
- `disk_read_bytes` (integer): Disk read bytes
- `disk_write_bytes` (integer): Disk write bytes
- `network_read_bytes` (integer): Network read bytes
- `network_write_bytes` (integer): Network write bytes
- `priority` (integer): Process priority
- `nice_value` (integer): Process nice value
- `cpu_time_user` (float): User CPU time in seconds
- `cpu_time_system` (float): System CPU time in seconds
- `cpu_time_total` (float): Total CPU time in seconds
- `memory_peak` (integer): Peak memory usage in bytes
- `memory_shared` (integer): Shared memory usage in bytes
- `memory_private` (integer): Private memory usage in bytes
- `io_read_ops` (integer): I/O read operations count
- `io_write_ops` (integer): I/O write operations count
- `io_read_bytes` (integer): I/O read bytes total
- `io_write_bytes` (integer): I/O write bytes total
- `io_wait_time` (float): I/O wait time in seconds
- `cpu_wait_time` (float): CPU wait time in seconds
- `cpu_idle_time` (float): CPU idle time in seconds
- `cpu_steal_time` (float): CPU steal time in seconds
- `cpu_guest_time` (float): CPU guest time in seconds
- `cpu_guest_nice_time` (float): CPU guest nice time in seconds
- `handle_count` (integer): Handle count (Windows/macOS)
- `page_faults` (integer): Page faults
- `working_set` (integer): Working set size (Windows/macOS)
- `peak_working_set` (integer): Peak working set size (Windows/macOS)
- `gpu_usage` (float): Process GPU usage percentage (macOS/Windows)
- `gpu_memory_used` (integer): Process GPU memory used in bytes (macOS/Windows)
- `energy_impact` (float): Energy impact score (macOS)
- `thermal_state` (string): Thermal state (macOS)
- `cpu_time` (integer): Total CPU time in milliseconds
- `user_time` (integer): User CPU time in milliseconds
- `system_time` (integer): System CPU time in milliseconds
- `memory_usage_percent` (float): Memory usage percentage
- `disk_usage_percent` (float): Disk usage percentage
- `network_usage_percent` (float): Network usage percentage
- `io_wait` (float): I/O wait percentage
- `steal_time` (float): Steal time percentage
- `disk_read_speed` (integer): Disk read speed in bytes/second
- `disk_write_speed` (integer): Disk write speed in bytes/second
- `network_receive_speed` (integer): Network receive speed in bytes/second
- `network_transmit_speed` (integer): Network transmit speed in bytes/second

**Platform-Specific Extensions:**

Linux extensions (provided by `sysinfo`, `procfs`, and platform-specific crates):

- `cgroup_id` (string): Control group ID (via `procfs` crate)
- `container_id` (string): Container identifier (via `procfs` crate)
- `perf_events` (json): Hardware performance counters (via `perf_event_open` syscall)
- `io_wait` (float): Process I/O wait percentage (via `/proc/[pid]/stat`)
- `steal_time` (float): Process steal time percentage (via `/proc/[pid]/stat`)
- `handle_count` (integer): File descriptor count (via `/proc/[pid]/fd`)
- `page_faults` (integer): Page faults (via `/proc/[pid]/stat`)
- `working_set` (integer): Working set size (via `/proc/[pid]/status`)
- `peak_working_set` (integer): Peak working set size (via `/proc/[pid]/status`)
- `cpu_temperature` (float): Process CPU temperature (via `sysfs` thermal zones)
- `power_usage` (float): Process power usage in watts (via `sysfs` power management)
- `energy_impact` (float): Process energy impact score (via `sysfs` energy management)
- `thermal_state` (string): Process thermal state (via `sysfs` thermal management)

macOS extensions (provided by `sysinfo`, `core-graphics`, and platform-specific crates):

- `handle_count` (integer): Process handle count (via `proc_info` syscall)
- `page_faults` (integer): Process page faults (via `proc_info` syscall)
- `working_set` (integer): Process working set size (via `proc_info` syscall)
- `peak_working_set` (integer): Process peak working set size (via `proc_info` syscall)
- `gpu_usage` (float): Process GPU usage percentage (via `core-graphics` or `Metal` API)
- `gpu_memory_used` (integer): Process GPU memory used (via `core-graphics` or `Metal` API)
- `metal_performance` (json): Process Metal performance data (via `Metal` API)
- `battery_level` (float): Battery level percentage (via `IOKit`)
- `battery_status` (string): Battery status (via `IOKit`)

Windows extensions (provided by `sysinfo`, `windows`, and platform-specific crates):

- `handle_count` (integer): Process handle count (via `windows` crate)
- `page_faults` (integer): Process page faults (via `windows` crate)
- `working_set` (integer): Process working set size (via `windows` crate)
- `peak_working_set` (integer): Process peak working set size (via `windows` crate)
- `gpu_usage` (float): Process GPU usage percentage (via `windows` crate with WMI)
- `gpu_memory_used` (integer): Process GPU memory used (via `windows` crate with WMI)
- `battery_level` (float): Battery level percentage (via `windows` crate)
- `battery_status` (string): Battery status (via `windows` crate)

#### Trigger Collectors (Event-Driven Enrichment)

##### Binary Analysis Collector (binmond)

The binary analysis collector will be responsible for examining the associated executable file for the process, and in some cases, the memory image of the process itself. It will collect general header fields, hash values, code signing information, imports and exports, and can accept YARA-like tasks to gather highly tuned data.

Status: Planned

**Virtual Table: `binary_analysis`**

Core fields available across all platforms (provided by `goblin` crate):

- `pid` (integer): Process ID (effective PK)
- `file_path` (string): Path to the analyzed binary
- `file_name` (string): Binary file name
- `file_size` (integer): File size in bytes
- `file_format` (string): Binary format (PE, ELF, Mach-O, etc.)
- `architecture` (string): Target architecture (x86, x64, ARM, etc.)
- `platform` (string): Target platform (Windows, Linux, macOS)
- `entry_point` (string): Entry point address
- `base_address` (string): Base address of binary
- `image_size` (integer): Size of loaded image
- `subsystem` (string): Subsystem type (console, gui, etc.)
- `machine_type` (string): Machine type identifier
- `characteristics` (json): Binary characteristics flags
- `sections` (json): Section headers and information
- `imports_count` (integer): Number of imported functions
- `exports_count` (integer): Number of exported functions
- `libraries_count` (integer): Number of linked libraries
- `compiler` (string): Compiler used to build the binary (via debug symbols)
- `compiler_version` (string): Compiler version (via debug symbols)
- `build_tool` (string): Build tool identification (via debug symbols)
- `build_date` (timestamp): Build date and time (via debug symbols)
- `debug_info` (json): Debug information sections
- `source_files` (json): Source file paths (via debug symbols)
- `panic_messages` (json): Embedded panic messages (Rust)
- `file_paths` (json): Embedded file paths (Rust/Go)
- `build_info` (json): Build information (Go binaries)
- `go_version` (string): Go runtime version (Go binaries)
- `go_modules` (json): Go module information (Go binaries)
- `rust_panic_info` (json): Rust panic metadata (Rust binaries)
- `packed` (boolean): Whether binary is packed/compressed
- `packer_type` (string): Type of packer used (UPX, PECompact, etc.)
- `upx_packed` (boolean): Whether binary is UPX-packed
- `upx_version` (string): UPX version used (if UPX-packed)
- `section_entropy` (json): Entropy values for each section
- `suspicious_sections` (json): Sections with high entropy or unusual names
- `entropy` (float): File entropy score
- `created_time` (timestamp): File creation time
- `modified_time` (timestamp): File modification time
- `sha256_hash` (string): SHA-256 hash of file contents
- `md5_hash` (string): MD5 hash of file contents
- `analysis_time` (timestamp): When analysis was performed

**Virtual Table: `code_signing`**

Core fields available across all platforms (provided by `goblin`, `x509-parser`, and platform-specific crates):

- `pid` (integer): Process ID (effective PK)
- `file_path` (string): Path to the signed binary
- `signed` (boolean): Whether the binary is signed
- `signature_valid` (boolean): Whether the signature is valid
- `signature_type` (string): Signature type (authenticode, adhoc, etc.)
- `certificate_chain` (json): Certificate chain information (via `x509-parser` crate)
- `signing_authority` (string): Certificate authority
- `publisher` (string): Code publisher
- `subject_name` (string): Certificate subject name
- `issuer_name` (string): Certificate issuer name
- `serial_number` (string): Certificate serial number
- `timestamp` (timestamp): Code signing timestamp
- `timestamp_valid` (boolean): Whether timestamp is valid
- `revocation_status` (string): Certificate revocation status
- `trust_level` (string): Trust level assessment
- `signature_algorithm` (string): Signature algorithm used
- `hash_algorithm` (string): Hash algorithm used
- `certificate_valid_from` (timestamp): Certificate valid from date
- `certificate_valid_to` (timestamp): Certificate valid to date

**Virtual Table: `imports_exports`**

Core fields available across all platforms (provided by `goblin` crate):

- `pid` (integer): Process ID (effective PK)
- `file_path` (string): Path to the binary
- `import_type` (string): Import or export
- `library_name` (string): Library name
- `function_name` (string): Function name
- `ordinal` (integer): Function ordinal
- `address` (string): Function address
- `rva` (string): Relative virtual address
- `api_category` (string): API category classification
- `suspicious` (boolean): Suspicious API usage flag
- `forwarded` (boolean): Whether function is forwarded
- `delayed_import` (boolean): Whether import is delayed
- `bound_import` (boolean): Whether import is bound
- `api_set` (string): API set name (Windows)
- `module_name` (string): Module name containing function
- `function_type` (string): Function type (stdcall, cdecl, etc.)
- `calling_convention` (string): Calling convention used
- `parameter_count` (integer): Number of parameters
- `return_type` (string): Return type of function

**Virtual Table: `yara_matches`**

Core fields available across all platforms (provided by `yara` crate):

- `pid` (integer): Process ID (effective PK)
- `file_path` (string): Path to the scanned file
- `rule_name` (string): YARA rule name
- `rule_namespace` (string): YARA rule namespace
- `rule_category` (string): Rule category (malware, packer, etc.)
- `rule_severity` (string): Rule severity level
- `match_count` (integer): Number of matches
- `match_strings` (json): Matched strings
- `rule_metadata` (json): Rule metadata
- `match_offset` (integer): Offset of match in file
- `match_length` (integer): Length of matched content
- `rule_tags` (json): Rule tags
- `rule_author` (string): Rule author
- `rule_version` (string): Rule version
- `rule_description` (string): Rule description
- `scan_time` (timestamp): When scan was performed

**Virtual Table: `debug_info`**

Core fields available across all platforms (provided by `goblin` crate and debug symbol parsing):

- `pid` (integer): Process ID (effective PK)
- `file_path` (string): Path to the binary
- `debug_format` (string): Debug format (DWARF, PDB, etc.)
- `compiler` (string): Compiler used (gcc, clang, msvc, rustc, etc.)
- `compiler_version` (string): Compiler version string
- `build_tool` (string): Build tool (make, cmake, cargo, etc.)
- `build_date` (timestamp): Build date and time
- `build_host` (string): Build host information
- `build_user` (string): Build user information
- `source_files` (json): Source file paths and line numbers
- `function_names` (json): Function names and addresses
- `variable_names` (json): Variable names and types
- `line_numbers` (json): Line number information
- `optimization_level` (string): Compiler optimization level
- `debug_level` (string): Debug information level
- `target_triple` (string): Target architecture triple
- `linker` (string): Linker used
- `linker_version` (string): Linker version
- `analysis_time` (timestamp): When analysis was performed

**Virtual Table: `language_metadata`**

Core fields available across all platforms (provided by `goblin` crate and language-specific parsing):

- `pid` (integer): Process ID (effective PK)
- `file_path` (string): Path to the binary
- `language` (string): Programming language (Rust, Go, C, C++, etc.)
- `language_version` (string): Language version used
- `runtime_version` (string): Runtime version (Go, .NET, etc.)
- `framework` (string): Framework used (Rust std, Go std, etc.)
- `panic_messages` (json): Embedded panic messages (Rust)
- `file_paths` (json): Embedded file paths (Rust/Go)
- `build_info` (json): Build information (Go binaries)
- `go_version` (string): Go runtime version (Go binaries)
- `go_modules` (json): Go module information (Go binaries)
- `rust_panic_info` (json): Rust panic metadata (Rust binaries)
- `rust_std_version` (string): Rust standard library version
- `cargo_version` (string): Cargo version used (Rust)
- `go_build_tags` (json): Go build tags used
- `go_ldflags` (json): Go linker flags used
- `rust_features` (json): Rust features enabled
- `rust_target` (string): Rust target triple
- `analysis_time` (timestamp): When analysis was performed

**Detection Methods:**

- **Go Metadata**: Parse `.note.go.buildid` section for build info, extract version strings from `.rodata` section, analyze Go runtime symbols and string tables using `goblin` crate
- **Rust Metadata**: Scan binary strings for panic messages with file paths, extract Cargo metadata from debug sections, identify Rust std library symbols and panic handlers using `goblin` crate
- **Language Detection**: Analyze import table for language-specific APIs (Go runtime, Rust std), scan string sections for language signatures, examine symbol tables for framework-specific functions using `goblin` crate
- **Metadata Extraction**: Parse section headers for embedded build info, extract version strings from string tables, analyze symbol tables for runtime metadata using `goblin` crate

**Virtual Table: `packer_analysis`**

Core fields available across all platforms (provided by `goblin` crate and entropy analysis):

- `pid` (integer): Process ID (effective PK)
- `file_path` (string): Path to the binary
- `packed` (boolean): Whether binary is packed/compressed
- `packer_type` (string): Type of packer used (UPX, PECompact, ASPack, etc.)
- `packer_version` (string): Version of the packer used
- `upx_packed` (boolean): Whether binary is UPX-packed
- `upx_version` (string): UPX version used (if UPX-packed)
- `upx_compression_ratio` (float): Compression ratio achieved by UPX
- `section_entropy` (json): Entropy values for each section
- `suspicious_sections` (json): Sections with high entropy or unusual names
- `upx_sections` (json): UPX-specific sections (UPX0, UPX1, etc.)
- `import_table_size` (integer): Size of import table (often minimal in packed binaries)
- `entry_point_entropy` (float): Entropy of entry point region
- `overlay_present` (boolean): Whether binary has overlay data
- `overlay_size` (integer): Size of overlay data
- `stub_size` (integer): Size of unpacking stub
- `compression_method` (string): Compression method used
- `compression_level` (integer): Compression level used
- `analysis_time` (timestamp): When analysis was performed

**Detection Methods:**

- **UPX Detection**: Analyze section names (`UPX0`, `UPX1`), section entropy, import table size, and UPX signatures in headers
- **Generic Packer Detection**: High section entropy (>7.0), minimal import table, unusual section names, entry point entropy analysis
- **Entropy Analysis**: Calculate Shannon entropy for each section to identify compressed/encrypted content
- **Section Analysis**: Inspect section headers for packer-specific characteristics using `goblin` crate

##### Memory Analysis Collector (memmond)

Status: Planned

**Virtual Table: `memory_regions`**

Core fields available across all platforms (provided by `sysinfo` crate and platform-specific extensions):

- `pid` (integer): Process ID
- `process_name` (string): Process name
- `region_address` (string): Memory region start address
- `region_size` (integer): Memory region size in bytes
- `region_type` (string): Region type (heap, stack, code, data, mapped)
- `permissions` (string): Memory permissions (read, write, execute)
- `protection` (string): Memory protection flags
- `mapped_file` (string): Mapped file path (if applicable)
- `allocation_time` (timestamp): When region was allocated
- `access_count` (integer): Number of accesses to region
- `dirty_pages` (integer): Number of dirty pages
- `shared` (boolean): Whether region is shared
- `commit_charge` (integer): Committed memory in region
- `working_set` (integer): Working set size for region
- `analysis_time` (timestamp): When analysis was performed

**Virtual Table: `memory_artifacts`**

Core fields available across all platforms (provided by `regex` crate and memory scanning utilities):

- `pid` (integer): Process ID
- `process_name` (string): Process name
- `artifact_type` (string): Artifact type (credential, key, connection, file, malware_signature, etc.)
- `artifact_value` (string): Artifact value
- `artifact_category` (string): Artifact category (suspicious, malicious, credential, network, file)
- `memory_address` (string): Memory address where artifact was found
- `confidence` (float): Detection confidence score
- `context` (json): Additional context information
- `pattern_used` (string): Regex pattern used for detection
- `match_length` (integer): Length of matched artifact
- `encoding` (string): Text encoding detected (UTF-8, ASCII, etc.)
- `extraction_time` (timestamp): When artifact was extracted

**Virtual Table: `memory_patterns`**

> **⚠️ LEGAL/COMPLIANCE WARNING**: Memory analysis involves accessing potentially sensitive data including PII, credentials, and proprietary information. Ensure compliance with CFAA, GDPR/CCPA, PCI-DSS, and obtain proper authorization/consent before deployment. Implement redaction/sanitization rules for extracted artifacts and maintain audit logs for all access.

Core fields available across all platforms (provided by `regex` crate and pattern matching utilities):

- `pid` (integer): Process ID
- `process_name` (string): Process name
- `pattern_type` (string): Pattern type (crypto_key, url, email, ip, malware_signature, shellcode, etc.)
- `pattern_value` (string): Pattern value
- `pattern_regex` (string): Regular expression used
- `match_count` (integer): Number of matches found
- `memory_addresses` (json): Memory addresses where pattern was found
- `confidence` (float): Pattern matching confidence
- `entropy_score` (float): Entropy of matched content
- `false_positive_rate` (float): Estimated false positive rate
- `rop_chain_detected` (boolean): Summary flag for ROP chain detection (detailed analysis in exploit_detection table)
- `buffer_overflow_detected` (boolean): Summary flag for buffer overflow detection (detailed analysis in exploit_detection table)
- `stack_corruption_detected` (boolean): Summary flag for stack corruption detection (detailed analysis in exploit_detection table)
- `scan_time` (timestamp): When pattern scan was performed

**Privacy & Compliance Controls:**

- **Redaction Rules**: Mask PII, credentials, keys using regex patterns; replace with hashed tokens
- **Retention Policy**: Memory artifacts retained for 7 days with secure deletion; audit logs for 90 days
- **Access Controls**: Memory analysis requires elevated permissions and multi-factor authentication
- **Scope Restrictions**: Default scanning excludes heap/user-data regions; limit to code/stack/mapped regions
- **Operational Checklist**: Obtain explicit consent, document legal basis, implement audit logging, establish data handling procedures

**Table Boundaries:**

- `memory_patterns`: Generic pattern-based detections (crypto keys, URLs, emails, IPs)
- `exploit_detection`: Deep exploit-specific analysis (ROP chains, buffer overflows, shellcode)

**Virtual Table: `heap_analysis`**

Core fields available across all platforms (provided by `sysinfo` crate and platform-specific memory analysis):

- `pid` (integer): Process ID
- `process_name` (string): Process name
- `heap_address` (string): Heap base address
- `heap_size` (integer): Heap size in bytes
- `allocation_count` (integer): Number of allocations
- `free_count` (integer): Number of frees
- `memory_leak_count` (integer): Number of potential memory leaks detected
- `fragmentation` (float): Heap fragmentation percentage; fragmentation is allocator- and workload-dependent and not by itself indicative of malicious behavior
- `largest_allocation` (integer): Size of largest allocation
- `average_allocation` (float): Average allocation size
- `peak_memory_usage` (integer): Peak memory usage
- `current_memory_usage` (integer): Current memory usage
- `memory_growth_rate` (float): Memory growth rate over time
- `analysis_time` (timestamp): When heap analysis was performed

**Virtual Table: `exploit_detection`**

Core fields available across all platforms (provided by `regex` crate, `sysinfo` crate, and custom exploit analysis logic):

- `pid` (integer): Process ID
- `process_name` (string): Process name
- `exploit_type` (string): Type of exploit detected (ROP, buffer_overflow, stack_overflow, heap_overflow, format_string, etc.)
- `exploit_confidence` (float): Confidence score for exploit detection
- `rop_chain_length` (integer): Length of detected ROP chain
- `rop_gadgets` (json): ROP gadgets found in the chain
- `buffer_size` (integer): Size of overflowed buffer
- `overflow_offset` (integer): Offset where overflow occurred
- `stack_pointer` (string): Stack pointer value at time of detection
- `return_address` (string): Return address value
- `canary_detected` (boolean): Whether stack canary was present
- `canary_value` (string): Stack canary value
- `aslr_enabled` (boolean): Whether ASLR was enabled
- `dep_enabled` (boolean): Whether DEP/NX was enabled
- `exploit_technique` (string): Specific technique used (ret2libc, ret2syscall, etc.)
- `shellcode_detected` (boolean): Whether shellcode was detected
- `shellcode_size` (integer): Size of detected shellcode
- `analysis_time` (timestamp): When exploit analysis was performed

**Detection Methods:**

- **Memory Region Analysis**: Use `sysinfo` crate for process memory information, `procfs` crate for parsing `/proc/[pid]/maps` (Linux), `mach2` crate for macOS memory regions, and `windows` crate for `VirtualQueryEx` (Windows) region details
- **Pattern Matching**: Use `regex` crate for pattern matching in memory dumps, scan for URLs, emails, IPs, crypto keys, and other artifacts using compiled regex patterns
- **Artifact Extraction**: Scan memory regions for strings, extract text using encoding detection, identify structured data patterns using entropy analysis
- **Heap Analysis**: Use `sysinfo` crate for process memory statistics, analyze allocation patterns, detect memory leaks through allocation/free tracking, calculate fragmentation metrics
- **Security Analysis**: Instrument memory activity via first-party telemetry built on `sysinfo`, `procfs`, `mach2`, and the `windows` crate (ToolHelp + ETW providers). Develop and ship an in-house `heap_sentinel` module for sustained allocation/leak detection so no runtime profile depends on experimental crates; keep optional debug profiling behind cargo features.
- **Threat Detection**: Combine maintained tooling—`capstone` for disassembly, `goblin` for PE/ELF parsing, and OS-backed memory snapshots—with custom heuristics that score injection indicators. Document fallback logic and regression tests for each heuristic so we can evolve without reintroducing unstable dependencies.
- **Exploit Detection**: Use `regex` crate for pattern matching to detect ROP gadgets, shellcode signatures, and exploit patterns in memory dumps
- **ROP Analysis**: Scan memory for ROP gadget patterns using regex, analyze return address chains for suspicious patterns, detect gadget sequences
- **Buffer Overflow Detection**: Use `sysinfo` and platform-specific crates to analyze stack frames, detect canary violations, identify return address corruption patterns
- **Shellcode Detection**: Use `regex` crate to scan for shellcode signatures, detect encoded payloads, identify suspicious code patterns

Mitigation note: any auxiliary instrumentation that requires vendor or experimental APIs will ship behind an `experimental_telemetry` feature flag. The production build matrix forbids enabling that flag, and security review tickets track removal timelines whenever we prototype with non-hardened crates.

##### Network Analysis Collector (netanalymond)

Status: Planned

**Virtual Table: `network_traffic`**

Core fields available across all platforms (provided by `sysinfo` crate, `netstat` crate, and `huginn-net` crate):

- `connection_id` (string): Unique connection identifier
- `protocol` (string): Protocol (tcp, udp, http, https, dns, etc.)
- `source_ip` (string): Source IP address
- `source_port` (integer): Source port number
- `destination_ip` (string): Destination IP address
- `destination_port` (integer): Destination port number
- `packet_count` (integer): Number of packets
- `bytes_transferred` (integer): Total bytes transferred
- `start_time` (timestamp): Connection start time
- `end_time` (timestamp): Connection end time
- `duration` (integer): Connection duration in seconds
- `direction` (string): Traffic direction (inbound, outbound)
- `pid` (integer): Process ID associated with connection
- `process_name` (string): Process name associated with connection
- `connection_state` (string): Connection state (established, listening, etc.)
- `bytes_sent` (integer): Bytes sent by process
- `bytes_received` (integer): Bytes received by process
- `tcp_ttl` (integer): TCP TTL value (via huginn-net)
- `tcp_window_size` (integer): TCP window size (via huginn-net)
- `tcp_mtu` (integer): TCP MTU value (via huginn-net)
- `analysis_time` (timestamp): When analysis was performed

**Virtual Table: `protocol_analysis`**

Core fields available across all platforms (provided by `huginn-net` crate for protocol fingerprinting and analysis):

- `connection_id` (string): Connection identifier
- `protocol` (string): Protocol name
- `version` (string): Protocol version
- `method` (string): HTTP method (for HTTP/HTTPS)
- `url` (string): Requested URL (for HTTP/HTTPS)
- `status_code` (integer): HTTP status code
- `user_agent` (string): User agent string
- `content_type` (string): Content type
- `content_length` (integer): Content length
- `server` (string): Server header
- `dns_query` (string): DNS query name
- `dns_response` (string): DNS response
- `ssl_version` (string): SSL/TLS version
- `cipher_suite` (string): Cipher suite used
- `certificate_issuer` (string): Certificate issuer
- `http_headers` (json): HTTP headers as key-value pairs
- `dns_record_type` (string): DNS record type (A, AAAA, CNAME, etc.)
- `dns_response_code` (integer): DNS response code
- `pid` (integer): Process ID associated with connection
- `process_name` (string): Process name associated with connection
- `tls_handshake_complete` (boolean): Whether TLS handshake completed
- `tls_certificate_valid` (boolean): Whether TLS certificate is valid
- `ja4_fingerprint` (string): JA4 TLS client fingerprint (via huginn-net)
- `http_user_agent` (string): HTTP User-Agent string (via huginn-net)
- `http_accept_language` (string): HTTP Accept-Language header (via huginn-net)
- `tcp_signature` (string): TCP signature fingerprint (via huginn-net)
- `analysis_time` (timestamp): When analysis was performed

**Privacy & Compliance Controls:**

**PII and Sensitive Data Handling:**

- **URL/DNS PII Redaction**: Strip or canonicalize query parameters, remove path segments resembling usernames/IDs (e.g., `/user/12345/profile` → `/user/[REDACTED]/profile`)
- **Token/Credential Detection**: Mask Authorization, Cookie, API key values in `http_headers` using regex patterns (e.g., `(?i)(authorization|cookie|x-api-key):\s*[^\s,]+` → `$1: [MASKED]`)
- **Data Retention Policy**: Protocol analysis records retained for 30 days with automated purge; audit logs for 1 year
- **Access Controls**: Protocol analysis requires elevated permissions and audit logging for all access
- **Legal/Operational Notes**: HTTP/HTTPS inspection requires TLS interception capabilities, explicit consent/notice, and jurisdiction-specific compliance (check local laws for network monitoring requirements)

**Affected Fields**: `url`, `dns_query`, `http_headers`, `user_agent`, `certificate_issuer`

**Virtual Table: `traffic_patterns`**

Core fields available across all platforms (provided by `sysinfo` crate for basic network statistics and `pnet` crate for detailed traffic analysis):

- `pattern_id` (string): Unique pattern identifier
- `interface_name` (string): Network interface name (via sysinfo)
- `packets_received` (integer): Packets received since last refresh (via sysinfo)
- `packets_transmitted` (integer): Packets transmitted since last refresh (via sysinfo)
- `total_packets_received` (integer): Total packets received (via sysinfo)
- `total_packets_transmitted` (integer): Total packets transmitted (via sysinfo)
- `errors_received` (integer): Errors on received packets (via sysinfo)
- `errors_transmitted` (integer): Errors on transmitted packets (via sysinfo)
- `mac_address` (string): MAC address of interface (via sysinfo)
- `source_ip` (string): Source IP address (via pnet packet analysis)
- `destination_ip` (string): Destination IP address (via pnet packet analysis)
- `source_port` (integer): Source port (via pnet packet analysis)
- `destination_port` (integer): Destination port (via pnet packet analysis)
- `protocol` (string): Protocol (tcp, udp, icmp, etc.) (via pnet packet analysis)
- `packet_size` (integer): Packet size in bytes (via pnet packet analysis)
- `timestamp` (timestamp): Packet timestamp (via pnet packet analysis)
- `pid` (integer): Process ID associated with connection (via sysinfo + netstat)
- `process_name` (string): Process name associated with connection (via sysinfo + netstat)
- `analysis_time` (timestamp): When analysis was performed

**Detection Methods:**

- **Network Interface Monitoring**: Use `sysinfo` crate to collect basic network interface statistics (packets received/transmitted, errors, MAC addresses) and track process network connections
- **Packet Analysis**: Use `pnet` crate for detailed packet inspection, protocol identification, and traffic analysis at the packet level
- **Process Association**: Use `netstat` crate to associate network connections with specific processes and track connection states
- **Protocol Fingerprinting**: Use `huginn-net` crate for multi-protocol passive fingerprinting, JA4 TLS client fingerprinting, TCP signature analysis, and HTTP fingerprinting

##### Registry/Configuration Collector (regmond)

Status: Planned

**Virtual Table: `system_configuration`**

Core fields available across all platforms (provided by `sysinfo` crate and configuration parsing crates):

- `pid` (integer): Process ID associated with configuration (effective PK)
- `config_path` (string): Configuration file or registry path
- `config_type` (string): Configuration type (file, registry, plist, ini, toml, yaml)
- `setting_name` (string): Configuration setting name
- `setting_value` (string): Configuration setting value
- `setting_type` (string): Value type (string, integer, boolean, json)
- `category` (string): Configuration category (security, network, system, etc.)
- `modified_time` (timestamp): When setting was last modified
- `file_size` (integer): Configuration file size in bytes
- `file_permissions` (string): File permissions (Unix) or security descriptor (Windows)
- `relevance_reason` (string): Why this config is relevant to the process (process_uses, system_security, recent_change)
- `analysis_time` (timestamp): When analysis was performed

**Virtual Table: `configuration_changes`**

Core fields available across all platforms (provided by file system monitoring and configuration parsing):

- `change_id` (string): Unique change identifier
- `pid` (integer): Process ID that triggered the change
- `config_path` (string): Configuration path
- `setting_name` (string): Setting name
- `old_value` (string): Previous value
- `new_value` (string): New value
- `change_type` (string): Change type (create, modify, delete)
- `change_time` (timestamp): When change occurred
- `file_event_type` (string): File system event type (via `notify` crate)
- `process_name` (string): Process name that made the change
- `user_id` (integer): User ID that made the change

**Platform-Specific Extensions:**

Windows extensions (provided by `winreg` crate and `windows` crate):

- `registry_hive` (string): Registry hive (HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER, etc.)
- `registry_key` (string): Registry key path
- `registry_value` (string): Registry value name
- `registry_type` (string): Registry value type (REG_SZ, REG_DWORD, etc.)
- `group_policy` (boolean): Whether setting is from Group Policy
- `security_descriptor` (json): Security descriptor information
- `registry_timestamp` (timestamp): Registry modification timestamp

macOS extensions (provided by `plist` crate and `xattr` crate):

- `plist_domain` (string): Property list domain (user, system, etc.)
- `launchd_service` (boolean): Whether setting is for launchd service
- `sandbox_entitlement` (boolean): Whether setting is sandbox entitlement
- `privacy_setting` (boolean): Whether setting is privacy-related
- `system_integrity` (boolean): Whether setting affects system integrity
- `extended_attributes` (json): Extended attributes (via `xattr` crate)
- `quarantine_flags` (json): Quarantine flags (via `xattr` crate)

Linux extensions (provided by `procfs` crate and configuration parsing):

- `config_file` (string): Configuration file path
- `systemd_unit` (string): systemd unit name
- `selinux_context` (string): SELinux security context
- `apparmor_profile` (string): AppArmor profile name
- `kernel_parameter` (boolean): Whether setting is kernel parameter
- `file_owner` (string): Configuration file owner
- `file_group` (string): Configuration file group
- `inode` (integer): File inode number

**Detection Methods:**

- **Process-Specific Config Discovery**: Use `sysinfo` crate to get process working directory, environment variables, and executable path to identify relevant configuration files (e.g., config files in working directory, environment-specified config paths)
- **Process-Specific System Configs**: Examine system configurations that directly affect the process (user permissions, resource limits, security contexts, firewall rules for process's network connections) using platform-specific crates
- **Recent Configuration Changes**: Use `notify` crate to identify configuration files that have been modified recently and associate them with the suspicious process
- **Configuration File Parsing**: Use `ini` crate for INI files, `toml` crate for TOML files, `serde_yaml` crate for YAML files, `plist` crate for macOS property lists, `winreg` crate for Windows registry access
- **Process Association**: Use `sysinfo` crate to associate configuration changes with specific processes and track modification sources
