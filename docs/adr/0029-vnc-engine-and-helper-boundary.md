# ADR 0029: VNC engine and helper boundary

## Status

Accepted for investigation. An isolated `vnc-rs 0.5.3` experiment is
validated against a local RFB fixture; no production VNC engine is selected
yet.

## Decision

MobaRust will keep VNC behind the existing controlled remote-desktop helper
boundary. The first implementation experiment must use a maintained,
license-compatible Rust RFB client if one can prove the required behavior. The
parent remains responsible for lifecycle, typed input, cancellation, bounded
frame validation, and native credential handoff. The helper remains
responsible for RFB negotiation, authentication, decoding, and protocol-specific
capability reporting.

LibVNCClient and TigerVNC remain research/manual interoperability references,
not dependencies of the Apache-2.0 parent. Their GPL licensing and native
build surface require a separate legal and packaging decision before any
bundling or linking. The historical `vnc` crate is not promoted without a
maintenance, security, and cross-platform review. The newer `vnc-rs` candidate
is also kept in a separate helper until parent integration and platform
evidence exist.

## Required evidence before promotion

- real local or explicitly approved VNC server fixture; the current local
  fixture covers both no-auth and VNC password challenge-response paths;
- framebuffer updates and rectangle bounds validated end to end;
- keyboard and pointer control;
- authentication without secrets in arguments, environment, logs, or React;
- capability-aware clipboard and resize behavior;
- bounded client-side quality policies with explicit encoding preferences;
- bounded cancellation, reconnect, and helper crash handling;
- Windows, Linux, and macOS packaging/interoperability evidence;
- dependency and license review.

The isolated helper currently has loopback evidence for authentication,
framebuffer, bounded keyboard/pointer input, clean stop, negotiation disconnect,
cooperative cancellation, connected-session loss, and bounded reconnect
attempts, including a bounded client-to-server clipboard message. A reconnect
keeps the helper process and credential handoff inside the native boundary; it
does not expose the password to the parent or React.
Its dependency audit is clean. The pinned
`vnc-rs 0.5.3` callback API still requires an owned password `String`; the
helper moves its zeroizing source buffer into that one value without an
additional plaintext clone, but the upstream-owned value itself is not
zeroizing. This API limitation remains a promotion gate until an audited
alternative is selected.

The helper also accepts only three bounded quality policies. They change the
requested supported VNC encoding order and refresh cadence, while the renderer
continues to receive a normalized RGBA framebuffer. The low-bandwidth policy
now requests Tight/JPEG before ZRLE and the helper decodes those rectangles
through a bounded native decoder. The policy is persisted as session metadata
with a compatibility default for older profiles; it does not expose a secret or
alter the server-side display mode.

After the helper reaches `Ready`, it reports its native capability set through
the shared event contract: text clipboard is available, server-side resize is
not, local scaling is available, and Gateway/audio are unavailable. The parent
forwards this metadata to the UI so the visible behavior matches the helper
that is actually running. This does not replace the required cross-platform
interoperability evidence.

The candidate is currently restricted to local fixtures: both the helper and
the Tauri parent accept only literal loopback IP targets (`127.0.0.1` or `::1`)
and reject hostnames and other addresses before opening a socket. This fail-
closed rule remains until an audited transport-security strategy is selected.

Pointer and wheel coordinates are clamped to the active framebuffer dimensions
inside the helper. This protects delayed input events that arrive after a
server-announced resize and keeps protocol coordinates within the negotiated
surface.

Until those checks pass, MobaRust must not advertise VNC as implemented. A
mock, screenshot, static framebuffer, or external viewer launch alone is not
evidence of an embedded VNC client.
