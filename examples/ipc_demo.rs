//! Cross-platform IPC demonstration using the interprocess transport
//!
//! This example demonstrates the portable, cross-platform nature of the
//! SentinelD IPC implementation using the `interprocess` crate.

use sentinel_lib::ipc::{InterprocessClient, InterprocessServer, IpcConfig};
use sentinel_lib::proto::{DetectionResult, DetectionTask, TaskType};
use std::time::Duration;
use tokio::time::timeout;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing for better visibility
    tracing_subscriber::fmt::init();

    println!("🚀 SentinelD Cross-Platform IPC Demo");
    println!("====================================");

    // Create a platform-appropriate endpoint
    let endpoint = create_demo_endpoint();
    println!("📡 Using endpoint: {}", endpoint);

    // Create IPC configuration
    let config = IpcConfig {
        endpoint_path: endpoint,
        accept_timeout_ms: 1000,
        read_timeout_ms: 5000,
        write_timeout_ms: 5000,
        max_connections: 1,
        ..Default::default()
    };

    // Create and start server
    println!("🔧 Starting server...");
    let mut server = InterprocessServer::new(config.clone())?;

    // Set up a simple echo handler
    server.set_handler(|task: DetectionTask| async move {
        println!("📥 Server received task: {}", task.task_id);
        Ok(DetectionResult {
            task_id: task.task_id.clone(),
            success: true,
            error_message: None,
            processes: vec![],
            hash_result: None,
        })
    });

    server.start().await?;
    println!("✅ Server started successfully!");

    // Give server time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Create client and send test task
    println!("🔌 Creating client...");
    let mut client = InterprocessClient::new(config)?;

    let test_task = DetectionTask {
        task_id: "cross-platform-demo-task".to_string(),
        task_type: TaskType::EnumerateProcesses as i32,
        process_filter: None,
        hash_check: None,
        metadata: Some("Cross-platform IPC demo".to_string()),
    };

    println!("📤 Sending test task...");
    let result = timeout(Duration::from_secs(5), client.send_task(test_task))
        .await
        .map_err(|_| "Request timed out")??;

    println!("📨 Received response:");
    println!("  - Task ID: {}", result.task_id);
    println!("  - Success: {}", result.success);
    println!("  - Error: {:?}", result.error_message);

    // Clean up
    server.stop().await?;
    println!("🛑 Server stopped");

    println!("✨ Cross-platform IPC demo completed successfully!");
    println!("   This same code works on Unix, macOS, and Windows!");

    Ok(())
}

fn create_demo_endpoint() -> String {
    #[cfg(unix)]
    {
        // Unix domain socket
        let temp_dir = std::env::temp_dir();
        temp_dir
            .join("sentineld-ipc-demo.sock")
            .to_string_lossy()
            .to_string()
    }
    #[cfg(windows)]
    {
        // Windows named pipe
        r"\\.\pipe\sentineld\ipc-demo".to_string()
    }
}
