# ADR 0012: Complete the project workspace in vertical slices

## Status

Accepted

## Context

The URL-backed dashboard has a stable project workspace, but creating a project
still exposes provider details inline, the overview repeats source metadata,
runtime logs and project lifecycle controls are absent, and connected services
have no visual model. Depot also needs templates without implying that it can
provision managed databases that remain outside the v0.1 scope.

## Decision

We will complete the workspace as independently releasable vertical slices:

1. A keyboard-accessible creation dialog will make GitHub the primary path and
   keep a manual repository URL as an explicit alternative. The final step
   captures project kind, builder, output directory, and service port so a new
   project starts with a usable service configuration.
2. The overview will compose existing project, configuration, deployment, and
   domain APIs. This avoids a premature aggregate endpoint while the data set is
   small.
3. Project settings will expose the existing update and guarded delete API.
4. Runtime logs will be a bounded, authenticated read from the active container;
   static projects will explain that they do not have a runtime process.
5. A service canvas will render persisted project relationships. Templates are
   declarative blueprints: they may configure Depot projects and document
   required external services, but will not claim to provision Postgres, Redis,
   object storage, or Docker Compose.
6. Shared dialogs, notifications, loading states, mobile navigation, and focus
   treatment will be tightened as part of each slice instead of postponed into
   a disconnected visual pass.

## Consequences

- Project creation becomes one guided flow and can apply configuration directly
  after import.
- Overview data may issue three small parallel requests; a server aggregate can
  replace them later without changing the page contract.
- Dependency-heavy apps can be represented honestly with operator-provided
  services, but one-click dependency provisioning remains future work.
- Runtime log access is deliberately bounded and never accepts arbitrary
  container identifiers from the browser.
