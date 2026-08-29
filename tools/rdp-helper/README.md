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

Safe local checks:

```text
cargo xtask check-rdp-helper
cargo run --manifest-path tools/rdp-helper/Cargo.toml -- \
  --mobarust-protocol rdp --host 127.0.0.1 --port 3389 --username fixture-user < /dev/null
```

The EOF smoke test never opens a network connection. A real interoperability
test must use a disposable local fixture or a separately approved test host.
