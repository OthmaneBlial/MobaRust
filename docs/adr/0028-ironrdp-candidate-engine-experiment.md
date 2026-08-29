# ADR 0028: Evaluate IronRDP behind the existing desktop helper boundary

## Status

Validated as an isolated compile-only experiment and deferred from the main
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

Run a disposable Cargo project outside the MobaRust workspace with
`ironrdp-client 0.1.0`, `ironrdp-pdu 0.9.0`, and the `rustls`/`clipboard`
features. The experiment:

- constructs an IronRDP `ConfigBuilder` from synthetic host/port/user/password
  inputs;
- creates the client image/input event channels;
- does not open a socket, spawn a helper, read the local vault, access the SSH
  agent, or touch any personal file;
- keeps the existing helper boundary as the intended production process
  boundary.

An attempted optional dependency in the main workspace was reverted because
IronRDP's `picky` dependency pins `aes-gcm 0.11.0-rc.4`, while MobaRust's
portable vault intentionally uses `aes-gcm 0.11.1`. The vault crypto was not
changed to accommodate an unvalidated RDP experiment.

The helper must still own the eventual engine configuration and secret
handling. Passwords must arrive through a protected native channel, never as
process arguments, environment variables, logs, or frontend state. Certificate
validation, reconnect, framebuffer conversion, keyboard/mouse mapping,
clipboard, resizing, audio, gateway support, and Windows interoperability are
still release gates.

## Verification and next gate

The isolated probe was run with:

```text
cargo run --manifest-path /tmp/mobarust-ironrdp-probe.XXXXXX/Cargo.toml
```

It completed successfully after adding the required synthetic
`MajorPlatformType::UNSPECIFIED` field. This proves API compatibility only;
the temporary project and its generated build artifacts are outside the
repository and were not used as application code.

The next experiment must be a dedicated helper executable and a disposable
local/Windows RDP fixture, with a real framebuffer and controlled input. Until
that evidence exists, the UI must not advertise RDP as implemented.
