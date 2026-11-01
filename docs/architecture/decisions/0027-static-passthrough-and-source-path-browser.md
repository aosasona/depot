# ADR 0027: Publish plain static trees and browse GitHub build paths

## Status

Accepted

## Context

Depot's static pipeline supported Node, Dockerfile, and Railpack builds, but a
repository containing already-built HTML/CSS/JavaScript had no valid builder.
Using `.` as its output directory reached a stricter adapter check and produced
a misleading Docker path-containment error.

Build configuration also required users to type repository-relative context and
Dockerfile paths from memory even when the project was imported through Depot's
GitHub App.

## Decision

When static builder selection is automatic, treat a committed directory
containing `index.html` as a no-build static artifact. Copy regular files and
directories directly into isolated release staging, reject symbolic links and
special files, omit Git metadata, apply the existing artifact size/count
limits, and promote through the same immutable static-release path.

Expose the persisted GitHub project identity through an authenticated read-only
endpoint. Reuse the installation-token-backed repository directory browser in
Build Configuration to select directory fields and the Dockerfile. Generated
output directories remain editable text because they commonly do not exist
until the build runs; the browser may still select an already committed output
directory.

Manual Git and SSH projects retain typed paths because Depot has no provider
credential for browsing them before checkout.

## Consequences

- Plain HTML sites deploy without Docker, Node, Railpack, or a synthetic build.
- `.` is a valid static output root while parent traversal and absolute paths
  remain forbidden.
- GitHub-imported projects get visual path selection without exposing an
  installation token to the browser.
- Repository browsing reflects the configured branch, while generated build
  artifacts must still be entered by their expected relative path.
