# ADR 0005: Version the secret-free session store

## Status

Accepted for the first saved-session slice.

## Decision

Saved session definitions use a small versioned JSON document under the
platform application-data directory. Writes use a unique temporary file,
`fsync`, and rename; on Unix, newly created local JSON store files use mode
`0600` because metadata can still reveal key paths, credential references, and
connection topology. The document stores connection metadata and opaque
credential references only. SSH host-trust selection (known-hosts path or
operator-pinned fingerprint) is also persisted as non-secret connection
metadata, so reconnecting a saved profile does not silently change its trust
policy. Unknown top-level fields and corrupt JSON are reported instead of
being replaced or silently discarded.

The same private temporary-file writer is used for the separate settings,
snippet, macro, and bounded audit JSON stores. This is a local file-permission
baseline, not protection against a malicious process with the same user
privileges or an OS backup that is configured to capture application data.

This is an intentionally narrow foundation. SQLite remains an option for the
large-session/search workload once the catalog needs indexes, audit history,
snippets, transfers, or migrations beyond this document model.

On Windows, the final replacement uses the OS replace-existing move primitive
with write-through semantics. It never deletes the existing catalog before the
replacement has succeeded, so a failed write cannot first remove the durable
session file.

## Consequences

The current implementation is easy to inspect, migrate, back up, and test. It
does not contain passwords, private keys, or tokens. A portable profile cannot
be made secure by copying this JSON beside an executable; portable encrypted
storage requires a separate key-management design.
