# Native VNC engine findings

## LibVNCClient

- Official project: https://github.com/LibVNC/libvncserver
- Official documentation: https://libvnc.github.io/
- The project exposes LibVNCClient as a cross-platform C client library and
  documents client examples and `pkg-config`/CMake integration.
- The build can use OpenSSL, GnuTLS, Libgcrypt, or included crypto depending on
  configuration. The included crypto path only supports VNC authentication,
  so a product build must make the crypto choice explicit.
- The upstream project is GPL-2.0-or-later. Its own FAQ states that linking
  LibVNCClient makes the linking program a derivative work. This conflicts with
  MobaRust's current Apache-2.0 distribution unless the helper is distributed
  under a compatible copyleft strategy and the legal consequences are accepted.
- The upstream README calls out client-pull framebuffer behavior and recommends
  Tight encoding and an appropriate pixel format for slow links. Continuous
  Updates is not currently supported there.

## TigerVNC

- Official project: https://github.com/TigerVNC/tigervnc
- It provides a cross-platform `vncviewer`, but the project is GPL-2.0 and is a
  complete C++/FLTK application rather than a small embeddable client ABI.
- It is useful as an interoperability/manual reference and possible external
  runner, but not a clean in-process dependency for the current product.

## Rust-native candidates

- `vnc` 0.4.0 is MIT/Apache-2.0 and exposes a client state machine, but its
  published release is from 2016 with old dependencies and limited documented
  platform coverage. It is suitable for a disposable protocol experiment, not
  an automatic production selection.
- The RustVNC organization advertises newer Apache-2.0 RFB encoding crates and
  a planned/client-library direction, but the public material currently marks
  the desktop VNC client as roadmap work. Encoding support alone is not a VNC
  client implementation.

## Conclusion

No candidate currently proves a production-ready, Apache-compatible,
cross-platform client with the required input, clipboard, resize, reconnect,
and cancellation behavior. MobaRust should keep the existing helper boundary,
run a pinned Rust-native client experiment first, and retain an explicitly
optional external TigerVNC/LibVNC route only after legal and packaging review.
