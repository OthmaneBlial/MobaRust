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
operations must be reported rather than simulated. Remote clipboard events are
treated as untrusted input: the desktop UI holds them transiently and requires
an explicit user click before writing to the local clipboard. It never copies
remote text automatically.

## Tests before release

Protocol-independent lifecycle tests cover connect, authenticated, active, lost,
reconnecting, cancelled, failed, and closed transitions. Framing tests cover
malformed/oversized rectangles, invalid dimensions, input bounds, and
credential redaction without opening a socket. A local VNC fixture should cover
framebuffer updates, input, resize, clipboard capability reporting, and server
disconnect. Manual interoperability checks must use a dedicated real VNC
server; mocks alone are not production evidence.

The shared native helper contract also puts a two-second deadline on control,
credential, and event pipe writes. The deadline is tested with a deliberately
backpressured in-memory pipe so cancellation cannot depend on a responsive
peer. It protects helper lifecycle only; it does not prove VNC network
throughput or cross-platform interoperability. If the parent-side input writer
fails, the desktop boundary emits a stable redacted diagnostic, transitions
the session to `Crashed`, and invokes bounded helper stop/reap cleanup without
echoing technical error details or secrets into the UI.

The parent reader also invokes bounded stop/reap cleanup after EOF or a
malformed event before removing the session, so a VNC helper cannot outlive a
failed native pipe. This remains local lifecycle evidence only.

Reader and writer failure paths claim the stop state atomically, so a single
pipe failure cannot produce duplicate crash events while cleanup is racing.

## Isolated implementation experiment

The separate `tools/vnc-helper` workspace now contains a real `vnc-rs 0.5.3`
RFB client behind the native helper contract. It negotiates RFB 3.8, supports
no-auth and VNC-password negotiation through the credential pipe, requests an
RGBA framebuffer, decodes raw/copy-rect updates, and forwards keyboard,
pointer, bounded Latin-1 clipboard input, and Tight/JPEG rectangles through a
bounded native decoder. Client-requested server-side resize is reported rather
than simulated; a server-announced `DesktopSize` change is applied to the
bounded canvas.

`tools/vnc-helper/tests/local_vnc.rs` runs deterministic no-auth and VNC
password RFB fixtures on an OS-assigned `127.0.0.1` port. The authenticated
fixture verifies security-type selection and the DES challenge response before
checking framebuffer, explicit server-side resize rejection, key, pointer,
server-announced resize, clipboard input, server-to-helper clipboard events,
and clean stop. A separate malformed fixture sends a rectangle beyond the
negotiated framebuffer and verifies that the helper emits a stable diagnostic,
fails closed, and exits without forwarding invalid pixels. The current
Tauri UI is
wired to the parent
supervisor/renderer and offers explicit user-triggered reconnect after helper
failure, but this is not yet cross-platform interoperability or production
evidence. The research working notes remain in
`research_vnc_integration/`, and VNC must not be advertised as shipped until
packaging and Windows/Linux/macOS checks are complete. The parent UI surfaces
remote clipboard events as an explicit “Copy text” action without
automatically touching the local clipboard; server-side resize remains
capability-dependent and is reported rather than simulated. The VNC canvas
keeps the negotiated server resolution and scales it locally to the viewport,
with that limitation shown in the overlay.

Clipboard opt-in is enforced at both sides of the native boundary. The parent
rejects server clipboard events that were not requested, while the helper
reports clipboard capability only when explicitly enabled, drops unsolicited
server text otherwise, and refuses client clipboard input before calling the
RFB engine. A loopback fixture verifies that the rejected client command
produces a diagnostic and no `ClientCutText` message reaches the VNC server.
The opt-in text path is still limited to the upstream engine's bounded
Latin-1-compatible channel and does not establish encrypted VNC transport.

The canvas input path accounts for that local scaling: the actual painted
framebuffer rectangle is calculated inside the letterboxed viewport, so input
in unused bands is not sent to the server. Pointer capture preserves drags
that leave the canvas, while pointer release/cancel sends `buttons: 0` at the
last valid remote pixel when necessary. The behavior is covered by
deterministic frontend tests and remains separate from the still-open
cross-platform interoperability gate.

