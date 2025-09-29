//! Minimal busrt API test to validate actual available methods

use anyhow::Result;
use tracing::info;

/// Test actual busrt API availability
pub async fn test_busrt_api() -> Result<()> {
    info!("Testing busrt API availability");

    // Test 1: Check if we can create a broker
    info!("Testing broker creation");
    let _broker = busrt::broker::Broker::new();
    info!("✅ Broker creation successful");

    // Test 2: Check available broker methods
    info!("Testing broker methods");
    // We'll discover what methods are actually available

    // Test 3: Check client API
    info!("Testing client API");
    // Check what client methods are available

    info!("API test completed");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    test_busrt_api().await
}
