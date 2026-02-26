# EventBus Performance Comparison Tests

This document describes the comprehensive performance comparison tests between the crossbeam-based `LocalEventBus` and the daemoneye-eventbus-based `DaemoneyeEventBus` implementations.

## Test Coverage

### 1. Throughput Comparison (`test_throughput_comparison`)

- **Purpose**: Compare event distribution throughput between implementations
- **Metrics**: Events per second, latency per event, successful subscribers
- **Configuration**: 5,000 events, 2 subscribers, 15-second timeout
- **Results**: DaemonEye EventBus achieves ~29% of crossbeam throughput with ~3.4x latency

### 2. Latency Characteristics (`test_latency_characteristics`)

- **Purpose**: Test latency under different load conditions
- **Test Cases**:
  - Low load: 1,000 events, 1 subscriber
  - Medium load: 5,000 events, 3 subscribers
  - High load: 10,000 events, 5 subscribers
- **Validation**: Both implementations maintain \<10ms average latency

### 3. Memory Usage Comparison (`test_memory_usage_comparison`)

- **Purpose**: Compare memory consumption under load
- **Configuration**: 8,000 events, 4 subscribers
- **Validation**: Both implementations stay under 100MB memory usage

### 4. Behavioral Equivalence (`test_behavioral_equivalence`)

- **Purpose**: Ensure identical event delivery behavior
- **Configuration**: 1,000 events, 2 subscribers
- **Validation**: Both implementations deliver all events with matching content

### 5. Concurrent Subscriber Performance (`test_concurrent_subscriber_performance`)

- **Purpose**: Test performance with multiple concurrent subscribers
- **Configuration**: 3,000 events, 6 subscribers
- **Validation**: Both implementations handle concurrent subscribers effectively

### 6. Backpressure Handling (`test_backpressure_handling_comparison`)

- **Purpose**: Test behavior under high load with limited buffer space
- **Configuration**: 15,000 events, 2 subscribers, 5,000 buffer size
- **Validation**: Both implementations process >80% of events under backpressure

## Performance Results Summary

| Metric                 | Crossbeam LocalEventBus | DaemonEye EventBus  | Ratio     |
| ---------------------- | ----------------------- | ------------------- | --------- |
| Throughput             | ~450,000 events/sec     | ~130,000 events/sec | 0.29x     |
| Latency                | ~2.2 μs/event           | ~7.6 μs/event       | 3.4x      |
| Memory Usage           | Minimal                 | Minimal             | Similar   |
| Behavioral Equivalence | ✅                      | ✅                  | Identical |

## Key Findings

1. **Performance Trade-off**: DaemonEye EventBus provides ~29% of crossbeam throughput, which is acceptable given the additional features (topic routing, embedded broker, cross-process communication)

2. **Latency Impact**: ~3.4x latency increase is reasonable for the message broker architecture

3. **Behavioral Equivalence**: Both implementations deliver identical results, ensuring migration safety

4. **Scalability**: Both handle concurrent subscribers and backpressure gracefully

5. **Memory Efficiency**: Both implementations have similar memory footprints

## Benchmark Integration

Additional criterion benchmarks are available in `benches/eventbus_comparison_benchmarks.rs`:

- `eventbus_throughput_comparison`: Detailed throughput measurements
- `subscriber_scalability_comparison`: Subscriber scaling performance
- `latency_comparison`: Latency characteristics across batch sizes
- `memory_efficiency_comparison`: Memory usage benchmarks

## Running the Tests

```bash
# Run all performance comparison tests
cargo test --test eventbus_performance_comparison -- --nocapture

# Run specific test
cargo test test_throughput_comparison --test eventbus_performance_comparison -- --nocapture

# Run benchmarks
cargo bench --bench eventbus_comparison_benchmarks
```

## Conclusion

The migration from crossbeam to daemoneye-eventbus maintains behavioral equivalence while providing the foundation for multi-process communication and advanced message routing. The performance trade-off (3.4x latency, 0.29x throughput) is acceptable given the architectural benefits and aligns with the requirements for the collector-core framework.
