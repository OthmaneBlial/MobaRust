# Rust integration findings

The existing `mobarust-remote-desktop` contract is a suitable seam for VNC:
typed start/stop/resize/key/pointer messages, bounded framebuffer events,
bounded diagnostics, and a separate native credential frame.

## Options

| Option | Strengths | Risks |
| --- | --- | --- |
| Rust-native RFB client in parent | Memory safety, simple packaging, direct async lifecycle | Current candidates do not yet prove feature/platform maturity |
| Rust-native RFB client in helper | Contains protocol crashes, preserves UI boundary, easy forced cancellation | Extra copy/IPC and helper packaging |
| LibVNCClient helper | Mature protocol and broad interoperability | GPL licensing, C ABI/crypto/build surface |
| TigerVNC external runner | Mature viewer and broad manual interoperability | GPL, process/window integration, weak embedded framebuffer story |

The safest first implementation remains a controlled helper. It must receive
credentials only over a native pipe, never through arguments/environment, and
must not inherit arbitrary environment state. The parent should validate frame
dimensions and pixel byte counts before emitting anything to the renderer.

The helper must expose a real RFB loop: negotiate security, authenticate, send
pixel-format and encoding preferences, request framebuffer updates, decode
rectangles, and forward pointer/keyboard events. A static framebuffer, mock
event stream, or screenshot does not count as implementation.

Clipboard support must be an explicit capability because VNC servers differ in
their clipboard extensions. Resize is also capability-dependent; the UI should
report unsupported resize rather than pretending the remote desktop changed.
