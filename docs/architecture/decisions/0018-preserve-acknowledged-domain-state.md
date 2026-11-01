# ADR 0018: Preserve acknowledged domain state after failed reconciliation

## Status

Accepted

## Context

The Caddy agent applies a complete desired-domain snapshot and restores its
previous valid configuration when validation or reload fails. Depot nevertheless
marked every stored domain as `error` after any failed attempt. Existing routes
therefore appeared broken even though Caddy was intentionally still serving
them.

## Decision

A failed snapshot only moves unacknowledged domains to `error`. Domains already
acknowledged as `active` retain that state because the agent preserves its last
valid configuration. The domain UI labels errors as synchronization failures,
shows the bounded agent error, polls while reconciliation is outstanding, and
offers an authenticated retry action.

A successful complete snapshot continues to mark every desired domain active
and clear its previous error.

## Consequences

- The dashboard no longer equates control-plane synchronization with public
  reachability.
- A newly attached route still reports a real failure until acknowledged.
- Removal failures remain represented by the durable reconciliation job because
  the removed domain no longer has a database row.
