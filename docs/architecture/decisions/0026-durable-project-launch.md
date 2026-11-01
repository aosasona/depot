# ADR 0026: Make first deployment a durable project launch

## Status

Accepted

## Context

Creating a project previously stopped after source and build settings were
saved. Users then had to open the project, inspect the repository, wait for the
inspection, and manually deploy its resolved commit. A browser-only chain for
those operations would be lost if the dialog closed or the connection dropped.

Project creation also lacked a practical way to paste an existing `.env` block.
Entering one encrypted value at a time made initial configuration unnecessarily
slow.

## Decision

The creation wizard accepts a bounded dotenv-style block, validates every entry
before creating the project, encrypts each value through the existing variable
API, and offers “create and deploy latest” as the default action.

Add an authenticated launch endpoint that enqueues repository inspection with a
signed-in actor and a durable `deploy_after_inspection` intent in the job
payload. After resolving the branch to an immutable commit, the worker creates
an idempotent creator-initiated deployment for that commit. Retried inspection
jobs reuse the existing manual deployment instead of creating duplicates.

The browser navigates to the new project's deployment view as soon as the
launch job is queued. Build progress and failures remain visible through the
normal durable job and deployment interfaces.

## Consequences

- Closing the creation dialog or browser does not cancel the first deployment.
- The first build always targets the exact commit inspected by Depot, rather
  than a mutable branch name guessed by the browser.
- Environment values remain write-only and encrypted; the textarea exists only
  in browser memory until each validated value is submitted.
- Project, build configuration, and environment writes are still separate API
  operations. If one fails, the project remains available for correction and
  relaunch rather than being deleted automatically.
