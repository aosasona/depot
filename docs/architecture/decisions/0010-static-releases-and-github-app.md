# ADR 0010: Immutable static releases and GitHub App source integration

## Status

Accepted

## Context

Depot can build and run a Docker service and receive a manually configured
signed GitHub webhook. Static projects still fail at deployment, retain no
immutable artifact, and cannot be served by the shared gateway. GitHub projects
must be created from pasted URLs and operators must copy webhook credentials by
hand.

Static output is untrusted build material. It must not escape its configured
directory through absolute paths or symlinks, and a failed release must never
replace the active site. GitHub App credentials grant repository access and
must not be stored in SQLite.

## Decision

Depot will build static Node projects in a generated disposable Docker builder
for the detected package manager. Its final image contains only an `/output`
artifact tree. Depot copies that tree into a same-filesystem staging directory,
rejects unsafe entries, and atomically renames it to an immutable release under
the configured static root. The gateway serves the active release directly,
including an `index.html` fallback for client-side routes. Static sites have no
persistent application container.

GitHub integration will use an operator-configured GitHub App ID, slug,
private-key file, and webhook secret file. Depot signs short-lived app JWTs,
exchanges them for installation tokens, links installations, lists and searches
their repositories, and creates a project directly from a selected repository.
The GitHub App's global push webhook feeds Depot's existing signed, idempotent
deployment path. Installation tokens and plaintext webhook secrets remain
transient. Manual Git URLs and project webhooks remain advanced fallbacks.

## Consequences

- Static Node projects gain a container-free runtime and atomic promotion.
- The generated builder initially supports npm, pnpm, and Yarn lockfiles;
  unsupported ecosystems fail explicitly rather than running an arbitrary host
  command.
- GitHub App setup is an operator task, but routine project creation becomes
  repository selection rather than credential and webhook plumbing.
- Private GitHub checkout will use short-lived installation credentials without
  storing tokens or exposing them on command lines.
