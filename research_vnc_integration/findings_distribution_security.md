# VNC distribution and security findings

- LibVNC's official build documentation describes CMake, OpenSSL/GnuTLS,
  optional compression/codec dependencies, and separate Windows cross-build
  concerns. Bundling it would therefore require per-platform reproducible
  builds and license notices.
- TigerVNC is GPL-2.0 and its viewer is a complete native application. It is a
  useful documented external integration target, but bundling or linking it
  needs a deliberate license decision.
- VNC authentication is not equivalent to transport encryption. A VNC profile
  must visibly identify whether the server connection is protected by an
  external tunnel/TLS-capable security type or is plaintext after legacy VNC
  authentication.
- Passwords must be resolved in Rust from the platform/portable vault and
  handed to the helper over the dedicated native channel. They must not appear
  in command-line arguments, environment variables, logs, crash diagnostics,
  exported profiles, or renderer state.
- Tests can validate lifecycle, framing, bounds, cancellation, and malformed
  rectangle behavior without opening a network socket. Real protocol evidence
  requires a dedicated local fixture or an explicitly approved test server;
  this workstation must not be used as an implicit VNC target.
