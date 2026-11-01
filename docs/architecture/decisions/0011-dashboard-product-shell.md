# ADR 0011: Product-oriented dashboard shell

## Status

Accepted

## Context

The authenticated interface presents operational controls beneath a large
marketing hero. Projects, deployments, configuration, domains, and users are
stacked into one long page. This makes the product feel like a prototype and
leaves no stable place for future service topology, templates, logs, or account
settings.

## Decision

Depot will use a persistent workspace shell with compact global navigation, a
context header, and separate top-level views. Projects are the default working
surface; administrator-only access management is a distinct Team view. Visual
styling will use restrained neutral surfaces, one violet accent, consistent
spacing and radii, and status color only where it carries operational meaning.

The first redesign phase changes the shell and hierarchy without hiding or
rewriting existing deployment controls. Later phases will split project detail
into overview, deployments, logs, variables, domains, and settings, and add the
connected-service canvas and templates on top of that structure.

## Consequences

- Routine work starts in a product workspace rather than a landing-page hero.
- Navigation can grow without adding another full-width panel to one page.
- Existing capabilities remain available during incremental redesign.
- URL-backed nested project routes remain follow-up work; the initial shell
  keeps view selection local to avoid coupling visual work to API changes.
