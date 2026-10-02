# Native macOS workflow receipt — 2026-10-02

MobaRust 0.1.17 production builds, macOS ARM64, with a separate portable app
copy and disposable HOME. No installed personal app, SSH agent, Keychain
entries, personal keys, remote hosts or hardware were used.

## Observed results

- Local PTY command input/output and two independent split panes worked.
- Generated Ed25519 authentication and explicitly pinned generated host trust
  connected to a disposable OpenSSH server bound to IPv4 loopback.
- The SFTP browser listed four generated files. The remote editor saved a
  change to `app.conf`; the fixture file contained the new bytes afterward.
- SFTP downloaded `report.csv`, reported 38/38 bytes completed, and a byte
  comparison with the fixture source passed. SHA-256:
  `a0054384166946236be789f01efbc6abd00e13d50144016e35b65114453043f1`.
- The native tunnel form rejected target port 65536 and remained open without
  starting a listener. Cancel closed the form.
- Local, remote and SOCKS5 forms each started a loopback listener on an
  automatically selected port. Each passed an HTTP health request to the
  fixture service; the manager reported forwarded bytes.
- Stop removed all three active listeners. A later local-forward recording
  take also passed the same health request.
- A snippet was saved and its variable preview rendered the expected command.
  It was not executed automatically. The light/dark switch changed the UI and terminal.

## Local test isolation

Run development and native acceptance servers only on `127.0.0.1` or `::1`,
using disposable credentials and generated host keys. These loopback listeners
are reachable from this computer only. Do not bind fixtures to wildcard or LAN
addresses, enable macOS Remote Login, change the firewall/router, or publish a
home-network port. Tunnel tests must also keep their listener and destination
inside the disposable loopback lab.

Stop the owned app, SSH/HTTP servers and relays after each native lab; verify
that their recorded ports refuse new connections. Use the generated test HOME
and explicit fixture trust, without personal SSH configuration, agent or keys.
Repository downloads and GitHub pushes are outbound operations; they do not
require an inbound test listener.

## Repeat the tunnel regression check

Use a dedicated loopback SSH/HTTP lab, explicit generated trust, and a separate
portable app copy. In each tunnel mode, open the form, confirm loopback/zero-port
defaults, reject a malformed or out-of-range port, then start the valid listener.
Read its assigned port from the manager. Send a bounded HTTP request through
that endpoint (use curl's SOCKS5 option for the proxy). Verify response and byte
counts, stop it, and verify a new connection to its port is refused.

`pnpm --dir apps/desktop test:unit` retains the runnable port-boundary regression
checks. Type checks and ESLint passed after replacing browser prompts with the
integrated form. The full local `cargo xtask check` had passed before this focused
frontend correction; the corrected frontend was then checked and both native
Mac release packages rebuilt.

This is a short lab smoke test, not sustained-use, clean-install, Windows/Linux
GUI or Intel hardware evidence. The video uses two successive 0.1.17 production
builds: the tunnel chapters show the final integrated form; earlier chapters
were recorded immediately before that focused correction.

## Post-release settings and local-shell shutdown

A rebuilt macOS ARM64 debug app with the settings-error correction after
`c40d349` verified cancelled, malformed, valid multiline and out-of-range
settings imports, rejected shortcut collisions, unchanged settings/session
bytes after rejection, and visible error focus in light/dark mode. Native menu
Quit returned exit status 0 and released the app's active local zsh PTY child.
These GUI checks started no server or network listener. The copied app's
actual HOME/ZDOTDIR was verified before and after automation selected it.

[Detailed receipt and remaining limits](text-input-dialogs.md#settings-error-visibility-and-normal-shutdown--2026-10-02).
This is local-shell shutdown evidence; active SSH, remote-desktop helpers and
in-flight transfer shutdown still require their own native checks.

## Post-v0.1.18 SSH shutdown and retry-budget check

The [native SSH retry receipt](ssh-reconnect.md) adds macOS ARM64 debug evidence:
a two-attempt budget exhausted against a rejecting loopback relay, explicit
profile recovery executed a fixture shell command, and native menu Quit released
an active SSH session/remote shell plus the local zsh PTY. The rebuilt candidate
also distinguished ERROR from normal CLOSED and removed stale LIVE/active
labels. This correction is on `main` after v0.1.18; published DMGs are unchanged.
In-flight transfer shutdown and other platforms remain separate gates.

## Post-v0.1.18 transfer cancellation and SSH-tab closure

The [native transfer lifecycle receipt](transfer-lifecycle.md) adds 32 MiB SFTP
download/upload cancellation with original preservation and part cleanup,
byte-matched download Retry, and a reproduced/fixed cleanup race when closing
SSH during an upload. Session-owned workers now finish cleanup before transport
disconnect. App Quit during an active transfer, recursive/SCP GUI workflows and
other platforms remain separate gates.
