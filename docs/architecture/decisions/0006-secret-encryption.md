# ADR 0006: Encrypt all project runtime values with an external master key

- Status: accepted
- Date: 2026-10-31

## Context

Runtime variables include credentials and ordinary configuration. SQLite
backups must not be sufficient to recover their values, and secret material
must not appear in API reads, process arguments, or deployment logs.

## Decision

Encrypt every project runtime value using XChaCha20-Poly1305 with a fresh
192-bit nonce. Bind ciphertext to its project ID and variable name as
authenticated additional data, preventing rows from being silently moved or
renamed. Store the 256-bit master key as a 64-character hexadecimal value in a
root/operator-managed file outside SQLite.

API reads return names, classification, and timestamps only. Values are
write-only after submission. The worker decrypts immediately before container
creation and supplies an env-file payload to Docker over stdin, never as
command-line arguments or build inputs.

## Consequences

SQLite backups do not reveal values without the separate key. Losing the key
makes all stored values unrecoverable, so it must be backed up separately and
restored before the database. Rotation requires a future transactional
re-encryption command; replacing the file alone is not rotation. Operators can
still inspect container environments through privileged Docker access, which
is an acknowledged runtime limitation.
