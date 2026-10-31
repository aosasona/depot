# ADR 0005: Install one binary as isolated systemd services

- Status: accepted
- Date: 2026-10-31

## Context

Depot ships one executable but has three operational roles with different
privileges. The API/gateway needs the SQLite data directory and a private TCP
listener. The worker additionally needs Git, workspace storage, and access to
the local Docker daemon. The Caddy agent runs on a different host and needs
write access only to Depot's generated Caddy directory.

## Decision

Install the same `/usr/local/bin/depot` binary into separate systemd units:

- `depot.service` runs `depot serve` as the unprivileged `depot` account;
- `depot-worker.service` runs `depot worker` as a separate `depot-worker`
  account. It shares the `depot` data group and its Docker group membership is
  an explicit host-level trust decision;
- `depot-caddy-agent.service` runs on the Caddy host as `depot-caddy`, with the
  host's `caddy` group and only the generated configuration directory writable.

The installer has distinct `vm` and `caddy-agent` modes. It does not edit the
operator-owned Caddyfile, firewall, Docker policy, or configuration secrets.
Those remain deliberate manual steps. A `DESTDIR` staging mode installs files
without creating users or invoking systemd, making the packaging path testable
without root access.

## Consequences

The API cannot invoke Docker directly, and compromise of its process does not
automatically grant Docker access. The worker still has the effective host
privilege inherent in Docker daemon access; this residual risk is documented.
The Caddy agent cannot modify the main Caddyfile through its normal filesystem
permissions. Operators must complete configuration, group policy, firewall,
and service enablement after reviewing generated files.
