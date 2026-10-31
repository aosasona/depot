# ADR 0007: Explicit transactional release activation

- Status: accepted
- Date: 2026-10-31

## Context

Choosing the newest successful deployment at request time allows an older,
slower build to become live after a newer deployment was requested. Runtime
side effects also cannot be atomically committed with SQLite, so switching and
container cleanup need a deliberate order and restart recovery.

## Decision

Persist exactly one `active` successful deployment per project, enforced by a
partial unique index. A worker builds a candidate and waits for readiness before
entering the completion transaction. That transaction completes the job,
rejects activation when any newer deployment request exists, deactivates the
previous release, and activates the candidate as one atomic database change.
The gateway reads only this explicit active row.

After commit, the worker removes the switched-out container and records its
retirement. Worker startup retries cleanup for inactive terminal containers
left behind by a process interruption. Cleanup failure never rolls traffic back
to an unhealthy or stale candidate.

Rollback creates a fresh deployment of a retained successful commit rather
than reactivating a stopped container. It therefore repeats checkout, build,
runtime injection, and readiness verification before promotion.

## Consequences

Failed and stale builds preserve the existing active release. There can be a
short period after promotion where the old container still runs but receives no
new gateway traffic. Deployment history is retained, while inactive runtime
containers are not. Depot retains the active and four most recent successful
service images per project; older inactive images are removed with restart
recovery. Making the retention count configurable remains follow-up work.
