# Depot v0.1 implementation plan

Depot will be delivered in independently testable milestones, with a complete
vertical slice kept working as each capability is added.

## Milestone A — foundation and authenticated product shell

1. Create the Rust workspace, frontend workspace, quality tooling, and CI.
2. Establish SQLite migrations, configuration loading, and the single `depot`
   executable with embedded frontend assets.
3. Implement secure first-run bootstrap, local sessions, role enforcement, and
   admin user management.
4. Build the responsive near-black/lime application shell and typed API client.
5. Verify formatting, linting, tests, a release build, and a binary smoke test.

## Milestone B — first service deployment

1. Add project configuration and repository inspection at immutable commits.
2. Add durable jobs, build detection, Dockerfile/Railpack orchestration, and
   incremental deployment events.
3. Add OCI lifecycle management, readiness-first promotion, and gateway routes.
4. Demonstrate a real public repository deployment through the private gateway.

## Milestone C — static sites and host HTTPS

1. Add isolated static builds, verified artifact extraction, and immutable
   release promotion through the shared static server.
2. Add domain reconciliation and the restricted `caddy-agent` process mode.
3. Verify generated Caddy configuration and document DNS/ACME prerequisites.

## Milestone D — automatic deployment and reliability

1. Add protected SSH deploy credentials. Signed, idempotent GitHub webhooks are
   complete.
2. Add encrypted variables/secrets, rollback, runtime logs, reconciliation, and
   release retention.
3. Verify stale and failed deployments cannot replace the healthy release.

## Milestone E — release hardening

1. Complete security, accessibility, integration, and end-to-end coverage.
2. Complete installation, upgrade, backup, recovery, and operational guides.
3. Exercise the clean-install acceptance scenario and publish honest limits for
   infrastructure that cannot be live-tested in the development environment.

## Immediate vertical slice

The first slice is: initialize an empty instance, create the first administrator,
sign in using a server-side session, load the authenticated dashboard, create a
member through an admin-only endpoint, and run it all from one Rust binary with
embedded production UI assets.
