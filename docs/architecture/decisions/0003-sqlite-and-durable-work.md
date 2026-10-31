# ADR 0003: Use SQLite as authoritative state with durable leased jobs

- Status: accepted
- Date: 2026-10-31

## Context

Depot targets one VM and needs reliable recovery without operating an additional
database or queue. Deployment side effects span Git, Docker, files, and Caddy and
cannot be committed atomically with database changes.

## Decision

Use SQLx with SQLite in WAL mode, foreign keys, and a bounded busy timeout. Store
jobs and ordered events durably. Workers claim jobs using expiring leases and
idempotency keys, and reconcile external state after interruption. Active release
changes use database transactions, but external side effects use explicit staged
state transitions and compensating cleanup.

## Consequences

Backup and operation suit a personal single-node service, and restarts do not lose
queued work. Write throughput is intentionally bounded, and multi-node scheduling
is deferred rather than implied.
