# Backlog · Event store cross-version importer (first schema v2)

**Milestone:** Unscheduled — triggers when `schema_version` first advances past its initial value · **Origin:** T3 (`spec/full/tickets/T3_·_M2_—_Storage_engine_(redb_§11.7_event_store_+_codecs_+_retention).md`)

## Why this exists

T3 ships the event store's signed export bundle on a `schema_version` mismatch (raw key/value bytes + manifest naming the old version + Ed25519 signature) and *locks the bundle format*, but deliberately defers the **importer** and the **historical-codec registry**. At T3 there is only one schema version, so the hard part — cross-version decode — has nothing to run against and cannot be tested. This ticket is the placeholder so that obligation is not lost.

## Scope (to be refined when v2 lands)

**In:**

- A version-aware importer that reads a T3 export bundle and re-ingests it into a compatible event store.
- A historical-codec registry retaining the ability to decode every prior `schema_version` value (postcard is not self-describing; the running binary must keep old decoders alive to read old bundles).
- Tests exercising a real v1→v2 cross-version decode + re-ingest round-trip.

**Out:** In-place live-store migration (the spec forbids it; rebuild-on-mismatch stays the startup path).

## Notes

- Additive-only codec evolution (version byte, optional trailing fields, no reordering) is the T3 design intent that keeps most schema changes backward-compatible — so this importer is expected to be needed rarely.
- Spec references: design.md §11.6–11.7; the T3 storage-engine brainstorm at `docs/brainstorms/2026-06-13-storage-engine-event-store-requirements.md`.
