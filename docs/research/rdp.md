# RDP integration research

This is a local study of the pinned `base/FreeRDP` and `base/remmina` clones
(`b2a1214` and `bb33690`, respectively).
It is not a claim that MobaRust currently supports RDP.

## Observations

- FreeRDP is a substantial Apache-2.0 C implementation with public client
  APIs, platform glue, plugins, codecs, certificate handling, input, display
  updates, clipboard, audio, and gateway-related surfaces.
- FreeRDP is built and packaged as a CMake project with versioned include,
  library, plugin, and data paths. Treating it as a tiny Rust library would
  hide meaningful packaging and ABI work.
- Remmina detects FreeRDP and WinPR at build time and enables its RDP plugin
  only when the required client packages are present. It has separate FreeRDP
  2 and FreeRDP 3 build paths.
- Remmina also demonstrates that protocol plugins need explicit optional
  dependency behavior. Its GPL-2.0-or-later application is a reference for
  integration shape, not code or asset source for MobaRust's Apache-2.0 tree.
- The current macOS host has no `xfreerdp`/`xfreerdp3` executable and no
  discoverable FreeRDP `pkg-config` package. No global dependency installation
  was attempted; the first experiment therefore remains process-contract and
  lifecycle work inside the repository.
- The same host has no `cmake` executable available for a repository-local
  FreeRDP configure/build probe. The ignored `base/FreeRDP` research corpus was
  kept read-only; no build directory, dependency download, or system install
  was started.
- An isolated disposable Cargo probe successfully compiled and instantiated
  `ironrdp-client 0.1.0` with synthetic configuration, without opening a
  socket. Adding it directly to the main workspace was deliberately reverted:
  its `picky` dependency pins `aes-gcm 0.11.0-rc.4`, which conflicts with the
  portable vault's `aes-gcm 0.11.1`. The vault dependency was not weakened for
  this experiment.
