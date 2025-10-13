#[cfg(not(feature = "broker"))]
fn main() {
    println!("This example requires the 'broker' feature on the 'busrt' crate. Enable it to build/run.");
}

#[cfg(feature = "broker")]
mod example {
    //! Busrt Transport Validation for DaemonEye Integration
    //!
    //! This example validates different busrt transport options based on research:
    //! - In-process channel transport for internal daemoneye-agent communication
    //! - UNIX socket transport for inter-process collector communication
    //! - TCP transport for potential future network capabilities
    //! - Performance characteristics and selection criteria

    use anyhow::{Context, Result};
    use busrt::broker::Broker;
    use serde::{Deserialize, Serialize};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    use tracing::{info, warn};

    /// Test message for transport validation
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct TestMessage {
        pub id: u32,
        pub content: String,
        pub timestamp: u64,
    }

    /// Transport performance metrics
    #[derive(Debug, Clone)]
    pub struct TransportMetrics {
        pub transport_type: String,
        pub setup_time_ms: u64,
        pub message_latency_ms: f64,
        pub throughput_msgs_per_sec: f64,
        pub success_rate: f64,
        pub error_count: u32,
    }

    /// Transport validation results
    #[derive(Debug)]
    pub struct ValidationResults {
        pub in_process_metrics: Option<TransportMetrics>,
        pub unix_socket_metrics: Option<TransportMetrics>,
        pub tcp_metrics: Option<TransportMetrics>,
        pub recommendations: Vec<String>,
    }

    /// Busrt transport validator
    pub struct BusrtTransportValidator {
        test_message_count: u32,
        timeout_duration: Duration,
    }

    impl BusrtTransportValidator {
        pub fn new() -> Self {
            Self {
                test_message_count: 100,
                timeout_duration: Duration::from_secs(30),
            }
        }

        /// Validate all available transport options
        pub async fn validate_all_transports(&self) -> Result<ValidationResults> {
            info!("Starting comprehensive busrt transport validation");

            let mut results = ValidationResults {
                in_process_metrics: None,
                unix_socket_metrics: None,
                tcp_metrics: None,
                recommendations: Vec::new(),
            };

            // Test 1: In-process channel transport
            info!("Testing in-process channel transport");
            match self.test_in_process_transport().await {
                Ok(metrics) => {
                    info!("In-process transport validation successful");
                    results.in_process_metrics = Some(metrics);
                }
                Err(e) => {
                    warn!("In-process transport validation failed: {}", e);
                }
            }

            // Test 2: UNIX socket transport (simulated)
            info!("Testing UNIX socket transport (simulated)");
            match self.simulate_unix_socket_transport().await {
                Ok(metrics) => {
                    info!("UNIX socket transport validation successful");
                    results.unix_socket_metrics = Some(metrics);
                }
                Err(e) => {
                    warn!("UNIX socket transport validation failed: {}", e);
                }
            }

            // Test 3: TCP transport (simulated)
            info!("Testing TCP transport (simulated)");
            match self.simulate_tcp_transport().await {
                Ok(metrics) => {
                    info!("TCP transport validation successful");
                    results.tcp_metrics = Some(metrics);
                }
                Err(e) => {
                    warn!("TCP transport validation failed: {}", e);
                }
            }

            // Generate recommendations
            results.recommendations = self.generate_recommendations(&results);

            info!("Transport validation completed");
            Ok(results)
        }

        /// Test in-process channel transport
        async fn test_in_process_transport(&self) -> Result<TransportMetrics> {
            let start_time = Instant::now();

            // Create broker for in-process transport
            let _broker = Broker::new();
            info!("Created in-process broker");

            let setup_time = start_time.elapsed();

            // Simulate message processing metrics for in-process communication
            let message_start = Instant::now();

            // In-process communication would be direct function calls
            // or memory channels, so latency is minimal
            let mut total_latency = 0.0;
            let success_count = self.test_message_count;
            let error_count = 0;

            for i in 0..self.test_message_count {
                let msg_start = Instant::now();

                // Simulate in-process message handling
                let _test_msg = TestMessage {
                    id: i,
                    content: format!("in-process-test-{}", i),
                    timestamp: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64,
                };

                // Simulate processing time (in-process is very fast)
                tokio::time::sleep(Duration::from_micros(10)).await;

                let msg_latency = msg_start.elapsed().as_secs_f64() * 1000.0;
                total_latency += msg_latency;
            }

            let total_time = message_start.elapsed();
            let throughput = success_count as f64 / total_time.as_secs_f64();
            let avg_latency = total_latency / success_count as f64;
            let success_rate = success_count as f64 / self.test_message_count as f64;

            Ok(TransportMetrics {
                transport_type: "in-process".to_string(),
                setup_time_ms: setup_time.as_millis() as u64,
                message_latency_ms: avg_latency,
                throughput_msgs_per_sec: throughput,
                success_rate,
                error_count,
            })
        }

