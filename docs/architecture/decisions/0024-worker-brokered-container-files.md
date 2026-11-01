# ADR 0024: Broker container file access through the worker

## Status

Accepted

## Context

Depot users need to inspect an active service container and move files between
their browser and that container. The API process deliberately has no Docker
group membership, and exposing either the Docker socket or the worker's Unix
socket to the browser would collapse that privilege boundary.

Container images also cannot be assumed to contain a particular language
runtime or file-management service. Depot already requires a POSIX shell for
its interactive shell feature, so a small fixed set of non-interactive shell
operations is the narrowest compatible implementation.

## Decision

Extend the signed, short-lived worker capability used by the browser shell with
an explicit operation and operation arguments. The HMAC covers the operation,
paths, byte count, active project, container, actor, expiry, and nonce. The
worker rechecks that the selected container is still the project's active
deployment before executing anything.

The API exposes authenticated file endpoints for directory listing, bounded
single-file upload/download, directory creation, rename, and guarded recursive
deletion. It validates canonical absolute container paths, applies CSRF checks
to mutations, limits payload and response sizes, and proxies requests through
the group-restricted Unix socket. It never accepts a Docker command or shell
program from the browser.

The worker invokes only fixed commands and fixed shell programs. User paths are
passed as positional arguments, never interpolated into shell source. Directory
list records are NUL-delimited so whitespace and control characters in names do
not change command structure. Root deletion and rename are forbidden.

## Consequences

- The API service retains no Docker privileges and the browser never sees a
  worker capability or local socket.
- File operations are scoped to the currently active service container; static
  releases and stopped deployments do not expose a file browser.
- Uploads are intentionally bounded and target one file at a time. This is an
  operator convenience, not a general volume synchronization protocol.
- Operations depend on the container providing `sh` and basic POSIX utilities,
  matching the existing browser-shell compatibility boundary. Minimal `scratch`
  images remain inspectable only through logs and metadata.
- Changes affect the mutable running container and disappear when a new release
  replaces it unless the application writes to externally persisted storage.
