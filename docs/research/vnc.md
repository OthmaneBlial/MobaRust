# VNC integration research

VNC remains a separate protocol adapter. It must not be inferred from RDP
support or represented by a placeholder image.

## Local evidence

The pinned Remmina source treats VNC as an optional plugin and looks for a
mature `LIBVNCSERVER` installation. It also has a separate optional GTK-VNC
path. This supports reusing a protocol implementation rather than writing RFB
message handling from scratch.

## Candidate architecture

Evaluate, in order:

1. a maintained Rust RFB client whose license and platform support fit
   MobaRust;
2. a narrowly scoped native helper around libvncclient/libvncserver-family
   code, with its own license and packaging review;
3. direct FFI only if the helper boundary is measurably insufficient.

The adapter contract must cover host, port, authentication, framebuffer
updates, keyboard, mouse, clipboard, scaling, fullscreen, resize behavior,
quality settings, reconnect, cancellation, and clean shutdown.

## Tests before release

Protocol-independent lifecycle tests cover connect, authenticated, active,
lost, reconnecting, cancelled, failed, and closed transitions. Local VNC
fixtures should cover framebuffer updates, input, resize, and server
disconnect. Manual interoperability checks must use a real VNC server; mocks
alone are not production evidence.
