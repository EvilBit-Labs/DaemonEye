# Product Strategy

## Overview

Procmond is a system monitoring tool that collects process information from the system and sends it to a central server. It is designed to be used in a distributed environment, where each server collects process information from the systems it is running on and sends it to a central server. The central server then aggregates the process information and sends it to a central analysis server.

## Core Functionality

Setting aside the high-level mission statement, the core functionality of the product is to allow administrators to run queries against the running system to detect suspicious activity. This is done by using a SQL-like query language that is translated into a set of detection tasks for "monitoring" collectors that run on the system and watch processes, files, network connections, and other system resources. When these tasks detect the portion of the "query" they are responsible for, they will trigger additional enrichment by "triggerable" collectors that will run in parallel and provide additional context for the detection. Combining these multiple collectors to return results in the form of a virtual "table" that can be queried against using the same SQL-like language is the core of the system functionality.

The DaemonEye-Agent is responsible for taking the "SQL queries" and turning them into "detection tasks" that are then sent to the appropriate collectors, and then collecting the results and maintaining the virtual database structure. The database structure is entirely logical, as it is actually stored in a high-performance key-value store and presented to by the agent as a virtual database.

## Prioritization

Given the core functionality, the most important features are:

- A robust SQL-like query language that is easy to learn and use and allows for complex queries with as much cross-platform consistency as possible
- A set of "monitoring" collectors that are responsible for watching the system and detecting suspicious activity, collecting as much as possible while remaining as small and efficient as possible
- A set of "triggerable" collectors that are responsible for providing additional context for the detection, gathering as much as possible while being consistent and easy to write queries against
- A set of "alerting" sinks that are responsible for delivering the results to the administrator

From a more practical standpoint, that means that the only real way to truly enhance the project core is to continue to standardize procmond, the collector-core framework, and the agent. While we can always add more sinks, more central monitoring and management, and easier deployment, those features are generally not core to the project's success and are geared more for the business and enterprise customers. The emphesis for this repository is define an extensive and robust collector-core framework that can be used to build a wide variety of collectors and agents, and then to build a wide variety of agents that can be used to detect suspicious activity on the system. While each platform does offer an open metadata value, which can contain platform-specific data, the goal is to find the best features for each platform and try to offer them in a consistent way across all platforms. We will be calling these "collector profiles" and they will be defined similar to database schemas.

### Collector Types

- `monitor`: These collectors are responsible for monitoring the processes through either subscription or polling. They will be responsible for collecting the most basic metadata about the process and then triggering the triggerable collectors to collect more detailed metadata. They will run as daemons, with their lifecycle managed by the DaemonEye-Agent, and will typically run for the entire time that the DaemonEye-Agent is running. They will only poll if they have alert tasks or if they do not have the ability to subscribe to the process events.
- `triggerable`: These collectors are responsible for collecting more detailed metadata about the process. They do not poll or subscribe to events and will only examine data when provided a task from the DaemonEye-Agent from a monitor collector. They will populate their own virtual tables in the agent's virtual database on demand. They will run as daemons, with their lifecycle managed by the DaemonEye-Agent, and will remain idle awaiting a task provided over their IPC channel. Their tasking is generally setup using the special DaemonEye-dialect SQL syntax `AUTO JOIN` to automatically collect the data they need (see `daemon_eye_spec_sql_to_ipc_detection_architecture.md` for more details).

### Collector Roadmap

The collector framework exposes virtual tables that can be queried using SQL-like syntax. Each collector provides specific data schemas that represent system resources and analysis results. The "primary key" for records is going to be a `pid` column, representing each process on the system. Since pids can be reused, the true primary key will be a composite of `pid`, `timestamp`, and a value that uniquely identifies the process from other possible processes that might reuse that same pid.

Triggerable collectors are not allowed to trigger other triggerable collectors. The DaemonEye-Agent will be responsible for triggering the appropriate collectors based on the SQL query and, when all data is returned, the DaemonEye-Agent will then trigger the alerting sinks to deliver the results. The alerting sinks will be responsible for delivering the results to the administrator. This decoupling allows the individual components to be more focused and have a reduced attack surface.

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
- `connection_id` (string): Unique connection identifier
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
- `event_type` (string): Event type (create, modify, delete, access, move)
- `file_path` (string): Full path to the file (effective FK to `file_metadata.file_path` on Windows)
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
- `inode` (integer): Inode number (Linux/Unix) (effective FK to `file_metadata.inode` on Unix systems)
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

**Virtual Table: `file_metadata`**

