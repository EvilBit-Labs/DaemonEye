# ADR: Migrate IPC Transport to Interprocess Crate

**Status**: In Progress **Date**: 2025-09-18 **Authors**: UncleSp1d3r

## Summary

Migrate DaemonEye's custom IPC implementation from hand-rolled Unix domain sockets and Windows named pipes to a unified transport layer using standard tokio networking APIs as a stepping stone toward the `interprocess` crate.

## Context

DaemonEye currently implements IPC communication between `procmond` and `daemoneye-agent` using custom implementations:

- **Custom Unix socket server** (~400 lines) with manual varint length framing, CRC32 validation, and connection management
- **Custom Windows named pipe server** with platform-specific error handling
- **Complex protocol layer** with custom message parsing and flow control
- **Maintenance overhead** of supporting multiple platform-specific implementations

### Current Implementation Issues

1. **High maintenance burden**: Complex custom networking code that duplicates standard library functionality
2. **Platform complexity**: Separate implementations for Unix and Windows with different error handling
3. **Limited testing**: Custom implementation lacks battle-testing compared to standard libraries
4. **Development velocity**: Time spent on IPC infrastructure instead of core security features

### Requirements

- **Security**: Preserve all current security features (permissions, connection limits, timeouts)
- **Protocol compatibility**: Maintain protobuf message format and CRC32 validation
- **Performance**: No regression in message throughput or latency
- **Rollback capability**: Ability to revert to legacy implementation if issues arise

## Decision

### Phase 1: Simplify with Tokio Native APIs (Current)

Replace custom socket implementations with standard tokio APIs:

- **Unix platforms**: `tokio::net::UnixListener` and `tokio::net::UnixStream`
- **Windows platforms**: `tokio::net::windows::named_pipe` (future work)
- **Codec preservation**: Keep existing protobuf + CRC32 framing protocol
- **Feature flags**: `ipc-interprocess` (default) vs `ipc-legacy` (rollback)

### Phase 2: Interprocess Crate Migration (Future)

Evaluate and potentially migrate to `interprocess::local_socket` for true cross-platform unification once Phase 1 is validated.

## Implementation Strategy

### 1. Transport Layer

```rust
// New unified config
pub struct IpcConfig {
    pub transport: TransportType,
    pub endpoint_path: String,  // Unix socket path or Windows pipe name
    pub max_frame_bytes: usize, // 1MB default
    pub accept_timeout_ms: u64, // 5 seconds
    pub read_timeout_ms: u64,   // 30 seconds
    pub write_timeout_ms: u64,  // 10 seconds
    pub max_connections: usize, // 16 default
    pub crc32_variant: Crc32Variant, // IEEE default
}
```

### 2. Security Preservation

- **Unix socket permissions**: Directory 0700, socket 0600 (owner-only access)
- **Connection limits**: Tokio semaphore-based connection limiting
- **Timeouts**: Per-operation timeouts with graceful degradation
- **No network access**: Local sockets/pipes only, verified by tests

### 3. Codec Compatibility

Preserve existing wire protocol:

```text
Frame format (little-endian):
├── u32 length (N) of protobuf message bytes
├── u32 crc32 checksum over message bytes
└── N bytes protobuf message (prost-encoded)
```

### 4. Rollback Strategy

- Feature flag `ipc-legacy` maintains original implementation
- Configuration toggle: `ipc.transport = "legacy"`
- Runtime telemetry identifies active transport
- Zero protocol changes enable hot rollback

## Benefits

### Immediate (Phase 1)

1. **Reduced complexity**: ~600 lines of custom code replaced with ~200 lines using standard APIs
2. **Better reliability**: Tokio's battle-tested networking stack
3. **Improved maintainability**: Standard error handling and lifecycle management
4. **Enhanced debugging**: Standard tooling works with tokio sockets

### Future (Phase 2)

1. **True cross-platform**: Single API for Unix and Windows
2. **Additional transports**: Shared memory, better named pipe support
3. **Community support**: Active maintenance and security updates
4. **Feature expansion**: Easier to add enterprise features (mTLS, etc.)

