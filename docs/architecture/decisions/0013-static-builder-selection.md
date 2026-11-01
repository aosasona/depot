# ADR 0013: Select a real build engine for static releases

## Status

Accepted

## Context

The first static release adapter always generated a Node builder and rejected a
repository without an npm, pnpm, or Yarn lockfile. It also rejected Railpack
before reaching the static path and ignored a configured Dockerfile. That made
ordinary React repositories and static-server images such as Chimney appear
unsupported even though Depot can promote their files without keeping the
container running.

## Decision

Static deployments resolve their engine as follows:

- `dockerfile` builds the configured Dockerfile and extracts the configured
  output path from the final image. The path is written without a leading slash,
  for example `var/www/html/site` for a Chimney image.
- `railpack` asks the official Railpack CLI to load a uniquely tagged image into
  Docker, then copies only the configured output directory from the image's `/`,
  `/app`, or `/workspace` root. Depot removes the temporary image afterward.
- `auto` prefers a configured Dockerfile that exists, then uses the contained
  Node builder. Node projects with a lockfile use frozen installs; a
  `package.json` without a lockfile uses `npm install` as a compatibility
  fallback.

All paths still validate the extracted tree, reject links and special files,
apply file/size bounds, and promote through immutable release directories.
Railpack uses the operator-installed `railpack` binary and a dedicated
`depot-buildkit` container created by guided setup.

Filesystem `--output` mode is deliberately avoided. It exports the entire
runtime root and asks BuildKit to recreate ownership and mode metadata that an
unprivileged Depot worker cannot safely apply. Image loading keeps those
operations inside Docker and exposes only the validated static artifact tree to
Depot's filesystem.

## Consequences

- Static build behavior now follows the saved builder selection.
- Chimney remains a build artifact source, not a permanently running static
  container.
- Lockfiles remain preferred for reproducibility, while simple frontend
  repositories no longer fail before their build starts.
- Railpack and BuildKit become checked installation dependencies.
