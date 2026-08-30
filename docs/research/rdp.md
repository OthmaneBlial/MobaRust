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
- An isolated disposable Cargo probe successfully compiled and instantiated
  `ironrdp-client 0.1.0` with synthetic configuration, without opening a
  socket. Adding it directly to the main workspace was deliberately reverted:
  its `picky` dependency pins `aes-gcm 0.11.0-rc.4`, which conflicts with the
  portable vault's `aes-gcm 0.11.1`. The vault dependency was not weakened for
  this experiment.

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

## IronRDP candidate result

The isolated `tools/rdp-helper` adapter confirms that a Rust-native candidate
can be placed behind the helper boundary with a reusable `RdpClient`, typed
image output, keyboard/mouse/resize input, TLS/CredSSP configuration, and a
zeroizing native credential frame. Its clipboard command is intentionally
rejected until a user-controlled OS clipboard backend is wired. This is still
not a production selection: certificate policy, reconnect, audio, gateway
behavior, packaging, and real Windows interoperability remain open gates. No
global package, personal credential, or remote server was used during the
local validation.

Connector failures are reduced to stable categories at the helper boundary,
including authentication/access rejection, protocol negotiation, malformed
data, and TLS/certificate-or-transport validation. This is diagnostic
hardening only: the pinned IronRDP rustls backend still uses a no-op
certificate verifier, so certificate validation remains a promotion blocker.
The helper also refuses an inherited `SSLKEYLOGFILE` variable; the TLS backend's
key-log facility must not write session key material during local experiments.

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

## Acceptance gates

Before calling RDP implemented, test a real server with hostname, port,
username, password, domain, certificate validation, resolution, dynamic
resize, keyboard, mouse, clipboard, fullscreen, scaling, reconnect, color
depth, audio configuration, gateway behavior, and connection diagnostics.
At least one Windows interoperability check is mandatory because Windows is
the primary MobaXterm audience. No screenshot or static framebuffer counts.