## Risks and Mitigations

### Risk: Performance Regression

- **Likelihood**: Low - tokio APIs are highly optimized
- **Mitigation**: Comprehensive benchmarks comparing old vs new implementation
- **Acceptance criteria**: \<5% performance regression

### Risk: Platform-Specific Issues

- **Likelihood**: Medium - different behavior across platforms
- **Mitigation**: Extensive cross-platform testing (Linux, macOS, Windows)
- **Acceptance criteria**: All current tests pass on all platforms

### Risk: Security Boundary Changes

- **Likelihood**: Low - same permission model maintained
- **Mitigation**: Security-focused testing with permission validation
- **Acceptance criteria**: Identical access control and privilege separation

### Risk: Interprocess Crate Limitations

- **Likelihood**: Medium - newer crate with less real-world usage
- **Mitigation**: Phase 1 with tokio provides stable fallback
- **Rollback plan**: Revert to Phase 1 implementation if Phase 2 issues arise

## Alternatives Considered

### Alternative 1: Keep Custom Implementation

**Pros**:

- No migration risk
- Complete control over behavior

**Cons**:

- Ongoing maintenance burden
- Limited feature expansion
- Platform complexity continues

**Decision**: Rejected due to high maintenance cost

### Alternative 2: Direct Migration to Interprocess Crate

**Pros**:

- Single migration effort
- Immediate cross-platform benefits

**Cons**:

- Higher risk single migration
- Less mature crate ecosystem
- Harder rollback path

**Decision**: Rejected in favor of phased approach

### Alternative 3: gRPC or Other RPC Framework

**Pros**:

- Industry standard
- Rich feature set
- Good tooling

**Cons**:

- Protocol breaking changes
- Network dependency concerns
- Overhead for simple use case

**Decision**: Rejected due to protocol compatibility requirements

## Success Metrics

### Functional Requirements

- ✅ All existing IPC tests pass
- ✅ Zero protocol changes required
- ✅ Feature flag rollback works
- 🔄 Cross-platform compatibility maintained

### Performance Requirements

- 📊 \<5% latency regression in request/response cycles
- 📊 Equal or better throughput for message encoding/decoding
- 📊 Memory usage within 10% of current implementation

### Security Requirements

- 🔒 Unix socket permissions enforced (0700 dir, 0600 socket)
- 🔒 Connection limits respected under load
- 🔒 All timeouts trigger appropriately
- 🔒 No unintended network listeners created

### Operational Requirements

- 📈 Zero breaking changes for operators
- 📈 Configuration backward compatibility
- 📈 Logging and metrics parity maintained

## Implementation Status

### Phase 1: Tokio Native APIs

- ✅ Feature flags and configuration
- ✅ Codec implementation with CRC32 validation
- ✅ Basic Unix socket server implementation
- ✅ Basic client implementation
- ✅ Unit tests passing
- 🔄 Windows named pipe implementation
- 🔄 Integration tests
- 🔄 Performance benchmarking
- 🔄 Security validation

### Future: Phase 2 Evaluation

- ⏸️ Interprocess crate API analysis
- ⏸️ Cross-platform compatibility testing
- ⏸️ Migration path planning

## Next Steps

1. **Complete Windows support** with tokio named pipes
2. **Add integration tests** for client-server communication
3. **Implement performance benchmarks** vs legacy implementation
4. **Security testing** with permission validation
5. **Update documentation** with new configuration options
6. **CI/CD integration** with cross-platform testing matrix

## References

- [Interprocess crate documentation](https://docs.rs/interprocess/)
- [Tokio networking guide](https://tokio.rs/tokio/tutorial/streams)
- [DaemonEye IPC requirements](../.kiro/specs/DaemonEye-core-monitoring/requirements.md)
- [Current IPC implementation](../procmond/src/ipc)

---

**Note**: This ADR will be updated as implementation progresses and Phase 2 decisions are made.
