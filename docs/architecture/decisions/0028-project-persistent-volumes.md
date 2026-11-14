# ADR 0028: Project-scoped persistent Docker volumes

## Status

Accepted

## Context

Depot replaces a service container on each successful deployment. Files written only to a
container layer therefore disappear on redeployment, including embedded SQLite databases and
uploaded files. Allowing arbitrary host bind mounts would expose the worker filesystem and make
validation, installation, and backups harder.

Dockerfiles can declare persistent paths with `VOLUME`. Railpack and other image builders may also
emit OCI image volume metadata, but source inspection cannot reliably infer every application's
writable data directory.

## Decision

Depot stores a bounded list of absolute container paths in each service configuration. Each path
is mounted from a deterministic, project-scoped Docker named volume. The name is derived from the
project identifier and a hash of the container path, so every deployment of the same project and
path reuses the same data while cloned projects remain isolated.

Repository inspection suggests final-stage Dockerfile `VOLUME` paths. Deployment also inspects the
built image and mounts declared OCI volumes, covering builders such as Railpack when their output
contains volume metadata. Explicit user configuration is merged with image declarations and takes
effect on the next deployment. Users can add an existing directory directly from the live
container file browser or enter paths in service settings. On the first stateful cutover, Depot
stops the predecessor and seeds each new managed volume with that directory's existing contents
before starting the replacement. Static projects do not receive runtime volumes.

Depot never accepts a host source path from project configuration. Volume removal is not coupled
to project or release deletion; destructive cleanup must be an explicit future operation.

## Consequences

- SQLite files, uploads, and similar state persist across container replacement when stored under a
  configured or declared path.
- Operators must include Docker named volumes in backups in addition to Depot's SQLite database,
  master key, and static artifacts.
- Applications without Dockerfile or OCI volume declarations require the user to enter their data
  paths.
- Changing a mount path creates a different volume and does not migrate existing data automatically.
