# ADR 0023: Make the Caddy agent domain policy configurable

## Context

The Caddy agent required at least one allowed domain suffix. This is useful when
an operator delegates one DNS zone to Depot, but it prevents a personal Depot
instance from serving unrelated domains without editing and restarting the host
agent for every new zone.

The agent already authenticates reconciliation requests, accepts only validated
public hostnames, writes only its isolated managed file, and routes every domain
to one operator-configured upstream. The mandatory suffix restriction is
therefore an optional hardening policy rather than the core trust boundary.

## Decision

Add an explicit `allow_any_domain` boolean to the agent configuration. When it
is true, every hostname must still pass Depot's strict public-hostname
validation, but it need not match a configured suffix. When it is false, the
existing non-empty suffix allowlist remains mandatory and unchanged.

Existing configurations default to `allow_any_domain = false`. New guided
setups default to unrestricted valid domains and offer restricted suffix mode as
an explicit alternative.

## Consequences

- Operators can attach unrelated valid domains without host configuration work.
- Hardened installations can continue limiting Depot to delegated DNS zones.
- Unrestricted mode lets a compromised authenticated control plane request
  certificates and routes for any valid hostname pointing at the Caddy host; the
  fixed upstream and isolated managed file remain enforced.
