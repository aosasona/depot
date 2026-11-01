# ADR 0017: Give deployments canonical detail routes

## Status

Accepted

## Context

Deployment history and retained build output are related, but the dashboard
currently separates them into different tabs. Finding the output for a specific
attempt requires remembering its commit and selecting it again in the logs
view. The selection is also lost on refresh and cannot be shared.

## Decision

Each deployment links to
`/projects/:projectId/deployments/:deploymentId`. The route keeps the project
Deployments tab active and renders the retained events and build output for that
exact deployment. The general Logs tab remains available for active runtime
output and browsing all build attempts.

The deployment ID lives in the URL rather than component-only state. An unknown
deployment ID produces an explicit not-found state within the project instead
of silently selecting another deployment.

## Consequences

- One click from deployment history opens the relevant output.
- Deployment logs survive refresh and support normal browser navigation and
  shareable links.
- No API change is required because deployment history already exposes the job
  identifier used to load retained output.
