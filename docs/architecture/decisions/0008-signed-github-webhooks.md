# ADR 0008: Signed and idempotent GitHub push webhooks

- Status: accepted
- Date: 2026-10-31

## Context

Automatic deployments need to authenticate an internet-originated request,
select an immutable revision, reject unrelated repository activity, and remain
safe when GitHub retries a delivery.

## Decision

Give each project an unguessable endpoint token and a separate random signing
secret. Store only the endpoint token digest and encrypt the signing secret
with the instance master key. Verify the GitHub HMAC-SHA-256 signature against
the raw, size-limited request body using constant-time comparison before JSON
parsing.

Accept only push events for the configured `owner/repository` and exact project
branch, and enqueue the event's full commit SHA. Insert the GitHub delivery ID
and deployment in one SQLite transaction. A duplicate delivery returns an
accepted response without creating another job.

## Consequences

Operators must copy each signing secret when it is issued, and rotating a
webhook requires updating GitHub immediately. Failed verification intentionally
does not reveal which check failed. Repository transfers or renames require a
rotation with the new full name.
