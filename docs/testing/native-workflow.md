# Native macOS workflow receipt — 2026-10-02

## v0.1.26 ARM64 release-copy editor acceptance — 2026-10-03

A disposable app copy came from the verified v0.1.26 ARM64 DMG, source/tag
`c10546b9c5d4efb4909ecb4b6795bf4ea2964b5f`. Its packaged runtime SHA-256
was `e5695e075f1489a6632ad2efd4ad1626afc4d91b4a5bc5c27c69741a54e6234b`
before lab metadata and the disposable ad hoc signature. The actual process
had isolated HOME/ZDOTDIR/XDG paths and empty agent settings, checked before
connection and again before Quit. All SSH/file operations used generated
credentials and files on an explicitly pinned loopback OpenSSH fixture.

- Initial UTF-8 Open showed the correct selector, clean state and disabled Save.
- Native Windows-1252 Save wrote exact nine-byte `café · €` plus newline,
  preserving mode `0640`. Switching back and saving wrote exact 13-byte UTF-8.
  Both receipts retained the selected encoding and cleared dirty state.
- New UTF-8 Save as wrote the same bytes with `0600`, rebound the editor path
  and left the other generated files unchanged. No editor parts remained.
- Normal menu Quit with SSH and the local PTY active released the owned app,
  local child, server session children and established fixture connections.
  The manual harness then exited successfully, removed its private root and
  reaped its daemon; the loopback port was reusable.

The version-aligned full local suite and both mounted Mac package checks passed.
After publication, all four public assets downloaded anonymously with HTTP 200
and matched the verified local bytes; both downloaded manifests passed.
[Release files, hashes and limits](../release/v0.1.26.md).

This is the stated ARM64 release-copy workflow, separate from the broader
pre-version-bump candidate receipt below. Initial GUI Open still requires UTF-8;
the selector controls output encoding. Explicit legacy-encoding Open, native
warning focus, uncertain promotion recovery, listing-limit refusals, clean
installation, sustained use and Windows/Linux acceptance remain pending.
No personal SSH files, agent, Remote Login or firewall/router settings changed.

## Post-v0.1.25 native editor acceptance — 2026-10-03

A local production ARM64 candidate from runtime source `982f297` passed the
complete editor recovery/Save as workflow after the wire fix in `05e80d9`.
It still reports version 0.1.25, but is a newer source build, not the published
v0.1.25 installer. Its clean built executable SHA-256 was
`cfceeae9d6b96f791a39f4dbd2c727045f537ab95810245f8c89ed63a84ff1c6`.
Only the disposable copy received lab metadata/state and an ad hoc signature.

- Opening UTF-8 showed the correct selector, a clean buffer and disabled Save.
  Native menu navigation selected Windows-1252; Save wrote exact bytes
  `63 61 66 e9 20 b7 20 80 0a` for `café · €` plus newline, preserving `0640`.
  Selecting UTF-8 and saving wrote the exact 13-byte UTF-8 representation.
  Each successful receipt retained the selected encoding and cleared dirty state.
- An emoji in Windows-1252 was visibly refused without changing the original
  bytes/mode; the local buffer remained available for correction.
- New Windows-1252 Save as wrote exact bytes with `0600`. Create only preserved
  an occupied legacy file and retained the source buffer/path. Explicit Replace
  converted that target to exact UTF-8 bytes, preserved `0640`, rebound the
  editor path and cleared dirty state.
- A controlled external update triggered conflict refusal, preserving its bytes
  and mode plus the local buffer. Cancel retained the edit. Confirmed discard,
  close and reopen loaded the external revision; a subsequent accented UTF-8
  Save succeeded with exact bytes and `0640`.
- Native menu Quit with SSH and the local PTY active released the owned app,
  local child, server session children and established connections. The manual
  harness then exited successfully, removed its root and reaped its daemon;
  the loopback port was reusable. No editor parts/backups remained.

