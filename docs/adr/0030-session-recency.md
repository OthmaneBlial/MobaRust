# ADR 0030 — Persisted session recency without command history

## Status

Accepted and implemented.

## Context

The session sidebar needs a useful Recent view for saved profiles, including
large catalogs. Persisting terminal commands or connection transcripts would
create an unnecessary privacy and secret-exposure risk.

## Decision

`SessionRecord` stores an optional `last_used_at` Unix timestamp. The native
`session_touch` command updates it when a saved profile is opened. The renderer
sorts and filters the metadata returned by `session_list`; it does not receive
credentials, shell input, terminal output, or connection transcripts.

The field is optional and defaults to `None`, so existing schema-one stores and
secret-free imports remain readable. Touch writes use the same atomic session
store persistence path as other metadata changes.

## Consequences

- Recent profiles remain available after restarting the application.
- A catalog export may contain usage timestamps but never terminal history or
  credential material.
- The feature intentionally does not claim a command audit trail. An optional
  audit event system must remain separate and privacy-conscious.
