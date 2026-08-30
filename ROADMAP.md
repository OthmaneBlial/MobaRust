# MobaRust roadmap

The roadmap is ordered by operator value and evidence, not by protocol count.

## 0.1 — local workstation foundation

- [x] Rust workspace with explicit connection and transfer state models
- [x] Rust-owned local PTY with resize, input, output batching, and clean exit
- [x] Tauri shell and xterm.js operator workspace
- [x] Persistent terminal tabs for local and remote sessions with per-tab lifecycle routing
- [x] Two-pane horizontal/vertical terminal splits with explicit close semantics
- [x] Nested splits, drag-resizing, and richer pane focus management
- [x] Local validation command and architecture/research records
- [ ] Cross-platform PTY matrix on Windows, Linux, and macOS

## 0.2 — SSH vertical slice

- [x] Host-key verification and known_hosts policy
- [x] Interactive SSH with PTY resize and cooperative close
- [x] Agent, password-reference, and private-key-reference auth in the saved-session UX
- [x] Encrypted-key passphrase entry and keyboard-interactive auth in the saved-session UX
- [x] SSH agent authentication through the native Quick Connect path
- [x] Bounded reconnect policy and failure-state telemetry
- [x] Local integration fixture for authentication, resize, PTY I/O, SFTP, and disconnect

## 0.3 — remote files and movement

- [x] SFTP browser with listing, create-folder, rename, delete, and single-file streaming/cancellation/progress
- [x] Native SCP compatibility primitive for streaming single-file upload/download
- [x] SCP transfer-manager wiring for bounded single-file jobs with progress, cancellation, and atomic commits
- [x] Bounded recursive SFTP upload/download with streaming files, progress, cancellation, and atomic file commits
- [x] Bounded native transfer manager (three concurrent single-file jobs)
- [x] Remote terminal and file browser composition

## 0.4 — network workstation

- [x] Local and remote SSH forwarding through native direct-tcpip/forwarded-tcpip channels with bounded clients and cancellation
- [x] Bounded local SOCKS5 `-D` proxy path with explicit tunnel-manager controls
- [x] Native remote and dynamic forwarding transport primitives
- [x] Local, remote, and dynamic forwarding manager UI with explicit direction labels and stop controls
- [x] Native jump-host chain transport with host-key policy per hop
- [x] Bounded shell reconnect attempts with explicit lifecycle telemetry
- [x] Resolve imported OpenSSH `ProxyJump` aliases into reconnectable saved profiles when matching saved hop records exist
- [x] Session tags, search, favorites, recents, and secret-free MobaRust import/export
- [x] Session folders and metadata-only profile editing/deletion
- [x] OpenSSH config import for common secret-free fields with an explicit compatibility report
- [x] OS credential vault abstraction
- [x] Explicit native vault reference save/delete commands with transient secret entry
- [x] Typed non-secret settings with validation, atomic persistence, reset, and terminal application
- [x] Secret-free snippets with tags, validated variables, rendered preview, and explicit manual copy
- [x] Portable encrypted vault backend with marker-gated data directory, explicit unlock/lock, and native-only lookup
- [ ] Signed portable distribution/package matrix

## Later protocol adapters

- [x] RDP/VNC helper-boundary research and versioned Rust-side contract
- [x] Isolated IronRDP helper with native credential handoff, lifecycle, input, and framebuffer bridge
- [ ] Real FreeRDP integration with a controlled helper and Windows evidence
- [ ] Real VNC integration with a mature engine and local/manual fixtures
- [x] Native Telnet transport with bounded negotiation and a local TCP fixture
- [x] Telnet Quick Connect, native session manager, terminal output, input, and resize wiring
- [x] Native serial transport configuration and recoverable device lifecycle
- [x] Serial Quick Connect, native session manager, terminal output, and input wiring
- [x] Explicit serial device refresh and secret-free saved serial profiles
- [ ] Hardware interoperability matrix
- [x] Bounded native TCP check and port-range diagnostic primitive
- [x] DNS resolution, bounded TCP checks, and explicit diagnostics UI
- [x] Bounded port-scan UI with progress and cancellation
- [x] Bounded platform-native ping and traceroute with cancellation and output limits
- [x] Explicit unauthenticated SSH fingerprint inspection with no credential or known_hosts access
- [ ] X11 forwarding and integrated/external X-server strategy
- [x] One-shot SSH remote monitoring with capability-aware metrics and bounded collection
- [ ] Signed portable packaging and cross-platform distribution evidence

## Operator workflows

- [x] Bounded terminal macros with visible confirmation, cancellable execution, and typed permission boundaries
- [x] Explicit multi-exec/broadcast mode with selected targets, strong indicator, whole-event preflight, and emergency disable
- [x] Macro recording, per-action approval policies, and browser stress testing
- [x] Bounded UTF-8 remote text editing with conflict detection and rollback-safe temporary-file promotion
- [x] Editor syntax highlighting, search/replace, encoding selection, and save-as workflow

Each adapter must have a real lifecycle, failure tests, and an honest platform
support statement before it is presented as complete.
