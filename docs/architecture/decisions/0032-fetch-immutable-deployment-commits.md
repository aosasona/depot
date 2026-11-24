# ADR 0032: Fetch immutable deployment commits directly

## Status

Accepted

## Context

Inspection and GitHub webhooks record an immutable commit SHA for each
deployment. The worker nevertheless deployed by shallow-cloning the configured
branch head and comparing that moving head with the recorded SHA. A newer push
between job creation and checkout therefore made a valid older deployment fail
with `checked out repository did not resolve to a valid commit SHA`.

Rapid pushes and queued jobs make this race normal rather than exceptional.

## Decision

Deployment checkout initializes an isolated repository, fetches the exact
validated 40-character commit SHA with depth one, checks out `FETCH_HEAD` in a
detached state, and verifies `HEAD` still equals the recorded SHA. Repository
inspection continues to resolve the selected branch, but deployment never
re-resolves that moving branch.

Authentication remains ephemeral and attached only to the fetch command. The
repository URL and expected SHA continue through the existing validation
boundary, and Git commands are invoked without a shell.

## Consequences

- A newer push cannot invalidate an already queued deployment.
- Concurrent webhook jobs build the commits they actually recorded.
- Force-pushed commits can only deploy while the remote still permits fetching
  that reachable object; otherwise Depot reports that the recorded commit is
  unavailable or inaccessible.
- Docker cache state is unrelated to source commit resolution.
