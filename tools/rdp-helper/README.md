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
trust behavior but is not production evidence: the helper remains loopback-only
and excluded from normal bundles. RD Gateway is also deferred until its
separate transport path has the same trust policy.

The helper enforces this boundary at runtime during the experiment: it accepts
only literal loopback IP targets (`127.0.0.1` or `::1`) and rejects hostnames and
all other addresses before opening a socket. The local TLS adapter validates
the presented certificate when a handshake is attempted, but this restriction
remains until real interoperability evidence exists.

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

Safe local checks:

```text
cargo xtask check-rdp-helper
cargo run --manifest-path tools/rdp-helper/Cargo.toml -- \
  --mobarust-protocol rdp --host 127.0.0.1 --port 3389 --username fixture-user < /dev/null
```

The EOF smoke test never opens a network connection. A real interoperability
test must use a disposable local fixture or a separately approved test host.
