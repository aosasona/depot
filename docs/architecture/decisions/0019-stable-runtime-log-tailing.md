# ADR 0019: Preserve runtime logs across polling refreshes

## Status

Accepted

## Context

Runtime logs are collected by short-lived worker jobs because the API process
does not have Docker access. Every polling cycle therefore has a brief queued or
running state. Replacing the current output with that intermediate state causes
visible flicker and resets the operator's scroll position.

## Decision

The browser retains the last completed runtime-log snapshot while the next
worker job runs. The console initially opens at the newest line and follows new
snapshots only while the viewer remains near the bottom. Scrolling upward pauses
following; returning to the bottom resumes it.

Initial connection and unrecoverable error states remain visible when no
successful snapshot exists. A refresh failure does not discard already visible
output.

## Consequences

- Polling no longer interrupts log reading or flashes an empty state.
- Operators can inspect older lines without being pulled back down.
- Runtime output is still bounded to the latest worker-provided snapshot rather
  than becoming an unbounded browser buffer.
