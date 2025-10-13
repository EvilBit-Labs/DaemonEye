use daemoneye_eventbus::{Topic, TopicAccessLevel, TopicPattern, TopicRegistry};

/// Demonstrates the topic hierarchy design for multi-collector communication
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("DaemonEye Topic Hierarchy Demo");
    println!("==============================\n");

    // Create topic registry for managing component access
    let mut registry = TopicRegistry::new();

    // Register component topic usage patterns
    register_component_topics(&mut registry)?;

    // Demonstrate topic creation and validation
    demonstrate_topic_creation()?;

    // Demonstrate wildcard pattern matching
    demonstrate_pattern_matching()?;

    // Demonstrate access control
    demonstrate_access_control(&registry);

    // Show practical usage scenarios
    demonstrate_usage_scenarios(&registry)?;

    Ok(())
}

/// Register typical component topic usage patterns
fn register_component_topics(
    registry: &mut TopicRegistry,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("1. Registering Component Topic Usage");
    println!("------------------------------------");

    // procmond (Process Collector) - publishes process events, subscribes to collector control
    registry.register_publisher("procmond", "events.process.lifecycle")?;
    registry.register_publisher("procmond", "events.process.metadata")?;
    registry.register_publisher("procmond", "events.process.tree")?;
    registry.register_publisher("procmond", "events.process.integrity")?;
    registry.register_publisher("procmond", "events.process.anomaly")?;
    registry.register_publisher("procmond", "events.process.batch")?;
    registry.register_publisher("procmond", "control.health.status")?;
    registry.register_publisher("procmond", "control.health.heartbeat")?;

    registry.register_subscriber("procmond", "control.collector.#")?;
    registry.register_subscriber("procmond", "control.health.heartbeat")?;

    println!("✓ Registered procmond topics");

    // daemoneye-agent (Orchestrator) - subscribes to all events, publishes control messages
    registry.register_publisher("daemoneye-agent", "control.collector.lifecycle")?;
    registry.register_publisher("daemoneye-agent", "control.collector.config")?;
    registry.register_publisher("daemoneye-agent", "control.collector.task")?;
    registry.register_publisher("daemoneye-agent", "control.agent.orchestration")?;
    registry.register_publisher("daemoneye-agent", "control.agent.detection")?;
    registry.register_publisher("daemoneye-agent", "control.agent.alert")?;
    registry.register_publisher("daemoneye-agent", "control.health.heartbeat")?;

    registry.register_subscriber("daemoneye-agent", "events.#")?;
    registry.register_subscriber("daemoneye-agent", "control.agent.#")?;
    registry.register_subscriber("daemoneye-agent", "control.health.#")?;

    println!("✓ Registered daemoneye-agent topics");

    // Future netmond (Network Collector)
    registry.register_publisher("netmond", "events.network.connections")?;
    registry.register_publisher("netmond", "events.network.dns")?;
    registry.register_publisher("netmond", "events.network.traffic")?;
    registry.register_publisher("netmond", "events.network.anomaly")?;
    registry.register_publisher("netmond", "control.health.status")?;

    registry.register_subscriber("netmond", "control.collector.#")?;

    println!("✓ Registered netmond topics (future)");

    // External SIEM system - subscribes to security-relevant events only
    registry.register_subscriber("siem-connector", "events.+.security")?;
    registry.register_subscriber("siem-connector", "events.+.anomaly")?;
    registry.register_subscriber("siem-connector", "control.health.alerts")?;

    println!("✓ Registered SIEM connector topics");
    println!();

    Ok(())
}

/// Demonstrate topic creation and validation
fn demonstrate_topic_creation() -> Result<(), Box<dyn std::error::Error>> {
    println!("2. Topic Creation and Validation");
    println!("--------------------------------");

    // Valid event topics
    let valid_topics = vec![
        "events.process.lifecycle",
        "events.process.metadata",
        "events.network.connections",
        "events.filesystem.operations",
        "events.performance.system",
    ];

    println!("Valid event topics:");
    for topic_str in &valid_topics {
        let topic = Topic::new(topic_str)?;
        println!(
            "  ✓ {} -> domain: {:?}, type: {:?}",
            topic.name(),
            topic.domain(),
            topic.topic_type()
        );
    }

    // Valid control topics
    let control_topics = vec![
        "control.collector.lifecycle",
        "control.agent.orchestration",
        "control.health.heartbeat",
    ];

    println!("\nValid control topics:");
    for topic_str in &control_topics {
        let topic = Topic::new(topic_str)?;
        println!(
            "  ✓ {} -> domain: {:?}, type: {:?}",
            topic.name(),
            topic.domain(),
            topic.topic_type()
        );
    }

    // Invalid topics (demonstrate validation)
    let invalid_topics = vec![
        ("", "empty topic name"),
        ("events..lifecycle", "empty segment"),
        ("events.Process.lifecycle", "uppercase not allowed"),
        ("system.reserved", "reserved prefix"),
        ("events.process.lifecycle.too.deep", "too many levels"),
    ];

    println!("\nInvalid topics (validation errors):");
    for (topic_str, reason) in &invalid_topics {
        match Topic::new(topic_str) {
            Err(e) => println!("  ✗ '{}' -> {} ({})", topic_str, e, reason),
            Ok(_) => println!("  ? '{}' -> unexpectedly valid", topic_str),
        }
    }

    println!();
    Ok(())
}

