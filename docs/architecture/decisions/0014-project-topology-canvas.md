# ADR 0014: Scope the service canvas to one project

## Status

Accepted

## Context

The first canvas visualized the workspace's Caddy-to-domain routing. The useful
mental model is instead one deployable project and the resources that application
uses: its active container, database, Redis or Valkey, object storage, and public
domains. Operators also need direct actions from those nodes.

## Decision

Each project has a URL-backed `canvas` tab. Its application node is derived from
the project and active deployment. External resource nodes are derived from
masked environment-variable metadata, grouped by database, Redis/Valkey, and
object-storage conventions. Domain nodes use persisted routing state.

The node context menu provides:

- Inspect, using only non-secret metadata.
- Clone for applications, copying source and service configuration but never
  secrets, domains, deployments, or runtime identity.
- Clone for external connections, which requires a new write-only secret.
- Shell for an active application, which provides the exact `docker exec`
  command to run on the Depot VM.

The public API process remains outside the Docker group. We will not add a web
shell by granting that process Docker access. A future browser terminal requires
an authenticated, audited worker control channel with explicit command and
session limits.

## Consequences

- The canvas reflects real project state without introducing fictional managed
  databases.
- Adding a connection writes an encrypted environment value through the existing
  secret boundary and immediately adds its node.
- Shell access is useful now from the host while preserving the API/worker
  privilege separation.
