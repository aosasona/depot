# ADR 0015: Bundle dashboard typography and use live system status

## Status

Accepted

## Context

The dashboard depended on platform fonts, which changed its hierarchy and line
metrics between operator machines. Its shell also repeated headings and showed
an unconditional healthy status even when readiness had not been checked.

## Decision

Bundle variable Space Grotesk and JetBrains Mono assets into the existing Vite
build and embedded binary. Use the former for display and interface text and the
latter for operational metadata, hashes, commands, and navigation labels.

Keep the approved square black-and-lime direction while making the shell denser:
a narrower sidebar, one visible page heading, compact project rows, uppercase
technical navigation, and flatter bordered surfaces. Health indicators query the
existing readiness endpoint and only use the accent state after a successful
response.

## Consequences

- Typography is stable without an external font CDN or runtime network request.
- The release grows by the bundled font subsets.
- Status chrome represents real control-plane readiness instead of placeholder
  copy.
- Authentication and dashboard screens share the same local font families.
