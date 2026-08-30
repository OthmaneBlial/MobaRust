# VNC integration research plan

## Main question

What is the safest maintainable way for MobaRust to provide a real, controllable
VNC client across Windows, Linux, and macOS without rewriting VNC protocol
machinery or exposing credentials to the frontend?

## Subtopics

1. Mature native engines: identify actively maintained VNC client engines and
   their supported protocol features, APIs, and licenses.
2. Rust integration options: compare Rust-native crates, C library FFI, and an
   isolated helper process for framebuffer, keyboard, mouse, clipboard, and
   resize behavior.
3. Distribution and security: identify platform, packaging, licensing,
   cancellation, and credential-boundary constraints.

## Expected synthesis

The findings will be compared against the existing versioned helper contract,
then summarized in `docs/research/vnc.md` and an ADR. No live VNC endpoint,
hardware, personal credential, or local SSH material is required for this
research.
