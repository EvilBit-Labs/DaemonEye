# Collector-Core Examples

This directory contains examples demonstrating various aspects of the collector-core framework and its integrations.

## Busrt Integration Examples

### busrt_minimal_poc.rs

**Purpose**: Demonstrates the basic busrt integration proof-of-concept for task 2.1.4.

**Features Demonstrated**:

- ✅ Embedded busrt broker in a simple application
- ✅ Basic pub/sub message exchange between broker and client
- ✅ RPC call patterns for request/response communication
- ✅ Graceful startup and shutdown sequences

**Key API Patterns**:

- `Broker::new()` - Create embedded broker instance
- `broker.spawn_fifo(path, buffer_size)` - Start FIFO transport
- `Config::new(path, client_name)` - Create client configuration
- `Client::connect(&config)` - Connect client to broker
- `client.publish(topic, payload, qos)` - Publish messages
- `client.subscribe(topic, qos)` - Subscribe to topics

**Usage**:

```bash
# Run the proof-of-concept
RUST_LOG=info cargo run --example busrt_minimal_poc

# Run tests
cargo test --example busrt_minimal_poc
```

**Expected Behavior**:

- In environments where busrt broker can start: Full functionality with actual message passing
- In test/restricted environments: Graceful degradation with API pattern demonstration

### Other Busrt Examples

- `busrt_api_test.rs` - Simple API exploration and testing
- `busrt_basic_example.rs` - Research and documentation of busrt patterns
- `busrt_corrected_example.rs` - API research and integration planning
- `busrt_transport_validation.rs` - Transport layer validation and performance testing
- `busrt_integration_poc.rs` - More comprehensive POC (work in progress)

## Integration with DaemonEye

The busrt integration examples serve as the foundation for migrating DaemonEye's event bus from crossbeam channels to busrt message broker, enabling:

1. **Multi-process communication** between daemoneye-agent and collector processes
2. **Pub/sub patterns** for event distribution across monitoring domains
3. **RPC patterns** for collector lifecycle management and health checks
4. **Scalable architecture** supporting future network-distributed deployments

## Requirements Validation

Task 2.1.4 requirements have been successfully validated:

- ✅ **Embedded busrt broker**: `Broker::new()` and `spawn_fifo()` demonstrate embedded deployment
- ✅ **Pub/sub message exchange**: Publisher/subscriber pattern with serialized messages
- ✅ **RPC call patterns**: Request/response communication using topic-based routing
- ✅ **Graceful lifecycle**: Proper startup, operational validation, and shutdown sequences

## Next Steps

The proof-of-concept provides the foundation for:

1. Implementing `BusrtEventBus` in collector-core (Task 2.3)
2. Migrating event distribution from crossbeam to busrt (Task 2.4)
3. Adding RPC patterns for collector lifecycle management (Task 2.5)
4. Supporting multi-process collector coordination (Task 2.6)
