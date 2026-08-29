# ADR 0005: Version the secret-free session store

## Status

Accepted for the first saved-session slice.

## Decision

Saved session definitions use a small versioned JSON document under the
platform application-data directory. Writes use a unique temporary file,
`fsync`, and rename. The document stores connection metadata and opaque
credential references only. Unknown top-level fields and corrupt JSON are
reported instead of being replaced or silently discarded.

This is an intentionally narrow foundation. SQLite remains an option for the
large-session/search workload once the catalog needs indexes, audit history,
snippets, transfers, or migrations beyond this document model.

## Consequences

The current implementation is easy to inspect, migrate, back up, and test. It
does not contain passwords, private keys, or tokens. A portable profile cannot
be made secure by copying this JSON beside an executable; portable encrypted
storage requires a separate key-management design.
