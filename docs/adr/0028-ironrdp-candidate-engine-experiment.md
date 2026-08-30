# ADR 0028: Evaluate IronRDP behind the existing desktop helper boundary

## Status

Validated as an isolated helper experiment and deferred from the main
workspace. RDP is not a shipped feature.

## Context

FreeRDP remains a mature Apache-2.0 C engine, but its ABI, plugin, codec, and
platform packaging surface is large. The repository already has a versioned
helper contract and supervisor, so a candidate engine must first prove that it
can provide a native event seam without leaking into React or requiring a
global installation.

The local workspace uses the repository's supported Rust stable toolchain. The candidate `ironrdp-client` release is
MIT/Apache-2.0 and exposes a reusable client with image output and typed input
events. This is an engineering evaluation, not a claim of interoperability.

## Decision

Keep the IronRDP adapter in the separate Cargo workspace at
`tools/rdp-helper`. It uses `ironrdp-client 0.1.0`, `ironrdp-pdu 0.9.0`, and
the `rustls`/`clipboard` features. A repository-local compatibility crate at
`tools/rdp-helper/ironrdp-tls-validated` supplies the TLS API expected by
IronRDP while delegating certificate verification to the platform verifier.
The helper:

- accepts only host metadata in process arguments;
- receives the password through the versioned zeroizing native-pipe frame;
- constructs an IronRDP `ConfigBuilder` with TLS and CredSSP enabled;
- maps real IronRDP image, keyboard, mouse, resize, lifecycle, and clean-stop
  events to the helper contract;
- maps connector failures to redacted authentication/access, protocol,
  malformed-data, and TLS/certificate-or-transport categories;
- never reads the local vault, accesses the SSH agent, or touches personal
  files;
- remains separate from the main workspace because IronRDP's `picky`
  dependency pins `aes-gcm 0.11.0-rc.4`, while the portable vault uses
  `aes-gcm 0.11.1`.

The vault crypto was not changed to accommodate the RDP experiment. Passwords
must arrive through a protected native channel, never as process arguments,
environment variables, logs, or frontend state. Trust/pinning policy,
reconnect interoperability, clipboard, audio, gateway support, packaging, and
Windows interoperability are still release gates. The current helper
deliberately reports clipboard input as unsupported rather than silently
bridging the local clipboard. RDP target metadata is passed unchanged to the
native TLS boundary; the helper does not resolve targets in React or use a
frontend-side trust decision.

The clipboard feature was checked against the pinned IronRDP source before
making this boundary decision. `ClipboardType::Enable` selects a native
`WinClipboard` backend on Windows, while the non-Windows implementation is a
stub. The feature flag alone is therefore insufficient for the macOS and Linux
targets. Keeping `ClipboardType::Stub` makes the limitation explicit and
prevents a false cross-platform capability claim. Any future clipboard adapter
must be platform-specific, user-controlled, bounded to safe text operations,
and isolated from automatic local clipboard writes.

The helper now owns a bounded reconnect policy after an active-session loss:
three attempts with exponential backoff, native credential reuse, and
cooperative cancellation during the delay. This is lifecycle hardening only;
the separate local/Windows fixture must prove that a real RDP session recovers
before the feature can be promoted.

The published `ironrdp-tls 0.2.2` implementation was audited locally after the
initial experiment. Its `native-tls` builder calls
`danger_accept_invalid_certs(true)` and disables SNI; its published Rustls path
also uses a no-certificate-verification implementation. The local compatibility
crate replaces that behavior with `rustls-platform-verifier` and SNI. This is
a security improvement for the isolated candidate, not production evidence:
the helper accepts hostname/IP metadata only through that native verification
path and remains excluded from normal bundles until real certificate fixtures,
Windows interoperability, dependency audit, and packaging checks pass. RD
Gateway remains deferred until its separate transport path has the same trust
policy.

The helper enforces a fail-closed trust boundary at runtime: it rejects
inherited `SSL_CERT_FILE`, `SSL_CERT_DIR`, and `SSLKEYLOGFILE` settings, then
passes the explicit target to the native adapter for DNS, SNI, and platform
certificate verification. Invalid or untrusted certificates are rejected.
Local tests still use only literal loopback listeners and never contact a real
remote host.

On macOS, `platform_tls_rejects_a_self_signed_loopback_certificate` creates a
short-lived synthetic self-signed certificate, serves it through a disposable
OpenSSL process on `127.0.0.1`, and verifies that the platform trust verifier
rejects it for the matching server name. It does not read personal
certificates or contact a remote host. This proves the candidate's TLS trust
decision only; Windows/Linux certificate-store fixtures and real RDP-server
interoperability remain open gates.

This remains a hard security gate. Promotion requires deterministic certificate
fixtures, an audited engine/backend with real certificate-chain and hostname
validation, and Windows interoperability evidence. The eventual distribution
matrix must also audit and package the platform TLS requirements (Windows
certificate APIs, Security Framework on macOS, and the platform verifier's
Linux trust sources). The candidate remains excluded from normal bundles until
those requirements and the existing RSA advisory are resolved.

## Verification and next gate

The architecture decision is intentionally broader than the current Rust
candidate. FreeRDP FFI/bindings offer the strongest mature-engine and native
performance path but carry the largest ABI, plugin, codec, and packaging
surface. Dynamic linking reduces the application footprint but introduces
runtime discovery and ABI drift. A controlled helper plus framebuffer bridge
keeps crashes, credentials, cancellation, and untrusted pixels outside the
React process at the cost of measurable IPC/copy latency. Native window
embedding may reduce that cost later, but requires separate Win32, X11/Wayland,
and AppKit lifecycle work. The comparative criteria and qualitative ratings
are recorded in `docs/research/rdp.md`; they are not benchmark results.

The isolated helper is validated locally with:

```text
cargo xtask check-rdp-helper
cargo run --manifest-path tools/rdp-helper/Cargo.toml -- \
  --mobarust-protocol rdp --host 127.0.0.1 --port 3389 --username fixture-user < /dev/null
```

The first command covers formatting, unit tests, and clippy. The second emits
only the helper lifecycle handshake and exits on EOF; it does not open a socket.
The helper's real connection path is still exercised only against a dedicated
fixture when one is available.

The next gate is a disposable local/Windows RDP fixture, with a real
framebuffer and controlled input, including a real loss and recovery cycle.
Until that evidence and packaging exist, the UI must not advertise RDP as
implemented.