Core fields available across all platforms (provided by `sysinfo` and platform-specific crates):

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
- `inode` (integer): Inode number (Linux/Unix) (effective PK on Unix systems and FK to `file_metadata.inode` on Unix systems)
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
- `file_archive_events` (json): Archive events (via `notify` crate)
- `file_index_events` (json): Index events (via `notify` crate)
- `file_content_indexed_events` (json): Content indexed events (via `notify` crate)
- `file_not_content_indexed_events` (json): Not content indexed events (via `notify` crate)
- `file_offline_events` (json): Offline events (via `notify` crate)
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

Core fields available across all platforms (provided by `goblin`)

- `file_path` (string): Path to the analyzed binary
- `file_name` (string): Binary file name
- `file_size` (integer): File size in bytes
- `file_format` (string): Binary format (PE, ELF, Mach-O, etc.)
- `architecture` (string): Target architecture (x86, x64, ARM, etc.)
- `platform` (string): Target platform (Windows, Linux, macOS)
- `compiler` (string): Compiler used to build the binary
- `build_tool` (string): Build tool identification
- `entropy` (float): File entropy score
- `complexity` (string): Complexity assessment (low, medium, high)
- `created_time` (timestamp): File creation time
- `modified_time` (timestamp): File modification time
- `sha256_hash` (string): SHA-256 hash of file contents
- `md5_hash` (string): MD5 hash of file contents
- `analysis_time` (timestamp): When analysis was performed

**Virtual Table: `code_signing`**

Core fields available across all platforms:

- `file_path` (string): Path to the signed binary
- `signed` (boolean): Whether the binary is signed
- `signature_valid` (boolean): Whether the signature is valid
- `certificate_chain` (json): Certificate chain information
- `signing_authority` (string): Certificate authority
- `publisher` (string): Code publisher
- `timestamp` (timestamp): Code signing timestamp
- `revocation_status` (string): Certificate revocation status
- `trust_level` (string): Trust level assessment

**Virtual Table: `imports_exports`**

Core fields available across all platforms:

- `file_path` (string): Path to the binary
- `import_type` (string): Import or export
- `library_name` (string): Library name
- `function_name` (string): Function name
- `ordinal` (integer): Function ordinal
- `address` (string): Function address
- `api_category` (string): API category classification
- `suspicious` (boolean): Suspicious API usage flag

**Virtual Table: `yara_matches`**

Core fields available across all platforms:

- `file_path` (string): Path to the scanned file
- `rule_name` (string): YARA rule name
- `rule_namespace` (string): YARA rule namespace
- `rule_category` (string): Rule category (malware, packer, etc.)
- `rule_severity` (string): Rule severity level
- `match_count` (integer): Number of matches
- `match_strings` (json): Matched strings
- `rule_metadata` (json): Rule metadata
- `scan_time` (timestamp): When scan was performed

##### Memory Analysis Collector (memmond)

Status: Planned

**Virtual Table: `memory_regions`**

Core fields available across all platforms:

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
- `analysis_time` (timestamp): When analysis was performed

**Virtual Table: `memory_artifacts`**

Core fields available across all platforms:

- `pid` (integer): Process ID
- `process_name` (string): Process name
- `artifact_type` (string): Artifact type (string, key, connection, file, etc.)
- `artifact_value` (string): Artifact value
- `artifact_category` (string): Artifact category
- `memory_address` (string): Memory address where artifact was found
- `confidence` (float): Detection confidence score
- `context` (json): Additional context information
- `extraction_time` (timestamp): When artifact was extracted

**Virtual Table: `memory_patterns`**

Core fields available across all platforms:

- `pid` (integer): Process ID
- `process_name` (string): Process name
- `pattern_type` (string): Pattern type (crypto_key, url, email, ip, etc.)
- `pattern_value` (string): Pattern value
- `pattern_regex` (string): Regular expression used
- `match_count` (integer): Number of matches found
- `memory_addresses` (json): Memory addresses where pattern was found
- `confidence` (float): Pattern matching confidence
- `scan_time` (timestamp): When pattern scan was performed

**Virtual Table: `heap_analysis`**

Core fields available across all platforms:

- `pid` (integer): Process ID
- `process_name` (string): Process name
- `heap_address` (string): Heap base address
- `heap_size` (integer): Heap size in bytes
- `allocation_count` (integer): Number of allocations
- `free_count` (integer): Number of frees
- `leak_count` (integer): Number of memory leaks
- `fragmentation` (float): Heap fragmentation percentage
- `largest_allocation` (integer): Size of largest allocation
- `average_allocation` (float): Average allocation size
- `analysis_time` (timestamp): When heap analysis was performed

