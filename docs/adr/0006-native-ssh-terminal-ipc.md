# ADR 0006: Run SSH shells as native session tasks

## Status

Accepted for the first SSH desktop adapter.

## Decision

`mobarust-ssh` owns connection, host-key, authentication, PTY, and channel
semantics. The Tauri layer owns only a map of opaque terminal IDs to bounded
command queues. Each connected shell runs as one native task with independent
read and write halves:

```text
React xterm -> typed ssh_write / ssh_resize / ssh_close -> bounded Rust queue
                                                          |
                                      russh PTY read/write halves
                                                          |
React xterm <- ssh://output / ssh://closed <- native task
```

Remote output is emitted as text events only; xterm treats the data as
terminal bytes rather than HTML. The output chunks are capped at 32 KiB and
the command queue at 64 messages. Connection setup uses a 12-second
operation-specific timeout. Unknown host keys remain rejected by default.

## Consequences

The remote shell is real and interactive, including resize and cooperative
close. A disconnect removes the native task and emits an explicit closed
event. A future reconnect layer must preserve the session identity while
making shell-state loss visible.

The first adapter supports password credentials referenced from the native
vault, private-key files with optional vault-backed passphrases, the local SSH
agent, and a bounded keyboard-interactive mode backed by one vault response.
The keyboard-interactive mode refuses echo-enabled prompts and does not expose
server prompt text to the renderer. It does not yet expose file
transfers, or unlimited scrollback replay across reconnects. A bounded attach
buffer protects the prompt and first output while xterm subscribes; the
remaining transfer and reconnect concerns are release gates before claiming
complete SSH UX. A read-only SFTP directory listing is available as a first
subordinate channel operation.
