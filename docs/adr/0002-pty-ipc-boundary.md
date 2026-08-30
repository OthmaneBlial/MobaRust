# ADR 0002: bounded PTY event transport

## Status

Accepted for the local terminal slice.

## Decision

The Rust PTY reader writes into a bounded synchronous channel. A second native worker coalesces reads for up to 8 ms and emits chunks capped at 32 KiB through the `terminal://output` event. The React side writes those chunks directly to xterm.js.

Input, resize, and close are separate typed Tauri commands. The local PTY
target is a strict tagged payload: unknown fields are rejected at the nested
target boundary instead of being silently discarded. No command accepts an
arbitrary shell string for native execution; the only shell process started by
this slice is the user's discovered local shell.

xterm also receives a small local HTTP(S)-only link provider. It parses each
rendered buffer line with a 16-link and 2 KiB-per-link bound, refuses URLs with
embedded user information, and asks for explicit confirmation before opening
an external page. Remote output remains terminal text and is never inserted as
HTML.

PTY title-change events are normalized to bounded display-only tab text, and
bell events are surfaced as a rate-limited UI notice. Neither event becomes
HTML, a command, or an unbounded stream of frontend work.

## Consequences

- A noisy process cannot create an unbounded IPC queue.
- Small reads are delayed by at most one batching window, preserving interactive feel while avoiding one event per character.
- The bounded channel creates backpressure at the native reader; future SSH adapters should reuse the same policy.
- The first release can measure batch size and latency independently from terminal rendering.

## Follow-up probes

Measure 10k/100k/1M line output, Unicode and ANSI-heavy output, memory, CPU, and resize latency on all target operating systems before tuning the limits.
