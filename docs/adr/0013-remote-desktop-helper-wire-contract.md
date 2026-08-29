# ADR 0013: Version the remote-desktop helper boundary before engine integration

## Status

Accepted. The Rust-side contract is implemented; FreeRDP/VNC engine
integration remains pending.

## Context

RDP and VNC are protocol families with substantial native dependencies,
platform glue, input handling, codecs, clipboard behavior, and failure modes.
The current development machine has no `xfreerdp`, `vncviewer`, FreeRDP
`pkg-config` package, or libvncclient installation. Installing system
dependencies or building a native engine globally is outside this experiment
and would create unnecessary risk for the host machine.

## Decision

MobaRust will keep protocol engines behind a controlled helper process. The
Rust crate `mobarust-remote-desktop` owns the versioned boundary and validates
messages before a process is started:

- launch arguments contain host metadata only, never passwords, credential
  references, private keys, or tokens;
- credentials are handed over through a separate native channel after helper
  startup;
- control messages are typed (`start`, `stop`, `resize`, keyboard, pointer,
  and clipboard);
- events are typed (hello, lifecycle state, framebuffer, clipboard, and
  bounded diagnostics);
- JSON frames have a four-byte big-endian length prefix and an 8 MiB maximum;
- clipboard input is capped at 1 MiB;
- helper lifecycle distinguishes protocol failure, crash, cancellation, and
  clean stop;
- debug formatting redacts opaque credential references and clipboard text.

The crate is a contract and test seam, not a fake RDP/VNC implementation. The
desktop UI will not advertise RDP or VNC as available until a real helper is
packaged and exercised against a real server.

## Consequences

The process boundary can contain native crashes and allows bounded graceful
shutdown followed by forced termination. It also adds packaging, IPC, and
framebuffer-copy work. A real engine experiment must measure input latency,
resize behavior, clipboard, reconnect, audio, certificate handling, and
Windows interoperability before the adapter is promoted.

The protocol-independent contract tests run without spawning a process or
reading host configuration. A future integration fixture must use a dedicated
temporary directory and local test server.

