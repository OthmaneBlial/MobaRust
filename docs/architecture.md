# Architecture overview

```text
React / TypeScript / xterm.js (apps/desktop/src)
                  ↓ typed Tauri commands and events
Rust desktop managers (apps/desktop/src-tauri/src)
                  ↓ native transport and persistence APIs
Workspace crates / isolated protocol helpers
                  ↓
SSH / SFTP / SCP / PTY / tunnels / serial / Telnet / diagnostics
```

React owns interaction, layout, terminal rendering, and transient editor
content. Rust owns connections, filesystem changes, credential lookup, child
processes, cancellation, persistence, and lifecycle state. The browser preview
uses synthetic state; a working preview alone does not prove native behavior.

## Where a contribution belongs

| Location | Responsibility |
| --- | --- |
| `crates/mobarust-core` | Secret-free session/settings/snippet/macro/audit models; connection/transfer state transitions; terminal batching |
| `crates/mobarust-ssh` | russh transport, per-hop trust/authentication, PTY channels, SFTP/SCP, forwarding, bounded remote editing/monitoring, X11 channels |
| `crates/mobarust-store` | Versioned atomic persistence; OpenSSH and profile import/export; validation and corruption refusal |
| `crates/mobarust-vault` | Native OS credential store and separate Argon2id/AES-GCM portable vault; zeroizing secret material |
| `crates/mobarust-network` | Bounded DNS/TCP/scan/ping/traceroute primitives |
| `crates/mobarust-telnet` | Plaintext Telnet negotiation, transport, and lifecycle |
| `crates/mobarust-serial` | Serial configuration, transport, and recoverable device loss |
| `crates/mobarust-remote-desktop` | Versioned bounded helper frames, capability validation, credentials/control separation, process supervision |
| `apps/desktop/src-tauri/src/main.rs` | Tauri command registration, app stores/vaults, sanitized diagnostics, native pickers |
| `apps/desktop/src-tauri/src/{ssh,terminal,network,serial,telnet,remote_desktop}.rs` | Session/job managers, typed request validation, cancellation, event routing, cleanup |
| `apps/desktop/src/App.tsx` and focused `.ts` modules | Operator UI and interaction policies, including macro targets, paste confirmation, remote paths, and saved-session requests |
| `tools/vnc-helper` | Isolated experimental VNC engine and controlled local RFB fixtures; normally packaged |
| `tools/rdp-helper` | Isolated experimental IronRDP engine and opt-in local server fixtures; excluded from normal bundles under its advisory gate |
| `xtask` | Sanitized developer validation, helper staging, packaging/checksum/layout audits, benchmarks |
| `benchmarks`, `fuzz` | Synthetic in-memory probes and isolated fuzz targets |
| `site`, `tools/release-assets.mjs` | Static project presentation and installer/version/checksum validation |

The two helper tools have separate Cargo workspaces/lockfiles. Keeping the RDP
candidate outside the desktop workspace prevents its experimental crypto and
native dependencies from weakening the SSH or portable-vault boundaries.

## Follow an SSH workflow

A saved profile contains host metadata, trust configuration, and opaque vault
references. The renderer creates a typed request. The native SSH manager
validates it and retrieves credentials inside Rust, then calls
`SshConnection::connect` or `connect_with_jump_chain`. Every hop verifies its
own key before authentication. The resulting connection opens PTY/SFTP or
forwarding channels. Bounded events route output/progress to the matching
terminal/transfer in the renderer.

Session closure uses a separate cancellation signal, observes in-flight
setup/reconnect work, and cleans up owned transfers/tunnels. EOF on a shell
does not discard a later exit status; normal shell exits and transport loss
follow different manager paths. Setup deadlines do not expire a healthy quiet
connection.

Remote files, terminal output, and helper messages remain untrusted. Editing
uses bounded reads and revision checks before complete-file promotion with
rollback attempts. See the [threat model](security/threat-model.md),
[SSH lab](testing/ssh-lab.md), and individual [decision records](adr/).

RDP/VNC interoperability, hardware serial behavior, signing, and full platform
runtime coverage are tracked separately in the [roadmap](../ROADMAP.md) and
[evidence matrix](testing/hardware-interoperability.md).