##### Network Analysis Collector (netanalymond)

Status: Planned

**Virtual Table: `network_traffic`**

Core fields available across all platforms:

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
- `analysis_time` (timestamp): When analysis was performed

**Virtual Table: `protocol_analysis`**

Core fields available across all platforms:

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

**Virtual Table: `threat_indicators`**

Core fields available across all platforms:

- `indicator_id` (string): Unique indicator identifier
- `indicator_type` (string): Indicator type (ip, domain, url, hash, etc.)
- `indicator_value` (string): Indicator value
- `threat_category` (string): Threat category (malware, phishing, botnet, etc.)
- `severity` (string): Threat severity (low, medium, high, critical)
- `confidence` (float): Detection confidence score
- `source` (string): Threat intelligence source
- `first_seen` (timestamp): When indicator was first seen
- `last_seen` (timestamp): When indicator was last seen
- `description` (string): Threat description
- `tags` (json): Additional tags and metadata

**Virtual Table: `traffic_patterns`**

Core fields available across all platforms:

- `pattern_id` (string): Unique pattern identifier
- `pattern_type` (string): Pattern type (bandwidth, frequency, geographic, etc.)
- `source_ip` (string): Source IP address
- `destination_ip` (string): Destination IP address
- `protocol` (string): Protocol
- `pattern_value` (float): Pattern value
- `baseline_value` (float): Baseline value for comparison
- `anomaly_score` (float): Anomaly score
- `time_window` (integer): Time window in seconds
- `occurrence_count` (integer): Number of occurrences
- `first_occurrence` (timestamp): First occurrence time
- `last_occurrence` (timestamp): Last occurrence time

##### Registry/Configuration Collector (regmond)

Status: Planned

**Virtual Table: `system_configuration`**

Core fields available across all platforms:

- `config_path` (string): Configuration file or registry path
- `config_type` (string): Configuration type (file, registry, plist, etc.)
- `setting_name` (string): Configuration setting name
- `setting_value` (string): Configuration setting value
- `setting_type` (string): Value type (string, integer, boolean, json)
- `category` (string): Configuration category (security, network, system, etc.)
- `modified_time` (timestamp): When setting was last modified
- `modified_by` (string): User or process that modified setting
- `backup_available` (boolean): Whether backup is available
- `compliance_status` (string): Compliance status (compliant, non-compliant, unknown)
- `analysis_time` (timestamp): When analysis was performed

**Virtual Table: `configuration_changes`**

Core fields available across all platforms:

- `change_id` (string): Unique change identifier
- `config_path` (string): Configuration path
- `setting_name` (string): Setting name
- `old_value` (string): Previous value
- `new_value` (string): New value
- `change_type` (string): Change type (create, modify, delete)
- `change_time` (timestamp): When change occurred
- `changed_by` (string): User or process that made change
- `change_reason` (string): Reason for change
- `impact_level` (string): Impact level (low, medium, high, critical)
- `rollback_available` (boolean): Whether rollback is available

**Virtual Table: `security_policies`**

Core fields available across all platforms:

- `policy_id` (string): Unique policy identifier
- `policy_name` (string): Policy name
- `policy_category` (string): Policy category (authentication, network, file, etc.)
- `policy_value` (string): Policy value
- `enforcement_level` (string): Enforcement level (advisory, mandatory, etc.)
- `compliance_status` (string): Compliance status
- `last_checked` (timestamp): When policy was last checked
- `violation_count` (integer): Number of violations
- `recommendation` (string): Security recommendation
- `platform_specific` (boolean): Whether policy is platform-specific

**Platform-Specific Extensions:**

Windows extensions:

- `registry_hive` (string): Registry hive (HKEY_LOCAL_MACHINE, etc.)
- `registry_key` (string): Registry key path
- `registry_value` (string): Registry value name
- `registry_type` (string): Registry value type
- `group_policy` (boolean): Whether setting is from Group Policy
- `security_descriptor` (json): Security descriptor

macOS extensions:

- `plist_domain` (string): Property list domain
- `launchd_service` (boolean): Whether setting is for launchd service
- `sandbox_entitlement` (boolean): Whether setting is sandbox entitlement
- `privacy_setting` (boolean): Whether setting is privacy-related
- `system_integrity` (boolean): Whether setting affects system integrity

Linux extensions:

- `config_file` (string): Configuration file path
- `systemd_unit` (string): systemd unit name
- `selinux_context` (string): SELinux security context
- `apparmor_profile` (string): AppArmor profile name
- `kernel_parameter` (boolean): Whether setting is kernel parameter
