# VNC integration research

VNC remains a separate protocol adapter. It must not be inferred from RDP
support or represented by a placeholder image.

## Evidence reviewed

The pinned Remmina source treats VNC as an optional plugin and looks for a
mature `LIBVNCSERVER` installation. It also has a separate optional GTK-VNC
path. The local 1Remote corpus uses a VNC-specific host and supports external
UltraVNC/TightVNC runners. This supports reusing a protocol implementation
rather than writing RFB message handling from scratch.

Official upstream references reviewed on 2026-08-30:

- [LibVNC/libvncserver](https://github.com/LibVNC/libvncserver) and its
  [build/API documentation](https://libvnc.github.io/): mature cross-platform
  C client/server library, GPL-2.0-or-later, CMake/pkg-config integration,
  selectable crypto backends, and documented client examples.
- [TigerVNC](https://github.com/TigerVNC/tigervnc): mature cross-platform
  viewer, GPL-2.0, C++/FLTK application rather than a small embedding ABI.
- [`vnc` 0.4.0 documentation](https://docs.rs/vnc/latest/vnc/):
  MIT/Apache-2.0 and a client state machine, but a 2016 release with old
  dependencies and insufficient current platform evidence.
- [RustVNC](https://github.com/rustvnc): promising Apache-2.0 RFB encoding
  work, but the public roadmap still separates the encoding libraries from a
  production desktop client.

LibVNC and TigerVNC are not automatic choices for the Apache-2.0 MobaRust
distribution. LibVNC's own documentation explains that linking its client
creates a derivative work, so a helper does not remove the licensing question.
The old `vnc` crate is a candidate for an isolated experiment only. Encoding
support by itself is not a VNC client.

## Candidate architecture

Evaluate, in order:

1. a maintained Rust RFB client whose license, lifecycle, and platform support
   fit MobaRust, initially inside the existing controlled helper boundary;
2. a narrowly scoped native helper around libvncclient/libvncserver-family
   code, only after a separate legal and packaging decision;
3. an explicitly optional external TigerVNC/other viewer integration for users
   who already have a compatible viewer, never presented as an embedded VNC
   workspace;
4. direct FFI only if a measured helper limitation justifies its additional
   crash and ABI surface.

The helper must implement a real RFB loop: security negotiation,
authentication, pixel-format and encoding selection, framebuffer rectangle
decoding, pointer/keyboard input, capability-aware clipboard, cancellation,
and clean shutdown. The parent contract must validate dimensions and pixel
byte counts before forwarding frames to the renderer. A static framebuffer,
mock event stream, or screenshot does not count.

VNC profiles must visibly distinguish legacy VNC authentication from transport
encryption. Resize and clipboard are capability-dependent; unsupported
operations must be reported rather than simulated.

## Tests before release

Protocol-independent lifecycle tests cover connect, authenticated, active, lost,
reconnecting, cancelled, failed, and closed transitions. Framing tests cover
malformed/oversized rectangles, invalid dimensions, input bounds, and
credential redaction without opening a socket. A local VNC fixture should cover
framebuffer updates, input, resize, clipboard capability reporting, and server
disconnect. Manual interoperability checks must use a dedicated real VNC
server; mocks alone are not production evidence.

## Isolated implementation experiment

The separate `tools/vnc-helper` workspace now contains a real `vnc-rs 0.5.3`
RFB client behind the native helper contract. It negotiates RFB 3.8, supports
no-auth and VNC-password negotiation through the credential pipe, requests an
RGBA framebuffer, decodes raw/copy-rect updates, and forwards keyboard,
pointer, and bounded Latin-1 clipboard input. Unsupported JPEG rectangles and
server-side resize are reported rather than simulated.

`tools/vnc-helper/tests/local_vnc.rs` runs a deterministic no-auth RFB fixture
on an OS-assigned `127.0.0.1` port and verifies handshake, framebuffer, key,
pointer, and clean stop. This is meaningful protocol evidence, but it is not
yet cross-platform interoperability or a production UI integration. The
research working notes remain in `research_vnc_integration/`, and VNC must not
be advertised as shipped until the parent supervisor/renderer, reconnect,
packaging, and Windows/Linux/macOS checks are complete.
