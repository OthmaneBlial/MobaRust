# ADR 0014: Keep SCP as a bounded native compatibility primitive

## Status

Accepted and implemented for bounded single-file jobs in the native SSH crate
and desktop transfer manager.

## Decision

MobaRust supports legacy SCP protocol framing through the existing `russh`
session boundary for single-file upload and download. Rust owns the remote
command, control acknowledgements, bounded byte buffers, local I/O, progress,
cancellation, and cleanup. The React renderer does not receive SCP bytes or an
arbitrary shell command capability. The transfer manager exposes an explicit
SFTP/SCP choice and defaults to SFTP; recursive transfers are rejected for SCP
and remain on the bounded SFTP walker.

The remote command is a fixed `scp -O -t/-f` operation with a shell-quoted
remote path. `-O` explicitly selects the legacy SCP protocol on OpenSSH
versions whose default `scp` mode is SFTP. The path validator rejects control
characters and empty paths; it does not turn this API into general remote
command execution.

SCP downloads commit through a local temporary file and a guarded atomic
replacement. The destination must not be a symlink, and Windows uses the OS
replace-existing move without deleting the old file first. SCP uploads stream
to a uniquely named remote temporary file and commit through the existing
native SFTP rename operation, preserving the destination on failed or cancelled
jobs. This requires the remote SFTP subsystem for the SCP manager
path; hosts that expose only legacy SCP must use a future compatibility mode
with a clearly weaker atomicity guarantee.

## Security and reliability boundaries

- SSH host-key and credential policy remains the same as the parent connection.
- File contents stream through a bounded 64 KiB buffer; they are not collected
  in memory or sent through the webview.
- SCP control lines have a fixed maximum size and declared file sizes are
  parsed as bounded protocol metadata.
- Remote paths are validated and shell-quoted before entering the fixed SCP
  command template.
- The fixture uses generated credentials, a temporary host key, an ephemeral
  port, and a temporary known_hosts file. It never consults the developer's
  home SSH configuration or SSH agent.

## Verification

`crates/mobarust-ssh/tests/local_sshd.rs` uploads and downloads a 128 KiB
fixture through the local OpenSSH server and verifies the exact bytes. The
same fixture also covers host-key rejection, key authentication, PTY I/O,
SFTP, forwarding, and jump-host behavior.

## Follow-ups

- decide whether SFTP should remain the default for all normal file movement;
- add failure-injection coverage for SCP manager commit failures;
- add platform interoperability checks where a real OpenSSH server is
  available in a controlled test environment.
