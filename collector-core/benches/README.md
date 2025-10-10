# Collector-Core Benchmarks

This directory contains Criterion benchmarks for performance regression testing.

## Event Bus Benchmarks (`event_bus_benchmarks.rs`)

Measures critical performance characteristics of the event bus implementation to catch regressions.

### Benchmarks

- **event_bus_startup**: Full lifecycle (create, start, shutdown) - establishes baseline startup cost
- **subscription_management**: Subscribe/unsubscribe operations - measures registration overhead
- **event_publishing**: Throughput for publishing events - critical path performance
- **event_delivery**: End-to-end latency from publish to subscriber receive
- **multi_subscriber_fanout**: Scaling behavior with multiple subscribers
- **sustained_throughput**: Sustained event delivery rates
- **statistics_overhead**: Performance cost of statistics collection
- **memory_pressure**: Behavior under high event volumes

### Performance Expectations

Conservative targets for CI environments:

- **Startup**: < 200ms (dev: < 100ms)
- **Subscribe/Unsubscribe**: < 50ms per operation (dev: < 20ms)
- **Publishing**: > 500 events/sec (dev: > 1000 events/sec)
- **Delivery Latency**: < 10ms end-to-end (dev: < 5ms)
- **Fanout**: Linear scaling up to 10 subscribers (dev: 20)
- **Statistics**: < 1ms per call (dev: < 0.5ms)

### Running Benchmarks

```bash
# Run all event_bus benchmarks
cargo bench -p collector-core --bench event_bus_benchmarks

# Run specific benchmark
cargo bench -p collector-core --bench event_bus_benchmarks 'event_publishing'

# Quick smoke test (reduced samples)
cargo bench -p collector-core --bench event_bus_benchmarks -- --quick

# Save baseline for comparison
cargo bench -p collector-core --bench event_bus_benchmarks -- --save-baseline main

# Compare against baseline
cargo bench -p collector-core --bench event_bus_benchmarks -- --baseline main
```

### CI Integration

The benchmarks are designed to accommodate resource-constrained CI runners:

- Sample sizes reduced for expensive operations (10 samples for lifecycle tests)
- Conservative performance targets
- Graceful handling of timing variance

### Interpreting Results

- **Mean time**: Average performance across all samples
- **Std dev**: Variance in measurements (lower is better for consistency)
- **Throughput**: Events/second for publishing/delivery benchmarks
- **Outliers**: Samples outside normal range (some variance expected on CI)

### Notes

- Full lifecycle benchmarks (startup, subscription) include expensive setup/teardown
- These establish baselines but may show variance on CI due to resource contention
- Core operation benchmarks (publishing, delivery) are more stable regression indicators
- The event bus uses an embedded busrt broker with Unix sockets/named pipes for IPC
- Socket path synchronization ensures reliable broker-client connections

## Collector Benchmarks (`collector_benchmarks.rs`)

Measures collector framework performance including event batching, backpressure, and shutdown coordination.

See individual benchmark documentation for details on performance targets and CI integration.
