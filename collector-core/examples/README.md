# Collector-Core Examples

This directory is reserved for examples that exercise the DaemonEye event bus integration and higher-level collector orchestration flows.

Historically this folder tracked busrt proof-of-concepts. The project has fully migrated to the in-house `daemoneye-eventbus` crate, so the old busrt material has been removed. Upcoming examples will focus on:

- Running an embedded broker inside `daemoneye-agent`
- Publishing and consuming `CollectionEvent` messages from multiple collectors
- Demonstrating graceful startup/shutdown workflows with the new transport
- Exercising RPC coordination once the protocol is finalized

The examples will land alongside the feature work in `.kiro/specs/daemoneye-core-monitoring/tasks.md`. Until then this directory is intentionally empty.
