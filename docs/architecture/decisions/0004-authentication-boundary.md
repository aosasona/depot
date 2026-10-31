# ADR 0004: Use opaque server-side sessions and origin-bound mutations

- Status: accepted
- Date: 2026-10-31

## Context

Depot is an administrative control plane exposed behind Caddy. Browser tokens
must be revocable, roles must be enforced by the server, and first-run setup must
not leave a permanent unauthenticated registration endpoint.

## Decision

Hash passwords with Argon2id. Authenticate browsers using a random opaque token in
an HttpOnly, Secure, SameSite cookie; persist only a hash of the token. Require a
matching CSRF token and a validated Origin for state-changing browser requests.
Bootstrap requires a time-limited token created explicitly by the operator and is
disabled transactionally when the first admin is committed. Authorization remains
in backend middleware and handlers, independent of UI visibility.

## Consequences

Sessions can be individually revoked and stolen database contents do not reveal
live bearer tokens. Operators must configure the public origin and TLS correctly,
and CLI recovery remains necessary when all administrators are locked out.
