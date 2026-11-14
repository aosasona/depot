# ADR 0030: Root-owned dashboard update boundary

## Status

Accepted

## Context

Depot administrators should be able to install an official release without SSH access. The API and
worker are intentionally unprivileged and must not receive general filesystem, package-manager, or
systemd control.

## Decision

The API may create one fixed request file in `/var/lib/depot/updater/inbox`, an inbox owned by the
`depot` API user and inaccessible to the worker. Its parent state directory and all helper output
remain root-owned, preventing symlink replacement across the privilege boundary. A root-owned
systemd path unit observes only the request file and starts a fixed-purpose oneshot helper. Neither the browser nor API can supply a command,
URL, release tag, path, or shell input.

The helper invokes only `depot-setup update --role vm --yes`. That existing workflow downloads from
the compiled official repository default, verifies `SHA256SUMS`, performs dependency preflight,
stops services, creates a timestamped backup, installs and validates the release, restarts API and
worker, and verifies the Caddy agent. If that workflow fails after making a new backup, the helper
restores the previous binary, configuration, key, and SQLite files before restarting Depot. Progress and the final result live in the updater directory so
they survive the API restart. Output is retained in a root-produced log for operator diagnosis but
is not returned by the API.

Only administrators can read updater state or create a request. Requests require session-bound CSRF
and are recorded in the audit log. Atomic request creation prevents concurrent updates.

## Consequences

- Existing installations require one manual package update to install and enable the helper.
- A compromised API account with administrator access can cause controlled update downtime, but
  cannot choose an artifact or execute arbitrary privileged commands.
- Dashboard updates automatically restore their immediately preceding backup on failure; the backup
  remains available for manual diagnosis and recovery.
