# MobaRust isolated RDP helper

This is a separate Cargo workspace for evaluating IronRDP behind the
`mobarust-remote-desktop` native-process contract. It is not included in the
main Cargo workspace because its prerelease crypto dependency conflicts with
the application's portable-vault dependency.

The helper accepts only non-secret connection metadata in argv. The password
is accepted only as a zeroizing framed native-pipe payload. It does not read
`~/.ssh`, use `SSH_AUTH_SOCK`, read the MobaRust vault, inspect personal files,
or contact a host until a caller sends both an explicit `Start` command and a
credential frame.

TLS key logging is refused even for direct helper launches. The helper rejects
an inherited `SSLKEYLOGFILE` variable because TLS key material must never be
written to an external log file.

It also rejects ambient `SSL_CERT_FILE` and `SSL_CERT_DIR` overrides. Tests and
future production configuration must use an explicit, reviewed certificate
policy rather than silently reading an arbitrary user-selected path.

This candidate is not staged into normal application bundles. Its separate
lockfile currently fails the local audit because IronRDP's pinned `picky`
dependency chain includes `rsa 0.10.0-rc.18` (`RUSTSEC-2023-0071`). It must
pass a fresh audit before packaging or production claims.

Connector failures are emitted as redacted categories rather than forwarding
IronRDP's internal context or server text. The published `ironrdp-tls 0.2.2`
implementation was audited locally and its TLS backends do not validate server
identity. The helper therefore patches that dependency inside this isolated
workspace with `ironrdp-tls-validated`, a small compatibility crate that uses
Rustls, SNI, and `rustls-platform-verifier`. This improves the candidate's
trust behavior but is not production evidence: RDP hostname/IP targets are
accepted only through this native platform-verification path, and the helper
remains excluded from normal bundles.

The candidate also supports explicit RD Gateway metadata
(`--gateway-endpoint` and `--gateway-username`). The gateway password is sent
separately from the session password as a role-tagged zeroizing native-pipe
frame. Neither secret is accepted in argv or written to logs. Gateway
configuration is opt-in and must be complete at the typed desktop boundary;
it remains an experiment until real Gateway interoperability, platform trust
fixtures, and the dependency audit are complete.

RDP clipboard redirection is disabled by default and can be requested only by
the explicit `--clipboard-enabled` argument from an opted-in profile. On
Windows, this selects IronRDP's native OS clipboard backend. On macOS/Linux,
the helper rejects the request before connecting because the pinned client
falls back to a stub backend there. The WebView clipboard payload is not used
as a second authority, and remote clipboard content is never written into the
Mac clipboard automatically.

The helper now accepts explicit hostname or IP metadata and passes it unchanged
to the native TLS adapter, which owns DNS, SNI, and platform certificate
verification. The same native `ServerName` parser rejects malformed targets
before a socket is opened, without echoing the submitted value. Untrusted or
invalid certificates fail closed. This candidate behavior is still subject to
real interoperability, dependency, and packaging evidence; local tests
continue to use only disposable loopback fixtures.

On macOS, `platform_tls_rejects_a_self_signed_loopback_certificate` generates a
short-lived synthetic certificate and key, serves it with a disposable local
OpenSSL fixture, and verifies that the platform trust verifier rejects it. The
test does not read personal certificates or contact a remote host. Equivalent
Windows/Linux certificate-store fixtures remain pending, and this test proves
TLS trust rejection only—not compatibility with a real RDP server.

Promotion requires an audited engine/backend with real certificate-chain and
hostname validation (or an explicit, reviewed pinning policy), deterministic
certificate fixtures, and Windows interoperability evidence. The candidate
remains excluded from normal bundles until those gates and the known RSA
advisory are resolved.

Audio redirection is not implemented. The desktop boundary and the helper
both reject an audio request explicitly; it is never silently discarded.

The helper applies a 15-second startup timeout to a stalled RDP handshake and
then uses a separate bounded graceful-stop window before forcing termination.
This prevents an unresponsive endpoint from leaving the helper permanently
stuck. After an active session loss it retries three times with exponential
backoff, and Stop cancels both the delay and the current attempt. Timing and
stalled-handshake behavior are covered locally; no RDP server interoperability
is claimed by these tests.

Resize commands are validated again inside the helper before they are sent to
IronRDP, and the requested display state is updated only after that enqueue
succeeds. If the RDP input channel closes, the helper emits a stable lifecycle
failure and uses the bounded stop/reconnect path rather than exposing a raw
channel error.

Keyboard input uses a shared set-1 scan-code contract. Extended keys carry an
explicit marker across the native boundary and become IronRDP's
`KeyboardFlags::EXTENDED`; malformed or out-of-range values are rejected
without reaching the engine. Real Windows keyboard-layout interoperability is
still a required follow-up test.

Safe local checks:

```text
cargo xtask check-rdp-helper
cargo run --manifest-path tools/rdp-helper/Cargo.toml -- \
  --mobarust-protocol rdp --host 127.0.0.1 --port 3389 --username fixture-user < /dev/null
```

The EOF smoke test never opens a network connection. A real interoperability
test must use a disposable local fixture or a separately approved test host.

The `local_process` integration test starts the real helper binary, sends the
typed `Start` command and zeroizing credential frame through native pipes,
checks `Hello`/`Starting`/`Ready`, and verifies a redacted bounded terminal
outcome plus clean process exit against a disposable loopback socket that
closes immediately. It proves the
helper-process boundary and cancellation/exit behavior; it does not prove
compatibility with a real RDP server.

The same fixture suite sends the session credential and the role-tagged Gateway
credential as two ordered native frames. The helper waits for both before
starting the candidate transport, and the closed loopback Gateway outcome
proves that the second frame is not mistaken for invalid input. Both fixture
secrets are checked against diagnostics; this is ordering/redaction evidence,
not Gateway interoperability evidence.

An additional opt-in `local-rdp-fixture` feature runs the real helper against
the official `ironrdp-server` implementation on `127.0.0.1`. It creates a
short-lived private CA and server certificate in a disposable temporary
directory, passes that CA only to the test-feature client, and never modifies
the macOS trust store. The fixture verifies the actual TLS/Hybrid handshake,
authentication, decoded framebuffer, keyboard and mouse input, and clean
Stop lifecycle. It is intentionally excluded from the normal helper check and
from every package path:

```text
cargo xtask check-rdp-fixture
```

This is stronger than a refused-port smoke test, but it remains local
interoperability evidence only. Windows/Linux certificate stores, real Windows
servers, Gateway interoperability, reconnect recovery, and production engine
selection remain open gates.
