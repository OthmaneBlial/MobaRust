# ADR 0020: Keep snippets secret-free and reviewable

## Status

Accepted and implemented for the first snippet-library slice.

## Decision

Snippets are reusable command templates stored in a separate versioned
`snippets.json` document. Each record contains only an identifier, title,
description, command text, tags, and validated variable names. The store uses
atomic writes and refuses corrupt or unsupported documents instead of silently
replacing them.

The renderer may list and edit snippet metadata through narrow typed Tauri
commands. Rendering substitutes explicitly entered variable values for a
preview or clipboard copy. A snippet is never sent to a terminal
automatically: the operator must review it and paste/send it deliberately.

## Security boundary

Snippet commands are user-authored text and may contain secrets accidentally.
The application does not attempt to classify or redact them. Snippets must
therefore remain separate from credential storage, must not receive automatic
credential expansion, and must not be included in diagnostic exports without
an explicit future privacy decision. Clipboard copies remain subject to the
operating system clipboard trust boundary.

## Consequences

The first implementation is useful for Docker, Kubernetes, Git, Linux, and
networking templates while keeping execution visible and cancellable by the
operator. Macro automation and broadcast execution remain separate features
with stronger permission and emergency-stop requirements.

## Verification

Core tests validate variable names and duplicate detection. Store tests cover
durable round trips, deletion, and refusal to replace a corrupt snippet file.
Frontend checks cover the typed command surface and the rendered preview UI.