/// Demonstrate wildcard pattern matching
fn demonstrate_pattern_matching() -> Result<(), Box<dyn std::error::Error>> {
    println!("3. Wildcard Pattern Matching");
    println!("----------------------------");

    // Create test topics
    let topics = vec![
        Topic::new("events.process.lifecycle")?,
        Topic::new("events.process.metadata")?,
        Topic::new("events.network.connections")?,
        Topic::new("events.filesystem.operations")?,
        Topic::new("control.collector.status")?,
        Topic::new("control.health.heartbeat")?,
    ];

    // Test patterns
    let patterns = vec![
        ("events.#", "All event topics"),
        ("events.process.+", "All process event types"),
        ("events.+.lifecycle", "Lifecycle events from all domains"),
        ("control.+.status", "Status messages from all components"),
        ("control.health.#", "All health monitoring topics"),
    ];

    for (pattern_str, description) in &patterns {
        let pattern = TopicPattern::new(pattern_str)?;
        println!("Pattern: {} ({})", pattern_str, description);

        for topic in &topics {
            let matches = pattern.matches(topic);
            let indicator = if matches { "✓" } else { "✗" };
            println!("  {} {}", indicator, topic.name());
        }
        println!();
    }

    Ok(())
}

/// Demonstrate access control
fn demonstrate_access_control(registry: &TopicRegistry) {
    println!("4. Access Control Demonstration");
    println!("-------------------------------");

    let test_topics = vec![
        ("control.health.status", "Public health topic"),
        ("events.process.anomaly", "Public anomaly event"),
        ("control.collector.config", "Restricted config topic"),
        ("events.process.metadata", "Restricted metadata topic"),
        ("control.collector.lifecycle", "Privileged lifecycle topic"),
        ("control.agent.policy", "Privileged policy topic"),
    ];

    for (topic, description) in &test_topics {
        let access_level = registry.get_access_level(topic);
        let level_str = match access_level {
            TopicAccessLevel::Public => "PUBLIC",
            TopicAccessLevel::Restricted => "RESTRICTED",
            TopicAccessLevel::Privileged => "PRIVILEGED",
        };
        println!("  {} -> {} ({})", topic, level_str, description);
    }

    println!();
}

/// Demonstrate practical usage scenarios
fn demonstrate_usage_scenarios(registry: &TopicRegistry) -> Result<(), Box<dyn std::error::Error>> {
    println!("5. Practical Usage Scenarios");
    println!("----------------------------");

    // Scenario 1: procmond publishing process events
    println!("Scenario 1: procmond publishing process lifecycle event");
    let topic = "events.process.lifecycle";
    let can_publish = registry.can_publish("procmond", topic);
    println!(
        "  Can procmond publish to '{}'? {}",
        topic,
        if can_publish { "✓ YES" } else { "✗ NO" }
    );

    // Scenario 2: daemoneye-agent subscribing to all events
    println!("\nScenario 2: daemoneye-agent subscribing to process events");
    let process_topic = Topic::new("events.process.lifecycle")?;
    let can_subscribe = registry.can_subscribe("daemoneye-agent", &process_topic);
    println!(
        "  Can daemoneye-agent subscribe to '{}'? {}",
        process_topic.name(),
        if can_subscribe { "✓ YES" } else { "✗ NO" }
    );

    // Scenario 3: SIEM connector subscribing to security events
    println!("\nScenario 3: SIEM connector subscribing to anomaly events");
    let anomaly_topic = Topic::new("events.process.anomaly")?;
    let can_subscribe = registry.can_subscribe("siem-connector", &anomaly_topic);
    println!(
        "  Can SIEM connector subscribe to '{}'? {}",
        anomaly_topic.name(),
        if can_subscribe { "✓ YES" } else { "✗ NO" }
    );

    // Scenario 4: Unauthorized access attempt
    println!("\nScenario 4: Unauthorized access attempt");
    let config_topic = "control.collector.config";
    let can_publish = registry.can_publish("siem-connector", config_topic);
    println!(
        "  Can SIEM connector publish to '{}'? {}",
        config_topic,
        if can_publish {
            "✓ YES"
        } else {
            "✗ NO (expected)"
        }
    );

    println!();

    // Show component topic summaries
    println!("Component Topic Summaries:");
    println!("-------------------------");

    let components = vec!["procmond", "daemoneye-agent", "netmond", "siem-connector"];
    for component in &components {
        println!("{}:", component);

        if let Some(publishers) = registry.get_publishers(component) {
            println!("  Publishers: {}", publishers.len());
            for topic in publishers.iter().take(3) {
                println!("    - {}", topic);
            }
            if publishers.len() > 3 {
                println!("    ... and {} more", publishers.len() - 3);
            }
        }

        if let Some(subscribers) = registry.get_subscribers(component) {
            println!("  Subscribers: {}", subscribers.len());
            for pattern in subscribers.iter().take(3) {
                println!("    - {}", pattern.pattern());
            }
            if subscribers.len() > 3 {
                println!("    ... and {} more", subscribers.len() - 3);
            }
        }
        println!();
    }

    Ok(())
}
