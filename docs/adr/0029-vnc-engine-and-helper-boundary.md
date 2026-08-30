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

- real local or explicitly approved VNC server fixture;
- framebuffer updates and rectangle bounds validated end to end;
- keyboard and pointer control;
- authentication without secrets in arguments, environment, logs, or React;
- capability-aware clipboard and resize behavior;
- bounded cancellation, reconnect, and helper crash handling;
- Windows, Linux, and macOS packaging/interoperability evidence;
- dependency and license review.

Until those checks pass, MobaRust must not advertise VNC as implemented. A
mock, screenshot, static framebuffer, or external viewer launch alone is not
evidence of an embedded VNC client.
