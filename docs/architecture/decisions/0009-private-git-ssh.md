# ADR 0009: Project-scoped private Git SSH credentials

- Status: accepted
- Date: 2026-10-31

## Context

Private source checkout requires a credential without leaking it through API
reads, process arguments, logs, Git prompts, or durable plaintext files. SSH
also needs explicit server identity verification to prevent first-use trust from
becoming a network interception path.

## Decision

Accept only project-scoped private keys for repositories using explicit
`ssh://git@host/path` URLs. Encrypt the private key with the instance master key
and project-bound associated data. Treat the private key as write-only in the
API. Store operator-supplied OpenSSH `known_hosts` data beside it.

The worker decrypts the key immediately before a Git operation and zeroizes the
in-memory string afterward. It creates a mode-0700 temporary directory with
mode-0600 identity and known-hosts files. A fixed wrapper invokes SSH with
batch mode, only the supplied identity, and strict host-key checking. The guard
removes all temporary files when the clone finishes or fails.

## Consequences

Operators must source host keys through a trusted channel and rotate deploy
keys explicitly. Passphrase-protected keys and private HTTPS credentials are
not supported. A Git host-key rotation intentionally breaks checkout until the
operator updates the trusted key.
