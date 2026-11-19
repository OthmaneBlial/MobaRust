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

Run all real-localhost suites with cleanup check: `./tools/local-protocol-lab.sh`
(2026-11-19 runs: all passed. Latest run after the import work: 14-session group connected in 558 ms, shells done in 870 ms under full-suite load; no fixture `sshd` left).

| Area | Evidence (2026-11-19, this Mac) | Gaps |
|---|---|---|
| SSH auth (key, agent, password, keyboard-interactive, wrong credentials) | Real local: `authentication.rs`, `local_sshd.rs` | Native UI prompts not re-checked this round |
| SSH PTY shell, resize, startup command, env | Real local | — |
| 14 concurrent SSH sessions (group open) | Real local: 14 sessions, bound 4, connect 209 ms, shells done 496 ms | Native picker and group run driven on 2026-11-19 (2 of 3 hosts, unreachable test hosts fail per host with Retry failed); a native group open against real `sshd` was not clicked |
| Reconnect after server restart | Real local: `interrupted_transport_recovers_after_the_fixture_server_restarts` | — |
| Handshake timeout / cancel | Real local | — |
| SFTP / SCP transfers, interruption and retry, permissions, links | Real local: `local_sshd.rs`, `sftp_bounds.rs`, `sftp_metadata.rs` | — |
| Remote text editing (encodings, conflicts, permissions) | Real local: `remote_editor.rs` | — |
| Tunnels through real OpenSSH | Real local: `tunnels_carry_traffic_through_real_sshd_and_release_ports` — 256 KiB byte-exact over direct-tcpip (local/dynamic tunnel path), refused target fails only the channel, remote forward round trip, listener released after cancel | Desktop listener layer (`ssh.rs`) covered by its own unit tests, not driven through the native UI |
| Remote-forward approval/revocation edge cases | In-process russh server: `forwarding.rs` (protocol-level, not OpenSSH) | — |
| Jump hosts (IPv4/IPv6, rekey) | Real local | — |
| X11 forwarding | Real local to a loopback display fixture; Xvfb test skipped when absent | — |
| Telnet | Loopback TCP fixture server inside the tests (negotiation, NAWS resize, plaintext round trip, reconnect after peer disconnect) | macOS ships no `telnetd`; the server side is a test fixture, not a real Telnet daemon |
| Serial | Real local PTY pair: `local_pty.rs` | No hardware |
| VNC | `tools/vnc-helper`: 26 unit + 17 tests against a loopback RFB fixture server written for the tests | macOS Screen Sharing not used; fixture is not a real VNC server |
| RDP | `tools/rdp-helper`: 31 unit + 2 local helper-process tests | No RDP server available locally; RDP sessions not verified end-to-end |
| MobaXterm import | Unit / fixture: Windows-1252 + CRLF export, UTF-16LE/BE with BOM, lowercase sections, nested folders, custom icons, `;  logout` prefix, `<default>` login, escaped login command and comment, SSH gateway, `C:` / `.ppk` key paths, SFTP, VNC/RDP reported, secrets never read. Real local OpenSSH: an imported Windows export connects through its gateway with its mapped key and runs its login command (`windows_mobaxterm_export_connects_through_its_gateway_to_local_sshd`) | No file produced by MobaXterm itself available; fixtures follow the community format study, so field meanings are not vendor-confirmed. Native import clicked through on 2026-11-19: native file picker, Windows-1252 preview (accents, €, gateway, skipped RDP/VNC and Passwords), import into nested folders, kept after relaunch ([screenshot](evidence/native-macos/import-preview.webp)) |
| Open selected sessions in a group | Browser preview with sample sessions: picker ticks 4 of 6, only those 4 attempted (they fail with "requires the desktop runtime" in preview) | Native picker driven: untick one of three, Open 2 ([picker](evidence/native-macos/group-picker.webp), [run](evidence/native-macos/group-run.webp)) |
| Restore tabs on next launch | Browser preview: on → 4 tabs restored after reload with no duplicate shell; off → 1 tab | Native relaunch kept imported sessions; tab restore itself not re-checked natively |
| Pinned groups | Browser preview with 300 sessions: pin, survive reload, counts, picker and Open all from the strip, unpin. Unit-tested parser drops malformed storage | Native not driven |
| Contrast and minimum text size | Browser preview, scripted audit of every view and dialog in dark and light: no text under 4.5:1 or under 10.5px after token fixes | Native rendering not compared |
| `--open-group` parsing and resolution | Unit | Native launch not tested |
| Terminal-first layout, focus mode, tab menu | Browser preview at 1440×900 and 1100×680 | Native screenshots blocked (no Screen Recording permission for the shell) |
| Desktop managers (PTY, SSH manager, backpressure, shutdown) | `cargo test -p mobarust`: 117 passed | — |
| Native macOS app (release build) | Native: DMG `hdiutil verify` VALID, arm64 ad hoc bundle; window in 370 ms warm / 1.49 s first launch; idle CPU 0.1–0.2 %; 105 MB RSS main plus WebKit helpers; SIGTERM exit about 100 ms with no helpers left (`benchmarks/2026-11-19-native-macos.md`) | Screenshots and clicks in the native window are blocked: no Screen Recording or input-automation access in this session |
| Linux / Windows | Not run this round. The Quality workflow is disabled on GitHub by the maintainer, and the Release workflow runs only on tags or a manual dispatch, so no remote CI was started | Existing docs record earlier cross-checks |

## Native macOS click-through (2026-11-19)

Release build of `main` (0.2.0), run as an isolated lab copy (`tools/prepare-macos-ui-lab.mjs`: own HOME, portable data, no SSH agent), driven with real mouse and keyboard events after Screen Recording and Accessibility were granted.

| Flow | Result |
| --- | --- |
| Local shell | `echo native-$((6*7))` printed `native-42` |
| MobaXterm import | Passed (see import row) |
| Group picker and run | Passed (see group rows) |
| Tab rename | Name kept when the shell sends its own title. Bug found: focus did not return to the shell after the dialog; fixed and re-checked natively |
| Move tab | ⌃⇧PageUp moved the focused tab left with focus in the terminal |
| Copy on select | Selecting `moba-select-check` put exactly that text on the clipboard |
| Quit | ⌘Q and the Quit menu both exit in about 1 s and end the shells |

