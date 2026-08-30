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

This candidate is not staged into normal application bundles. Its separate
lockfile currently fails the local audit because IronRDP's pinned `picky`
dependency chain includes `rsa 0.10.0-rc.18` (`RUSTSEC-2023-0071`). It must
pass a fresh audit before packaging or production claims.

Connector failures are emitted as redacted categories rather than forwarding
IronRDP's internal context or server text. The helper selects IronRDP's
`native-tls` backend, so certificate chains and hostnames are checked through
the operating-system trust store. Explicit self-signed acceptance or pinning
policy is not wired yet; this remains an isolated candidate and must not be
treated as a production RDP client.

The helper applies a 15-second startup timeout to a stalled RDP handshake and
then uses a separate bounded graceful-stop window before forcing termination.
This prevents an unresponsive endpoint from leaving the helper permanently
stuck. The behavior is covered by a loopback-only stalled-handshake unit test.

Safe local checks:

```text
cargo xtask check-rdp-helper
cargo run --manifest-path tools/rdp-helper/Cargo.toml -- \
  --mobarust-protocol rdp --host 127.0.0.1 --port 3389 --username fixture-user < /dev/null
```

The EOF smoke test never opens a network connection. A real interoperability
test must use a disposable local fixture or a separately approved test host.
