# ADR 0029: Explicit runtime-variable reveal

## Status

Accepted

## Context

Depot encrypts every runtime environment value and omits values from normal list responses. That
prevents accidental disclosure, but operators cannot verify a saved value before rotating it.

## Decision

Saved values remain absent from project and variable-list responses. An authenticated operator may
explicitly reveal one value through a CSRF-protected POST endpoint. Depot decrypts only that record,
marks the response `Cache-Control: no-store`, and records the variable name and actor in the audit
log without recording the value.

The web client retains a revealed value only in mounted component memory. Hiding it removes that
value from UI state, and leaving the project destroys the remaining state. Reveal responses are not
placed in the query cache or browser storage.

## Consequences

- Operators can inspect both secret and non-secret runtime values deliberately.
- Normal reads, logs, errors, and audit metadata remain value-free.
- Anyone with project access can already replace or execute with these values and may now reveal
  them explicitly; future project-level roles may narrow this permission further.
