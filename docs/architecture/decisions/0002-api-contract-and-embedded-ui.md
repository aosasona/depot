# ADR 0002: Generate the frontend contract and embed immutable UI assets

- Status: accepted
- Date: 2026-10-31

## Context

The TypeScript client must not drift from Rust API behavior, and production must
not need Node.js or a frontend asset directory.

## Decision

Rust DTOs define the HTTP contract and produce an OpenAPI document during a
checked generation step. TypeScript API types are generated from that document.
The Vite build emits hashed assets which the Rust binary embeds at compile time.
API, webhook, and event routes are registered before an explicit SPA fallback.
Hashed files receive immutable cache headers; the HTML shell does not.

## Consequences

Contract drift becomes a CI-visible diff and production installation remains a
single file. Rust release builds require a completed frontend build, while normal
backend test builds use a checked-in minimal generated asset set when appropriate.
