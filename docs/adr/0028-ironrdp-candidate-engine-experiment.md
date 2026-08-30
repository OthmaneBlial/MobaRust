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

The local workspace uses Rust 1.95. The candidate `ironrdp-client` release is
MIT/Apache-2.0 and exposes a reusable client with image output and typed input
events. This is an engineering evaluation, not a claim of interoperability.

## Decision

Keep the IronRDP adapter in the separate Cargo workspace at
`tools/rdp-helper`. It uses `ironrdp-client 0.1.0`, `ironrdp-pdu 0.9.0`, and
the `native-tls`/`clipboard` features. The helper:

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
bridging the local clipboard.

The helper now owns a bounded reconnect policy after an active-session loss:
three attempts with exponential backoff, native credential reuse, and
cooperative cancellation during the delay. This is lifecycle hardening only;
the separate local/Windows fixture must prove that a real RDP session recovers
before the feature can be promoted.

The selected `ironrdp-tls 0.2.2` native-tls backend delegates certificate-chain
and hostname validation to the platform connector and trust store. The helper
does not yet expose a deliberate self-signed acceptance or certificate-pinning
policy, so this trust-policy UX remains a promotion gate. The dependency audit,
reconnect, audio, gateway, packaging, and real Windows interoperability gates
also remain open.

This choice trades the intentionally disabled verifier in the rustls path for
platform TLS dependencies: Schannel on Windows, Security Framework on macOS,
and the native-tls/OpenSSL path on Linux. The eventual distribution matrix must
package and audit those runtime requirements; the candidate remains excluded
from normal bundles until that work and the existing RSA advisory are resolved.

## Verification and next gate

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

The next gate is parent-process wiring plus a disposable local/Windows RDP
fixture, with a real framebuffer and controlled input, including a real loss
and recovery cycle. Until that evidence and packaging exist, the UI must not
advertise RDP as implemented.
