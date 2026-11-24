# ADR 0031: Duplicate project configuration without runtime state

## Status

Accepted

## Context

A project can accumulate unusable deployment or Docker runtime state while its
source and configuration remain valuable. Recreating its build settings,
environment, and persistent paths by hand is slow and error-prone. Copying
deployment identities or project-scoped Docker volume names would reproduce the
same corruption and could attach the new project to old runtime data.

Encrypted environment values are authenticated against their project ID and
variable name, so their stored ciphertext cannot safely be copied between
projects.

## Decision

Add one authenticated, CSRF-protected duplication endpoint. The API decrypts
each source variable only inside the trusted secret boundary, immediately
re-encrypts it against the new project ID, and zeroizes the plaintext. The store
then creates the project, copies the service configuration, inserts the newly
encrypted variables, creates an optional default domain, and records an audit
event in one SQLite transaction.

The duplicate receives a fresh project ID and therefore fresh Docker volume,
deployment, and runtime identities. Deployments, containers, images,
logs, custom domains, SSH credentials, deploy tokens, and webhook credentials
are not copied. GitHub App repository ownership is not copied because the
current integration maps one linked repository to one project; the duplicate
retains the repository clone URL and branch as a normal Git source.

## Consequences

- A broken project can be replaced without re-entering build settings or
  environment values.
- Existing named-volume data is deliberately not attached to the duplicate.
- The operation cannot leave a partially copied project if encryption or a
  database write fails.
- Private GitHub App repositories need to be re-linked through a future
  multi-project repository integration before the duplicate can deploy.
