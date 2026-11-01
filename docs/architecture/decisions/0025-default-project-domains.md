# ADR 0025: Attach configured default domains during project creation

## Status

Accepted

## Context

Every new Depot project normally needs a public hostname, but operators had to
attach one manually after creating the project. Depot already requires a stable,
unique project slug, making that slug the least surprising default subdomain.

The default DNS zone is installation-specific and must remain optional so
existing installations and operators using unrelated custom domains continue to
work unchanged.

## Decision

Add an optional `domains.default_suffix` setting to Depot's main configuration.
The guided VM setup asks for it and validates it as a public DNS hostname. When
configured, both manual and GitHub project creation atomically create the
project and `<project-slug>.<default_suffix>` domain. Depot then queues the same
full-snapshot Caddy reconciliation used for manually attached domains.

The suffix is not a Caddy-agent allowlist. Operators may still attach other
valid hostnames according to the agent's independent domain policy.

## Consequences

- New projects are addressable with a predictable hostname without a second
  setup step.
- A conflicting generated hostname prevents project creation instead of leaving
  a partially configured project.
- Existing configurations omit the setting and retain the previous behavior.
- DNS records and wildcard DNS remain operator responsibilities; Depot manages
  the Caddy route only.
