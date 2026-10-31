# ADR 0001: Use a modular Rust monolith with one distributable binary

- Status: accepted
- Date: 2026-10-31

## Context

Depot needs separate API, worker, gateway, migration, setup, doctor, and host
Caddy-agent operating modes, but production must receive one executable. A large
single crate would make security boundaries and infrastructure integrations hard
to review, while separately distributed services would violate packaging goals.

## Decision

Use a Cargo workspace of capability-focused library crates and one `depot` binary
crate. The binary owns configuration and process lifecycle, then invokes library
entry points selected by subcommand. React assets are compiled before the final
Rust build and embedded in that executable.

The initial slice creates only crates needed by working behavior. Further crates
are introduced when their capability is implemented, avoiding empty abstractions.

## Consequences

Process modes can be isolated with systemd while sharing versioning and release
artifacts. Capability boundaries remain explicit and testable. Compile time and
dependency coordination are higher than for one crate, but operational packaging
stays simple.