- A current upstream manifest check on 2026-08-30 still shows
  `ironrdp-connector` depending on `picky = 7.0.0-rc.25` and `sspi`; upstream
  tracks removal of that deprecated CredSSP surface in [issue #1433](https://github.com/Devolutions/IronRDP/issues/1433),
  which remains open. The [RustSec RSA advisory](https://rustsec.org/advisories/RUSTSEC-2023-0071.html)
  still reports no patched `rsa` release. This confirms that the candidate's
  dependency gate is still current rather than a stale local lockfile result.

## Architecture options

| Option | Strength | Main risk | Initial decision |
| --- | --- | --- | --- |
| FreeRDP FFI | Direct access and low overhead | Large unsafe ABI and callback surface | Defer |
| Generated/native bindings | Better typed Rust calls | Still inherits ABI and build matrix | Defer |
| Dynamically linked library | Smaller MobaRust binary | Runtime discovery, ABI drift, missing libraries | Evaluate per platform |
| Controlled helper process | Crash containment and clear cancellation | IPC and helper packaging | Select for prototype |
| Isolated subprocess | Strong failure boundary | Framed protocol and lifecycle complexity | Select with helper |
| Framebuffer bridge | Cross-platform Tauri surface and testable pixels | Input/display latency and copy cost | Select first |
| Native window embedding | Potentially best latency | Window-handle lifecycle differs on Win/Linux/macOS | Later experiment |

### Decision matrix

The following is a qualitative architecture review from the pinned
FreeRDP/Remmina source study and the isolated Rust experiment. It is not a
benchmark and does not claim that an option is production-ready.

| Option | Stability / maintainability | Security boundary | Performance / input latency | License | Packaging | Windows / Linux / macOS | Clipboard / audio / resize / multi-monitor |
| --- | --- | --- | --- | --- | --- | --- | --- |
| FreeRDP FFI | Mature engine, high unsafe ABI and callback maintenance | Weak unless wrapped in a helper | High potential, native event path | Apache-2.0 engine; dependency review required | High: ABI, plugins, codecs, data paths | Strong candidate, but each target needs native packaging evidence | Broad protocol surface, but each feature needs explicit bridge and tests |
| Generated/native bindings | Less handwritten call glue; ABI still evolves | Same native-risk profile unless isolated | High potential | Depends on wrapped FreeRDP artifacts | High: headers, library, plugins, target toolchains | Same platform matrix as FreeRDP | Depends on exposed bindings and callback ownership |
| Dynamically linked library | Smaller app and replaceable engine | Runtime library trust and ABI drift | High when library is present | Depends on exact distribution/linking choice | Very high: discovery, versions, plugins, installers | Conditional on a supported library on all targets | Potentially broad, but failures must degrade explicitly |
| Controlled helper process | Clear crash/cancellation boundary and test seam | Strongest practical boundary in this repository | Good; framebuffer copy is measurable overhead | Helper engine still needs separate license review | Medium-high: one helper/resource per target | Portable shape; real evidence still required on each OS | Parent contract can expose only supported capabilities |
| Isolated subprocess | Strong failure containment and independent dependency graph | Strong, if credentials use native IPC only | Good, with startup/IPC overhead | Isolated engine license remains visible | Medium-high; separate build and audit | Suitable for target-specific engines | Feature support is explicit rather than inferred |
| Framebuffer bridge | Renderer-independent and easy to bound/test | Remote pixels remain untrusted data | Moderate; copy/encode cost and input round trips | Inherits engine license | Moderate; no native window handle ABI | Same renderer contract on all targets | Clipboard/audio/display changes require typed events |
| Native window embedding | Lowest possible copy latency | Larger native handle and callback surface | Potentially best | Inherits engine and platform glue licenses | Very high; window handles differ by target | Requires separate Win32/X11/Wayland/AppKit work | Better native feature access, harder lifecycle and testing |

The selected prototype is therefore an isolated controlled helper with a
versioned framebuffer bridge. FreeRDP remains the mature-engine reference, but
no FreeRDP library or executable is installed on the development Mac and no
binding is shipped. The IronRDP candidate is kept in its own audited workspace
until its dependency, trust-policy, packaging, and Windows interoperability
gates are resolved. This decision is reversible after real measurements of
startup, framebuffer throughput, input latency, clipboard/audio behavior,
dynamic resize, and multi-monitor support on the target platforms.

## IronRDP candidate result

The isolated `tools/rdp-helper` adapter confirms that a Rust-native candidate
can be placed behind the helper boundary with a reusable `RdpClient`, typed
image output, keyboard/mouse/resize input, TLS/CredSSP configuration, and a
zeroizing native credential frame. Its clipboard policy is now explicit and
opt-in: Windows can select IronRDP's native OS clipboard backend, while the
helper rejects the opt-in before connecting on macOS/Linux until a reviewed
platform adapter exists. Audio requests are rejected at both the desktop
boundary and helper boundary rather than silently ignored. This is still
not a production selection: cross-platform certificate fixture coverage,
reconnect interoperability, audio, gateway behavior, packaging, and real
Windows interoperability remain open gates. The helper now rebuilds a native
RDP client after an active-session loss with a user-configurable, bounded
0–10-attempt exponential-backoff policy; it keeps the credential inside the
helper and honors Stop during the delay. Each retry emits a fresh `Starting`
state before the helper rebuilds its `Ready`/capabilities/`Active` cycle.
Legacy profiles default to three
enabled attempts, while users can disable reconnect or set zero attempts.
No global package, personal credential, or remote server was used during the
local validation.

The candidate's `clipboard` feature was inspected locally at the pinned
`ironrdp-client 0.1.0` source. `ClipboardType::Enable` uses IronRDP's native
Windows clipboard implementation on Windows, but falls back to a stub backend
on non-Windows platforms. MobaRust therefore keeps clipboard disabled by
default, passes an explicit `--clipboard-enabled` opt-in only for RDP, and
selects `ClipboardType::Enable` only when that policy is requested. On
macOS/Linux the native parent rejects that opt-in before launching a helper,
with the helper retaining the same defense in depth instead of silently
claiming support. The Windows backend owns the OS clipboard
exchange; the helper's clipboard command does not create a second clipboard
authority or automatically write remote content into the Mac clipboard. A
future implementation still needs reviewed macOS/Linux adapters, bounded text
handling, explicit user action for remote-to-local copies, deterministic
cleanup, and Windows interoperability evidence. The helper now emits a typed
capability report after `Ready`; the native parent first verifies that the
reported protocol matches the requested session, then forwards it to the UI.
The parent also verifies requested clipboard, audio, Gateway, and RDP color
depth capabilities before forwarding the report. The UI can show the actual
platform backend and disable unsupported clipboard actions before a user
attempts them. A mismatched report fails the session locally and is never
treated as proof of a different protocol or feature. The parent also refuses
active framebuffer, clipboard, or `Active` state events until a valid report
has arrived, and requires the versioned `Hello` event to be the first helper
message.

Connector failures are reduced to stable categories at the helper boundary,
including authentication/access rejection, protocol negotiation, malformed
data, and TLS/certificate-or-transport validation. A local source audit of the
published `ironrdp-tls 0.2.2` implementation found that its `native-tls` builder
calls `danger_accept_invalid_certs(true)` and disables SNI; its published
Rustls path also uses a no-certificate-verification implementation. The helper
experiment now patches that dependency inside its isolated workspace with
`ironrdp-tls-validated`, a small compatibility crate that uses Rustls, platform
certificate verification, and SNI. This improves the candidate's trust
behavior but does not establish production interoperability: certificate
fixtures beyond the macOS local rejection proof, Windows evidence, dependency audit, and packaging gates remain. RDP
hostname/IP metadata is now allowed only through this native verification path.

The isolated candidate also accepts an explicit RD Gateway endpoint and
username, and routes a separate opaque gateway credential reference to the
helper as a role-tagged zeroizing native frame. The IronRDP transport selects
gateway mode only from that explicit metadata; no gateway secret is placed in
argv, logs, ordinary profile JSON, or frontend state. This is a local
architecture and boundary increment, not gateway interoperability evidence:
the trust policy, dependency audit, Windows/Linux validation, and real Gateway
server checks remain open.

The helper fails closed at runtime for unsafe trust configuration: it rejects
inherited `SSL_CERT_FILE`, `SSL_CERT_DIR`, and `SSLKEYLOGFILE` settings before a
connection attempt, then passes the explicit hostname or IP to the native TLS
adapter for DNS, SNI, and platform certificate verification. The same native
`ServerName` parser rejects malformed targets before a socket is opened,
without echoing the submitted value. Invalid or untrusted certificates are
rejected. Local tests still use only disposable loopback listeners and never
contact a real remote host.

The macOS-only
`platform_tls_rejects_a_self_signed_loopback_certificate` test adds a concrete
trust-store proof: it creates a short-lived synthetic self-signed certificate,
serves it through a disposable OpenSSL process on `127.0.0.1`, and verifies
that the platform verifier rejects it for the matching server name. The test
does not read personal certificates or contact a remote host. It proves the
TLS trust decision only; it is not RDP-server interoperability evidence.
Equivalent Windows/Linux certificate-store fixtures and real RDP-server
testing remain open.

Real-server promotion remains a hard gate, not a missing preference in the UI:
MobaRust still needs deterministic cross-platform certificate fixtures, an
audited dependency chain, and Windows interoperability evidence. The local
Rustls verifier adds
platform trust-store and packaging surface on Windows, macOS, and Linux; that
must be included in the future distribution matrix. No TLS key-log output or
ambient certificate override is allowed during local experiments.
The helper owns a 15-second startup deadline around the candidate's network
handshake. When it expires, it requests a cooperative close and waits only a
separate bounded grace period before forcing task termination. A stalled
loopback handshake test verifies that this path returns promptly; it does not
prove remote-server interoperability.

The candidate also normalizes a closed IronRDP input channel into a stable
helper-level failure. It stops the client before returning and routes an active
or reconnecting attempt through the existing bounded reconnect budget; an
initial-attempt failure emits `Failed` instead of returning an unhandled raw
channel error. This protects the lifecycle boundary only and does not change
the still-open dependency, certificate, or Windows interoperability gates.

The same bounded stop/reap path now runs when the IronRDP output channel closes
or emits an unexpected termination error before the helper chooses `Lost` or
`Failed`. A client task therefore cannot be left detached while the outer loop
starts a reconnect. This remains local lifecycle evidence, not proof of a real
RDP server or cross-platform runtime.

The real-process fixture now also sends `Start`, the session credential, and the
role-tagged Gateway credential in sequence, then verifies that the helper
reaches `Ready` without treating the second frame as invalid input. A closed
loopback Gateway endpoint ends the candidate cleanly and the test asserts that
neither fixture secret appears in diagnostics. This proves ordering and
redaction at the native process boundary; it does not prove Gateway protocol
interoperability.

## Local real-server fixture

The opt-in `local-rdp-fixture` test now runs the compiled helper against the
official [`ironrdp-server`](https://docs.rs/ironrdp-server/0.13.0) implementation
on `127.0.0.1`. The fixture generates a short-lived private CA and a separate
server certificate with an IP SAN in a disposable temporary directory, then
uses the test-only CA feature in the isolated TLS compatibility crate. It does
not modify the macOS trust store, inspect personal certificates, or contact a
remote host.

The test proves a real TLS/Hybrid handshake, configured username/domain/password
credential acceptance,
non-empty decoded framebuffer delivery, keyboard and mouse input reaching the
server handler, a 400×240 monitor-layout request reaching the server display
handler, clean helper/server shutdown, and one real loss/recovery cycle that
reaches a fresh `Starting`/`Ready`/`Active` sequence and receives a new
framebuffer. It is substantially stronger than a port-open or refused-port
smoke test. The fixture's explicit CA branch is not enabled by the normal
helper build or package path, so it does not weaken the production candidate's
platform certificate validation.

Run it with:

```text
cargo xtask check-rdp-fixture
```

This closes the local real-server and one controlled reconnect-fixture gaps
only. It does not prove FreeRDP or Windows interoperability, platform
trust-store behavior on Windows/Linux, Gateway interoperability, reconnect
behavior against independent servers, audio, clipboard, or production
packaging.

The helper also revalidates every dynamic resize at the final native command
handler before enqueueing it into IronRDP. Invalid dimensions therefore cannot
mutate the remembered display size or reach the engine even if a future caller
invokes the handler outside the already-validating wire decoder.

The shared helper contract also rejects a display whose raw RGBA framebuffer
would exceed the bounded IPC frame budget. This happens before helper launch
and before a resize reaches the client input queue, so an oversized request
cannot turn into an avoidable native allocation or a late serialization error.

The pinned IronRDP client accepts only 16-bit and 32-bit color depth values.
MobaRust now validates that capability in the shared launch contract, at the
Tauri request boundary, and again in the helper argument parser. An unsupported
value such as 24-bit therefore fails locally with an actionable error before a
process starts; this is capability validation, not evidence of production RDP
interoperability.

The same protocol-aware validation keeps saved RDP profiles from persisting an
audio request while the current helper has no audio backend. The field remains
available in the model for a future reviewed backend, but it is rejected before
launch rather than being silently ignored.

The real compiled helper process now has an integration smoke test. It sends a
typed `Start` frame and a zeroizing credential frame over native pipes, checks
the `Hello`/`Starting`/`Ready` lifecycle, exercises a refused disposable
loopback socket that closes immediately, verifies that the diagnostic does not contain the fixture
secret, and confirms clean process exit. This closes a helper-process lifecycle
gap without pretending that a refused port is RDP interoperability evidence.

## Prototype boundary

The first experiment should package a pinned FreeRDP client helper and expose a
small versioned IPC protocol. The Rust-side contract is now captured in
`mobarust-remote-desktop` and `docs/adr/0013-remote-desktop-helper-wire-contract.md`:

Packaging is currently gated: the isolated IronRDP candidate fails the
separate dependency audit because its pinned `picky` chain contains
`rsa 0.10.0-rc.18` (`RUSTSEC-2023-0071`). The candidate remains available for
repository-local checks but is not staged into normal application bundles.
Reconsider packaging only after selecting a maintained, audited engine or
dependency path.

For development-only parent-process experiments, `cargo xtask stage-rdp-helper`
explicitly builds and stages the candidate under the repository's ignored
helper directory. The normal `stage-helpers`, `package-check`, and release
paths continue to remove or exclude it. This makes the existing Tauri/helper
wire path runnable alongside the explicit local-server fixture without
silently turning the audited candidate into a shipped dependency.

```text
MobaRust Rust core
  -> start / configure / resize / key / pointer / clipboard / stop
  <- connection state / framebuffer regions / diagnostics / clipboard
```

The helper must receive credentials over a protected native channel, never as
command-line arguments or logs. It must have a bounded startup timeout,
cooperative stop, bounded graceful shutdown, and forced termination fallback.
The parent owns the helper process and reports a crash distinctly from a
remote protocol failure.

Control, credential, and event frames now use a dedicated two-second native
pipe-write deadline. A stalled helper or parent cannot keep an RDP lifecycle
task blocked forever; the process supervisor remains responsible for the
bounded stop and forced-reap fallback. This is local IPC evidence, not RDP
interoperability evidence.

If the parent-side input writer fails, the desktop boundary emits a stable
redacted diagnostic, transitions the session to `Crashed`, and invokes the same
bounded helper stop/reap path. This keeps local IPC failure visible without
echoing technical error details or secrets into the UI.

The parent reader also invokes that same bounded stop/reap path after EOF or a
malformed event, so an exited or wedged helper is not left behind when its
session is removed. This is lifecycle hardening, not evidence of a real RDP
server connection.

Reader and writer failure paths claim the stop state atomically, so a single
pipe failure cannot produce duplicate crash events while cleanup is racing.

The desktop canvas uses a framebuffer-aware input map. When `object-fit:
contain` adds letterbox bands, clicks and wheel events in those bands are
discarded instead of being mapped to a misleading remote pixel. Pointer
capture keeps drag events on the canvas, and a release/cancel outside the
painted image reuses the last valid pixel to send an explicit `buttons: 0`.
This is deterministic UI-boundary behavior; it does not replace real RDP
interoperability testing.

Dynamic resize measurements are also fail-closed: a hidden or zero-sized pane
does not produce a synthetic minimum-size resize request, while visible sizes
are rounded and bounded before they reach the native command boundary.

Connection metadata is bounded before helper startup as well: host and domain
are limited to 255 bytes, usernames to 256 bytes, and credential references to
128 bytes. Control characters are rejected in the parent, shared helper
contract, and helper argument parser, including credential references at the
Tauri boundary before vault lookup. Rejections use stable generic messages;
the submitted values are not echoed into diagnostics.

When the desktop window loses focus, the UI releases the last captured pointer
button and all tracked remote scancodes. This is a client-side safety release;
the native helper still owns the actual protocol input and session shutdown.

The shared key contract now encodes RDP set-1 input explicitly: the frontend
maps the standard keys, function keys, lock keys, keypad, Insert/PrintScreen,
and extended keys such as arrows, Home/End, PageUp/PageDown, Delete, and
right-side modifiers. Extended keys carry a dedicated bit and the helper
converts it into IronRDP's `KeyboardFlags::EXTENDED`. Invalid values are
rejected before they reach the engine. This is protocol-correct input mapping
at the boundary; it still needs validation against real Windows keyboard
layouts.

High-frequency pointer moves are coalesced only while adjacent and pending;
pointer-down, pointer-up, cancel, and button-state transitions retain order.
The browser-side queue is also capped at 128 items. When saturated, stale
motion is evicted first; if only transitions remain, new non-release
transitions are rejected and an explicit release replaces the oldest event.
This keeps stale cursor positions from filling the native command queue without
dropping the events that define a click or release.

## Acceptance gates

Before calling RDP implemented, use a secure engine/backend to test a real
server with hostname, port, username, password, domain, certificate
validation, resolution, dynamic resize, keyboard, mouse, clipboard, fullscreen,
scaling, reconnect, color depth, audio configuration, gateway behavior, and
connection diagnostics.
At least one Windows interoperability check is mandatory because Windows is
the primary MobaXterm audience. No screenshot or static framebuffer counts.
