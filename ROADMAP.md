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
- [ ] Password, key, encrypted key, agent, and keyboard-interactive auth in the saved-session UX
- [ ] Reconnect policy and failure-state telemetry
- [x] Local integration fixture for authentication, resize, PTY I/O, SFTP, and disconnect

## 0.3 — remote files and movement

- [ ] SFTP browser with streaming, cancellation, conflict handling, and progress
- [ ] SCP compatibility path
- [ ] Bounded global transfer manager
- [ ] Remote terminal and file browser composition

## 0.4 — network workstation

- [ ] Local, remote, and dynamic SSH forwarding
- [ ] Jump hosts and multi-hop chains
- [ ] Session folders, tags, search, favorites, and import/export
- [ ] OS credential vault abstraction and portable encrypted vault research

## Later protocol adapters

RDP, VNC, Telnet, Serial, X11 forwarding, monitoring, and portable packaging follow after the SSH and transfer foundations have production evidence. Each adapter must have a real lifecycle, failure tests, and an honest platform support statement before it is presented as complete.
