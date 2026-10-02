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
