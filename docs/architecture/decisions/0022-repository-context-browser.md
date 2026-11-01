# ADR 0022: Browse GitHub directories when choosing a build context

## Context

Depot persists a repository-relative build context, but project creation always
saved `.`. Monorepo users therefore had to create the project first and type a
path from memory. Private GitHub repository contents cannot be fetched safely by
the browser because its installation credential must remain server-side.

The Templates page also included one application-specific blueprint. That
example aged independently of the application and obscured the reusable workflow
for any service that depends on databases, caches, or object storage.

## Decision

The GitHub App client will expose an authenticated, read-only directory listing
through Depot's API. The project wizard will use it as a lazy folder browser and
persist the selected directory as the service configuration's `context_path`.
The API verifies the linked installation and repository before every listing,
and accepts only bounded repository-relative paths.

Manual Git URLs retain a typed context field because Depot has no provider
credential with which to inspect those repositories before checkout.

The application-specific blueprint will be replaced by a provider-neutral guide.
It explains how to deploy the application first, connect external dependencies
through the canvas, and let those connections create encrypted runtime
environment entries.

## Consequences

- Monorepo source selection happens before the first build and works for private
  GitHub repositories without exposing tokens.
- Directory requests are lazy, so very large repositories do not require a
  recursive tree response.
- Git submodules and non-GitHub manual repositories are not browsable in this
  flow; their context remains directly editable.
- Dependency provisioning remains operator-managed, while environment wiring is
  handled consistently through Depot's existing encrypted variable workflow.
