# ADR 0013: Version the remote-desktop helper boundary before engine integration

## Status

Accepted. The Rust-side contract, isolated IronRDP/vnc-rs helpers, and the
current Tauri framebuffer/input bridge are implemented; production RDP/VNC
promotion remains pending.

## Context

RDP and VNC are protocol families with substantial native dependencies,
platform glue, input handling, codecs, clipboard behavior, and failure modes.
The current development machine has no `xfreerdp`, `vncviewer`, FreeRDP
`pkg-config` package, or libvncclient installation. Installing system
dependencies or building a native engine globally is outside this experiment
and would create unnecessary risk for the host machine.

## Decision

MobaRust will keep protocol engines behind a controlled helper process. The
Rust crate `mobarust-remote-desktop` owns the versioned boundary and validates
messages before a process is started:

- launch arguments contain host metadata only, never passwords, credential
  references, private keys, or tokens;
- credentials are handed over through a separate native channel after helper
  startup;
- control messages are typed (`start`, `stop`, `resize`, keyboard, pointer,
  wheel, and clipboard); wheel deltas are bounded before reaching a helper;
- events are typed (hello, native capability report, lifecycle state,
  framebuffer, clipboard, and bounded diagnostics); capability reports are
  emitted by the running helper so the UI can distinguish a compiled backend
  from a protocol feature flag, while the native parent rejects a report for
  a protocol or requested feature other than the one requested at session
  start; the report also includes `transportEncrypted`, which is validated by
  the parent and displayed by the UI instead of being inferred from a
  protocol label; RDP capability depth entries are validated against the
  shared `16` or `32`-bit contract before they can cross the boundary, and
  active framebuffer, clipboard, or `Active` state events are rejected until
  one validated report has been received;
- envelopes and every deserialized payload reject unknown fields, so a
  malformed or newer-incompatible helper message fails closed instead of
  silently discarding data;
- connection metadata is bounded and rejects control characters and leading or
  trailing whitespace before it reaches a helper, listener, or protocol
  channel;
- the parent requires `Hello` as the first event, `Starting` before the
  initial `Ready` and after every `Reconnecting` event, `Ready` before each
  capability report, and rejects duplicate handshakes, premature reconnects,
  or active data that arrives before a validated capability report;
- framebuffer and clipboard data are accepted only after the corresponding
  attempt has entered `Active`; a `Reconnecting` event starts a fresh
  capability-to-active sequence, clears the cached capability report, and
  clears remote input state in the UI, so stale data or pressed keys cannot
  cross the boundary while a new connection is being established;
- the parent applies a five-second deadline until the helper has completed its
  `Hello` and capability-report handshake, restarting that deadline after each
  `Reconnecting` event; a silent or half-started cycle becomes a failed session
  and reuses the bounded stop/reap path;
- the parent stores the requested protocol policy with each live session and
  rejects protocol-unsupported commands before they enter the helper queue;
  VNC server resize is rejected, RDP/VNC clipboard input requires explicit
  session opt-in, and the policy is rechecked against the running helper's
  capability report before forwarding input; inbound clipboard events are also
  rejected when the runtime report does not advertise clipboard support;
- interactive keyboard, pointer, wheel, resize, and clipboard commands are
  accepted only after the helper reaches `Active`; `Stop` remains permitted in
  every non-terminal phase so startup and reconnect cancellation cannot be
  blocked by readiness gating;
- every frame has a four-byte big-endian body-length prefix and an 8 MiB
  maximum body; wire version 2 sends framebuffer pixels as binary RGBA,
  while control, credential and other event payloads retain strict JSON;
- clipboard input is capped at 1 MiB;
- helper lifecycle distinguishes protocol failure, crash, cancellation, and
  clean stop;
- debug formatting redacts opaque credential references and clipboard text.

Wire version 2's framebuffer body is `MRFB` (four bytes), wire version
(`u16`), width (`u16`), height (`u16`), then exactly `width × height × 4` RGBA
bytes. All integer fields are big-endian. The ten-byte header is reserved in
`MAX_FRAMEBUFFER_BYTES`, so an accepted display fits the body budget regardless
of pixel values. The decoder checks the outer length, version, dimensions and
exact pixel count before copying pixel data. Malformed frames fail closed;
the existing zeroizing temporary buffers and pipe-write deadlines remain.

The version change is deliberate: the parent and both helpers use the shared
`WIRE_VERSION` in their startup handshake. Version 1 and version 2 binaries
must not be mixed; old/new mismatches are rejected before remote data is
accepted. Published v0.1.17 installers remain version 1 until rebuilt as a
matching app/helper cohort. This native pipe change does not change the Tauri
renderer event schema or establish GUI performance or real-server compatibility.

The crate is a contract, test seam, and native-parent API. The isolated
`tools/rdp-helper` and `tools/vnc-helper` implement real protocol-client paths
behind that boundary, and the current debug Tauri package wires their typed
framebuffer/input events into the desktop canvas. This is still a controlled
integration experiment: production promotion requires the interoperability,
certificate, reconnect, packaging, and platform evidence listed below.

## Consequences

The process boundary can contain native crashes and allows bounded graceful
shutdown followed by forced termination. It also adds packaging, IPC, and
framebuffer-copy work. The UI offers explicit user-triggered reconnect after a
helper failure; it does not run an unbounded background reconnect loop. A real
engine experiment must measure input latency, resize behavior, clipboard,
reconnect, audio, certificate handling, and Windows interoperability before
the adapter is promoted.

Fullscreen and visual scaling remain renderer-owned controls: the UI can place
the canvas in fullscreen and preserve its aspect ratio without asking the
remote protocol to resize. RDP dynamic resize is coalesced to the latest
bounded viewport size before it crosses the Tauri command queue, and pending
resize timers are cancelled when a view closes. VNC server-side resize remains
capability-dependent and is not simulated.

The protocol-independent contract tests run without spawning a process or
reading host configuration. The helper's EOF smoke test also avoids sockets.
A future interoperability fixture must use a dedicated temporary directory and
local or explicitly approved test server. See
`docs/security/safe-testing.md` for the workstation safety boundary.