The isolated PID's HOME/ZDOTDIR/XDG and empty agent settings were checked before
connection and again before Quit. All operations used generated files/credentials
and the [disposable loopback lab](ssh-lab.md#disposable-native-file-editor-lab--2026-10-03).
The full local suite passed before the candidate build. This closes the stated
macOS conflict/Save as workflow gate; warning focus, lost promotion replies,
malicious path races, sustained use, Windows/Linux and updated installers remain
separate. Public v0.1.25 retains its selector defect.

## v0.1.25 release-copy editor check — 2026-10-03

A fresh disposable copy of the verified ARM64 DMG runtime (source `534c1ef`)
matched its packaged executable before lab metadata and its ad hoc test
signature were applied. Native accessibility and screenshot observation worked
on this attempt; the earlier observation failures below remain historical and
their cause is unproven. The isolated launch's actual process environment was
checked before using generated loopback OpenSSH credentials and files.

- A local PTY accepted input and returned the expected marker.
- Save refused an externally changed file, preserving its exact bytes and
  mode `0640`, with the local edit still visible. Cancel retained the dirty
  buffer; confirmed discard, close and reopen loaded the external revision.
- Save after reopen wrote the accented UTF-8 buffer exactly and preserved
  `0640`. New Save as wrote the same bytes with mode `0600` and rebound the
  editor path. Create only refused an occupied target without altering its
  bytes/mode or the buffer/path; explicit Replace succeeded and preserved `0640`.
- Windows-1252 Save failed at native argument decoding because the enum's JSON
  names differed from the renderer's values. The target remained unchanged.
  [Wire correction and regression](../adr/0022-bounded-remote-text-editor.md#native-encoding-wire-regression--2026-10-03)
  are on main after this release; the published installer is unchanged.
- Native menu Quit with active SSH released the owned app, local PTY and
  server session children/connections. After the stop marker, the manual
  fixture exited successfully, its directory was removed, its daemon was
  absent and its loopback port was reusable. No editor parts/backups remained.

The first connection exposed SFTP's account-home default: only directory names
were listed, with no file opened or copied there. A fresh fixture used the
corrected disposable SFTP working directory for all editor checks above.
An unexpected process change during keyboard automation also required stopping
the owned direct relaunch and restarting through the isolated launcher. See
[lab isolation and relaunch limits](ssh-lab.md#disposable-native-file-editor-lab--2026-10-03).

This establishes these short macOS ARM64 release workflows, not native encoding
acceptance, saved-with-warning focus, uncertain rename recovery, clean
installation, sustained use or Windows/Linux behavior. At this check the editor
gate stayed open; the newer candidate receipt above records its subsequent closure.

## v0.1.24 release-copy observation attempt — 2026-10-03

A fresh disposable copy came from the anonymously downloaded, verified ARM64
DMG. Before lab metadata and its ad hoc test signature were applied, the runtime
matched the packaged executable SHA-256. The actual PID had disposable
HOME/ZDOTDIR/XDG paths and empty SSH-agent variables; its process sample reported
the generated bundle identity and version 0.1.24 in AppKit's event loop.

CUA listed the app as running but native selection again failed with
`cgWindowNotFound`. No SSH fixture, file operation or native Quit check was
started. The owned app and shell were stopped by SIGTERM and verified absent;
only the generated copy was removed. This is isolation and signal-cleanup
evidence, not GUI, clean-install, editor-save or normal-Quit acceptance.

## v0.1.22 release-copy observation attempt — 2026-10-03

A disposable ARM64 app copy was prepared from the verified v0.1.22 DMG.
Only that copy received a unique bundle identity, portable empty state and an
ad hoc test signature. Its running PID had the generated HOME/ZDOTDIR/XDG
paths and empty SSH-agent variables. The release image was detached before
launch, and no SSH or VNC fixture was started.

CUA listed the app as running, but selecting its native surface failed with
`cgWindowNotFound`; no accessibility or screenshot acceptance was obtained.
This observation failure does not establish an application defect or working
GUI behavior. The owned app and its local shell child were terminated and
verified absent, then the disposable copy was removed. This was cleanup by
signal, not evidence of normal menu Quit. Release GUI, clean-install and
fullscreen acceptance remain pending; the earlier results below retain their
original version and scope.

## v0.1.17 observed workflows

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
