# ADR 0020: Present templates as an operational catalog

## Status

Accepted

## Context

The Templates page inherited card and primary-action rules from the original
authentication prototype. Its actions, feature tags, spacing, and connected-app
diagram did not match the denser dashboard shell and broke down at intermediate
viewport widths.

## Decision

Templates use a compact catalog treatment with explicit support state, numbered
entries, row-based capabilities, and consistently sized actions. Connected-app
blueprints use a separate dependency-plan surface that distinguishes the Depot-
managed application from operator-provided services.

All geometry remains square, neutral surfaces carry the hierarchy, and lime is
reserved for supported state, connectors, and actions. The layout collapses to
one column before content becomes cramped.

## Consequences

- The page visually belongs to the same operational workspace as projects and
  deployments.
- Supported templates and operator-assisted blueprints are easier to scan.
- Future templates can reuse the catalog structure without inventing new card
  styling.