Window focus loss also triggers a bounded client-side release for the last
pointer position and tracked keys, preventing a local focus transition from
leaving remote input logically pressed.

The parent and helper contract also bounds connection metadata before any
helper process starts: hosts are limited to 255 bytes and usernames to 256
bytes, with control characters rejected and invalid values reported without
echoing their contents.

Adjacent motion events are coalesced before crossing the Tauri boundary, while
button transitions and releases remain ordered. The browser-side queue is
capped at 128 items and evicts motion before transitions; a saturated queue
keeps an explicit release as its final safety event. This is a local
backpressure measure for the framebuffer UI, not a claim about VNC network
throughput.

The helper now also reports a server disconnect during negotiation and can
cooperatively cancel a stalled RFB negotiation as soon as `Stop` arrives,
rather than waiting for the connection timeout. Its helper-owned source copy of
the credential is zeroizing. A connected-session loss now emits `Reconnecting`,
retries with a user-configurable, bounded 0–10-attempt policy and exponential
backoff, honors `Stop` during both the delay and handshake, and emits `Failed`
after the final refused/failed attempt. The default remains three enabled
attempts, and users can disable reconnect or set zero attempts. The loopback
fixture verifies recovery to a second real framebuffer and bounded failure
when the fixture disappears. A separate loopback fixture verifies that an
explicitly disabled policy transitions straight from an active loss to
`Failed` without emitting `Reconnecting`. This is helper-level evidence only;
the Tauri parent still requires the full cross-platform and manual
interoperability gate. `vnc-rs 0.5.3` still
requires the authentication callback to return an owned `String`, so that
upstream API limitation remains a credential-lifetime promotion gate and is
not hidden by the local wrapper. The helper moves its zeroizing source buffer
into the one upstream-owned `String` required by that callback, avoiding an
additional `to_string()` copy; the upstream-owned value itself is still not
zeroizing.

The real-process fixtures also wait for the helper to exit while the parent
stdin pipe remains open. The helper reads frames on a dedicated native thread
because Tokio's standard-input adapter uses an uncancellable blocking read;
this keeps terminal failure, Stop, and reconnect shutdown from depending on a
parent-side pipe close. The thread does not log or retain credential material,
and its decoded frames remain zeroizing. This is process-lifecycle evidence,
not cross-platform VNC interoperability evidence.

Because the candidate does not provide a generally validated encrypted
transport, the helper and the Tauri parent fail closed for safety: they accept
only literal loopback IP targets (`127.0.0.1` or `::1`) and reject hostnames or
other addresses before opening a socket. This is a local-fixture restriction,
not a claim that VNC is ready for production use.

The VNC adapter requests an RGBA framebuffer and forwards four bytes per pixel
to the desktop renderer. RDP-only settings such as color-depth selection,
domain, and audio are not passed to the VNC helper; VNC currently exposes
local viewport scaling rather than pretending to provide server-side color
depth control.

The VNC profile now offers an explicit clipboard opt-in backed by the helper's
negotiated Latin-1 text channel; it is independent from the RDP-only native OS
clipboard backend. The VNC profile also offers three bounded quality policies:
`balanced`
prefers ZRLE, `low-latency` prefers raw rectangles, and `low-bandwidth`
prefers Tight/JPEG followed by ZRLE while reducing refresh frequency. Each
policy selects a bounded framebuffer refresh cadence (100 ms, 50 ms, or 250 ms
respectively). Tight/JPEG rectangles are decoded with bounds on compressed
input, decoded dimensions, pixel format, and RGBA output size. These are
client-side encoding preferences; they do not claim to change the VNC server
resolution or invent transport encryption.

The connected-session loop also puts a dedicated two-second deadline on each
keyboard, pointer, wheel, clipboard, and framebuffer-refresh write passed to
the VNC engine. This prevents a full internal input queue from making a helper
wait forever when a server stops accepting traffic; expiry follows the normal
bounded reconnect path. The `vnc-rs` `poll_event` call is wrapped in a separate
250 ms deadline, returning control to the outer command select so Stop and
cancellation remain responsive even when a healthy server is quiet. A poll
timeout is not an idle-session timeout: the helper keeps the session alive and
does not disconnect merely because no framebuffer event arrived.