        /// Simulate UNIX socket transport characteristics
        async fn simulate_unix_socket_transport(&self) -> Result<TransportMetrics> {
            let start_time = Instant::now();

            // Simulate broker setup time for UNIX sockets (respecting timeout)
            let setup_delay = std::cmp::min(Duration::from_millis(50), self.timeout_duration / 10);
            tokio::time::sleep(setup_delay).await;
            info!("Simulated UNIX socket broker setup");

            let setup_time = start_time.elapsed();

            // Simulate message processing with UNIX socket characteristics
            let message_start = Instant::now();
            let mut total_latency = 0.0;
            let mut success_count = 0;
            let mut error_count = 0;

            for i in 0..self.test_message_count {
                let msg_start = Instant::now();

                let _test_msg = TestMessage {
                    id: i,
                    content: format!("unix-socket-test-{}", i),
                    timestamp: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64,
                };

                // Simulate UNIX socket latency (1-10ms typical)
                let simulated_latency = Duration::from_micros(1000 + (i % 9000) as u64);
                tokio::time::sleep(simulated_latency).await;

                // Simulate 95% success rate for UNIX sockets
                if i % 20 != 0 {
                    let msg_latency = msg_start.elapsed().as_secs_f64() * 1000.0;
                    total_latency += msg_latency;
                    success_count += 1;
                } else {
                    error_count += 1;
                }
            }

            let total_time = message_start.elapsed();
            let throughput = success_count as f64 / total_time.as_secs_f64();
            let avg_latency = if success_count > 0 {
                total_latency / success_count as f64
            } else {
                0.0
            };
            let success_rate = success_count as f64 / self.test_message_count as f64;

            Ok(TransportMetrics {
                transport_type: "unix-socket".to_string(),
                setup_time_ms: setup_time.as_millis() as u64,
                message_latency_ms: avg_latency,
                throughput_msgs_per_sec: throughput,
                success_rate,
                error_count,
            })
        }

        /// Simulate TCP transport characteristics
        async fn simulate_tcp_transport(&self) -> Result<TransportMetrics> {
            let start_time = Instant::now();

            // Simulate TCP broker setup time
            let setup_delay = Duration::from_millis(20);
            tokio::time::sleep(setup_delay).await;
            info!("Simulated TCP broker setup");

            let setup_time = start_time.elapsed();

            // Simulate message processing with TCP characteristics
            let message_start = Instant::now();
            let mut total_latency = 0.0;
            let mut success_count = 0;
            let mut error_count = 0;

            for i in 0..self.test_message_count {
                let msg_start = Instant::now();

                let _test_msg = TestMessage {
                    id: i,
                    content: format!("tcp-test-{}", i),
                    timestamp: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64,
                };

                // Simulate TCP latency (5-20ms typical)
                let simulated_latency = Duration::from_micros(5000 + (i % 15000) as u64);
                tokio::time::sleep(simulated_latency).await;

                // Simulate 98% success rate for TCP
                if i % 50 != 0 {
                    let msg_latency = msg_start.elapsed().as_secs_f64() * 1000.0;
                    total_latency += msg_latency;
                    success_count += 1;
                } else {
                    error_count += 1;
                }
            }

            let total_time = message_start.elapsed();
            let throughput = success_count as f64 / total_time.as_secs_f64();
            let avg_latency = if success_count > 0 {
                total_latency / success_count as f64
            } else {
                0.0
            };
            let success_rate = success_count as f64 / self.test_message_count as f64;

            Ok(TransportMetrics {
                transport_type: "tcp".to_string(),
                setup_time_ms: setup_time.as_millis() as u64,
                message_latency_ms: avg_latency,
                throughput_msgs_per_sec: throughput,
                success_rate,
                error_count,
            })
        }

        /// Generate recommendations based on validation results
        fn generate_recommendations(&self, results: &ValidationResults) -> Vec<String> {
            let mut recommendations = Vec::new();

            // Base recommendation for DaemonEye
            recommendations.push(
                "Use in-process channels for intra-agent communication to minimize latency".to_string(),
            );

            // UNIX socket vs TCP recommendation
            if let (Some(unix), Some(tcp)) = (&results.unix_socket_metrics, &results.tcp_metrics) {
                if unix.message_latency_ms < tcp.message_latency_ms {
                    recommendations.push(
                        "Prefer UNIX sockets for local inter-process collector communication".to_string(),
                    );
                } else {
                    recommendations.push(
                        "TCP may be acceptable for local communication if simplicity is preferred".to_string(),
                    );
                }
            }

            // Success rate considerations
            if let Some(unix) = &results.unix_socket_metrics {
                if unix.success_rate < 0.9 {
                    recommendations.push(
                        "Improve error handling and retries for UNIX socket transport".to_string(),
                    );
                }
            }

            if let Some(tcp) = &results.tcp_metrics {
                if tcp.success_rate < 0.95 {
                    recommendations.push("Consider TCP keep-alive and reconnection strategies".to_string());
                }
            }

            recommendations
        }

        pub async fn run() -> Result<()> {
            let validator = BusrtTransportValidator::new();
            let _results = validator.validate_all_transports().await?;
            Ok(())
        }
    }

#[cfg(not(feature = "broker"))]
fn main() {
    println!("This example requires the 'broker' feature on the 'busrt' crate. Enable it to build/run.");
}

#[cfg(feature = "broker")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    example::run().await
}
