//! Simple busrt API test to understand the correct usage patterns

use anyhow::Result;
use busrt::broker::Broker;

// Let's try to import client modules to see what's available
use busrt::client::AsyncClient;
use busrt::ipc::{Client, Config};

/// Test basic busrt API to understand correct usage
pub async fn test_busrt_api() -> Result<()> {
    println!("Testing busrt API patterns");

    // Test broker creation
    let mut broker = Broker::new();
    println!("✅ Broker created successfully");

    // Test FIFO spawn
    match broker.spawn_fifo("test.sock", 100).await {
        Ok(()) => {
            println!("✅ FIFO transport spawned successfully");
        }
        Err(e) => {
            println!("⚠️  FIFO spawn failed (expected in test): {}", e);
        }
    }

    // Test client connection using concrete Client type
    println!("Testing client connection...");

    let config = Config::new("fifo:test.sock", "test_client");
    match Client::connect(&config).await {
        Ok(mut client) => {
            println!("✅ Client connected successfully");

            // Test basic operations
            match client.subscribe("test.topic", busrt::QoS::No).await {
                Ok(receiver_opt) => {
                    println!(
                        "✅ Subscription successful, receiver: {:?}",
                        receiver_opt.is_some()
                    );
                }
                Err(e) => println!("⚠️  Subscription failed: {}", e),
            }
        }
        Err(e) => {
            println!("⚠️  Client connection failed (expected in test): {}", e);
        }
    }

    println!("Broker API test completed");

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    test_busrt_api().await
}
