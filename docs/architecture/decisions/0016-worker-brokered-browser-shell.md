# ADR 0016: Broker browser shells through the worker

## Status

Accepted

## Context

Project operators need to inspect an active application from the project canvas.
The public API intentionally does not belong to the Docker group, so directly
starting `docker exec` from an HTTP handler would collapse an important privilege
boundary.

## Decision

The authenticated API upgrades a same-origin request to WebSocket only after it
resolves the project's active deployment. It creates a short-lived capability
binding the user, project, session, and exact active container, records an audit
event, signs the capability with the instance key, and proxies bytes over a
group-restricted Unix socket.

The worker owns that socket, verifies the signature and expiry, independently
checks that the container remains the project's active deployment, and starts a
non-TTY interactive `sh` process through Docker. Sessions have a fifteen-minute
hard limit, terminate on browser disconnect, accept bounded messages, and share
a small global concurrency limit.

## Consequences

- The browser gets a persistent shell without granting Docker access to the API.
- Full-screen TTY applications are not supported in the first version because
  the broker is a line-oriented byte stream rather than a pseudo-terminal.
- A worker restart closes active terminals without affecting deployments.
- Shell content is streamed and not retained in SQLite; session creation is
  retained in the audit log without command or output contents.
