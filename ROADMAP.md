# MobaRust roadmap

The roadmap is ordered by operator value and evidence, not by protocol count.

## 0.1 — local workstation foundation

- [x] Rust workspace with explicit connection and transfer state models
- [x] Rust-owned local PTY with resize, input, output batching, and clean exit
- [x] Tauri shell and xterm.js operator workspace
- [x] Local validation command and architecture/research records
- [ ] Cross-platform PTY matrix on Windows, Linux, and macOS

## 0.2 — SSH vertical slice

- [x] Host-key verification and known_hosts policy
- [x] Interactive SSH with PTY resize and cooperative close
- [x] Agent, password-reference, and private-key-reference auth in the saved-session UX
- [ ] Encrypted-key passphrase entry and keyboard-interactive auth in the saved-session UX
- [x] SSH agent authentication through the native Quick Connect path
- [ ] Reconnect policy and failure-state telemetry
- [x] Local integration fixture for authentication, resize, PTY I/O, SFTP, and disconnect

## 0.3 — remote files and movement

- [x] SFTP browser with listing, create-folder, rename, delete, and single-file streaming/cancellation/progress
- [ ] SCP compatibility path
- [x] Bounded native transfer manager (three concurrent single-file jobs)
- [ ] Remote terminal and file browser composition

## 0.4 — network workstation

- [x] Local SSH forwarding through native direct-tcpip channels with bounded clients and cancellation
- [ ] Remote and dynamic SSH forwarding
- [ ] Jump hosts and multi-hop chains
- [ ] Session folders, tags, search, favorites, editing, and full import/export
- [x] OpenSSH config import for common secret-free fields with an explicit compatibility report
- [x] OS credential vault abstraction
- [ ] Portable encrypted vault research and implementation

## Later protocol adapters

RDP, VNC, Telnet, Serial, X11 forwarding, monitoring, and portable packaging follow after the SSH and transfer foundations have production evidence. Each adapter must have a real lifecycle, failure tests, and an honest platform support statement before it is presented as complete.
