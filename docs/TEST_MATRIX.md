# Test matrix

Kinds of evidence, from strongest to weakest:

- **Real local** — real protocol traffic against a throwaway server on
  loopback (for example `/usr/sbin/sshd` started by the test with its own
  keys, config and temp home). Cleaned up by the test.
- **Unit / fixture** — pure logic with fixtures; no network.
- **Browser preview** — the real React UI rendered by Vite without the Tauri
  runtime. Layout only; no IPC.
- **Native** — the packaged or debug Tauri app driven on macOS.

Nothing simulated is listed as real end-to-end.

| Area | Evidence (2026-11-19, this Mac) | Gaps |
|---|---|---|
| SSH auth (key, agent, password, keyboard-interactive, wrong credentials) | Real local: `authentication.rs`, `local_sshd.rs` | Native UI prompts not re-checked this round |
| SSH PTY shell, resize, startup command, env | Real local | — |
| 14 concurrent SSH sessions (group open) | Real local: 14 sessions, bound 4, connect 209 ms, shells done 496 ms | Native "Open all" not driven yet |
| Reconnect after server restart | Real local: `interrupted_transport_recovers_after_the_fixture_server_restarts` | — |
| Handshake timeout / cancel | Real local | — |
| SFTP / SCP transfers, interruption and retry, permissions, links | Real local: `local_sshd.rs`, `sftp_bounds.rs`, `sftp_metadata.rs` | — |
| Remote text editing (encodings, conflicts, permissions) | Real local: `remote_editor.rs` | — |
| Tunnels (local, remote, dynamic) and cleanup | Real local: `forwarding.rs` | — |
| Jump hosts (IPv4/IPv6, rekey) | Real local | — |
| X11 forwarding | Real local to a loopback display fixture; Xvfb test skipped when absent | — |
| Telnet | Unit tests in `mobarust-telnet` | No local Telnet server on macOS; loopback fixture to add |
| Serial | Real local PTY pair: `local_pty.rs` | No hardware |
| RDP / VNC | Helper unit and fixture tests | No local RDP/VNC server; not verified end-to-end |
| MobaXterm import | Unit / fixture | No real MobaXterm export available |
| `--open-group` parsing and resolution | Unit | Native launch not tested |
| Terminal-first layout, focus mode, tab menu | Browser preview at 1440×900 and 1100×680 | Native screenshots blocked (no Screen Recording permission for the shell) |
| Linux / Windows | Not run this round | Existing docs record earlier cross-checks |
