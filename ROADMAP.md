# MobaRust roadmap

**Updated 2026-10-03.** The next goal is a dependable remote workstation across Windows, macOS, and Linux. Work is ordered by reliability, security, and operator value.

The [v0.1.29 Mac previews](docs/release/v0.1.29.md) stop automatic reconnect
retries after uncertain startup delivery. A disposable copy of the verified
ARM64 DMG passed Escape cancellation before connection, approved startup
exactly once, private fixture interruption, replacement password/OTP challenges
and the phase-specific timeout. No third connection appeared for 63.59 seconds
while the listener stayed available. Normal menu Quit released the app/local
PTY; the fixture deadline removed private state and released its loopback port.

Earlier [setup receipts](docs/testing/ssh-lab.md#repeatable-native-shell-setup-lab-on-main)
cover connected exact-once startup, shell refusal and explicit SSH-tab cleanup.
Those results retain their recorded source/artifact scope. Broader native
successful-startup/output-ordering, queued expiry/overflow, sustained workloads
and updated Windows/Linux downloads remain pending.

**Checklist completion: 57 of 76 items (75%).** Items differ in scope; this is not a production-readiness score.

A checked item means its stated implementation or test exists. It does not imply signed installers, broad server compatibility, or hardware certification. Changes on `main` and published downloads are separate milestones.

## Where the project stands

| Area | Progress | What remains |
| --- | --- | --- |
| **Published previews** | macOS ARM64/x64 v0.1.29; Windows/Linux x64 v0.1.12 | Align platform releases; verify clean install/uninstall; signing and notarization. |
| **Core workstation** | Rust/Tauri shell, xterm.js, tabs, nested splits, settings, session organization, and native PTY | Complete the shell/platform and GUI evidence matrix. |
| **SSH and files** | Interactive SSH, jump chains, reconnect, SFTP/SCP, recursive transfers, tunnels, and remote editing | Broader authentication/server matrix, restart recovery, and sustained workloads. |
| **Quality checks** | Full local macOS ARM64 suite; one completed green Ubuntu/macOS/Windows run | GitHub workflows are disabled by request. New Linux shell variants and repeated Windows startups still need runtime evidence. |
| **Remote desktop** | Isolated RDP/VNC helpers and controlled loopback fixtures | RDP dependency/certificate gates; real-server sessions; platform packaging and long-run stability. |
| **Hardware and distribution** | Unix serial PTY fixtures and unsigned package-layout checks | Real serial adapters, external X servers, GUI behavior, and trusted installers. |

Sources: [native/platform evidence](docs/testing/hardware-interoperability.md), [SSH lab](docs/testing/ssh-lab.md), [dependency audit](docs/security/dependency-audit.md), and [release notes](docs/release/v0.1.29.md).

## Completed in the current improvement cycle

These changes are included in the v0.1.17 Mac preview. Windows/Linux installers remain v0.1.12.

- [x] Keep established quiet SSH sessions alive beyond their setup deadline.
- [x] Preserve exit status sent after channel EOF, so normal shell exit does not become a reconnect.
- [x] Distinguish rejected host keys, unreadable trust files, and transport failures; preserve redacted errors.
- [x] Add `cargo xtask test-ssh` with disposable HOME/state, generated credentials, encrypted-key rejection/recovery, and stalled-handshake cancellation/cleanup.
- [x] Test a distinct two-bastion/target topology, per-hop key rejection, and Unicode SFTP over the chain.
- [x] Exercise empty/wrong-key rejection and successful signing with a dedicated Unix SSH agent.
- [x] Exercise `::1` and explicit IPv6 known_hosts locally; expose a skip when loopback IPv6 is unavailable.
- [x] Interrupt an established loopback SSH transport, reject connections while its server is stopped, and recover after restart with unchanged keys/trust.
- [x] Verify frontend, native workspace tests, and Clippy on Ubuntu, macOS, and Windows in [Quality run 36998538983](https://github.com/OthmaneBlial/MobaRust/actions/runs/36998538983), source `ac70e39`.
- [x] Include the Windows resource icon required by ordinary native builds; Windows now reaches its runtime tests.
- [x] Fix release-asset tests launched from a different working directory and include them in the local suite.
- [x] Add contributor/security/architecture guides and GitHub report/PR templates.
- [x] Refresh advisory records and publish a benchmark receipt with hardware, samples, and observed CPU load.
- [x] Replace native tunnel browser prompts with an integrated form; verify invalid-port rejection, all three modes and listener cleanup on macOS ARM64.
- [x] Build and check both v0.1.17 Mac DMGs and record the real native workflows.

The SSH lab has **42 unit tests, eight Rust authentication-wire tests, one Unix native-lab directory-permission/cleanup regression and 12 OpenSSH integration test cases**, plus opt-in direct and two-bastion native authentication fixtures. IPv6 and real Xvfb have conditional prerequisites; harness success alone does not prove those optional cases ran. The local IPv6 case ran successfully; real Xvfb was skipped.

### Included in the v0.1.18 Mac preview

- [x] Replace the remaining 19 browser text prompts with a shared modal input, multiline JSON and read-only copy fields; retain cancellation and session ownership guards.
- [x] Replace all 20 browser confirmations with modal approval and explicit Cancel / Create only / Replace choices; pin paste and macro destinations across reconnects, including unchanged SSH IDs.
- [x] Verify native macOS ARM64 input and approval for Unicode mkdir/rename, permission validation, Save as collision policies and local multiline paste in a disposable lab.
- [x] Verify native paste and per-action macro refusal across real loopback SSH reconnects, exact selected-target broadcast delivery, emergency shortcuts, session JSON import and startup-command approval on macOS ARM64.
- [x] Handle emergency stops before terminal/modal event propagation; clear stale import errors after a successful retry.
- [x] Bind file actions and editor documents to their original SSH connection generation; verify native stale approval refusal for transfers/retry, mkdir, rename, delete, chmod and editor Save/Save as after real reconnects.
- [x] Diagnose the VNC fixture disconnects as server input deadlines expiring during large JSON framebuffer processing; use a minimum-size resize canvas and retain the original deadlines and protocol assertions. All 17 parallel fixture cases and the ordinary local `cargo xtask check` passed. [Diagnosis and limits](docs/research/vnc.md#local-fixture-deadline-diagnosis--2026-10-02).
- [x] Measure native framebuffer encoding/decoding in debug and release; fix JSON expansion that rejected valid HD images, retain the 8 MiB limit with wire version 2, and exercise 1920×1080 through the real loopback VNC helper before input/clipboard checks. [Measurements and limits](benchmarks/2026-10-02-framebuffer-ipc.md).
- [x] Replace native-to-WebView JSON pixel arrays with a bounded latest-frame binary pull; test coalescing and stale reconnect replies, and observe Full-HD rendering plus keyboard input before/after reconnect in a disposable macOS ARM64 app. [Native receipt and limits](docs/testing/remote-desktop-renderer.md).
- [x] Resolve native fullscreen rejection on macOS ARM64 with Tauri 2.12.1; observe Full-HD canvas entry, focused keyboard input, button/Escape exit and unchanged server geometry in the disposable native app. [Receipt](docs/testing/remote-desktop-renderer.md#fullscreen-recheck-with-tauri-2121).
- [x] Rebuild matching wire-version-2 Mac app/helper DMGs; verify ARM64 release-copy rendering, keyboard/reconnect, settings rejection and native Quit releasing zsh plus VNC helper. [Release receipt](docs/testing/remote-desktop-renderer.md#v0118-release-bundle-recheck).
- [ ] Verify fullscreen keyboard/button/Escape acceptance in the Mac release and the updated runtime on Windows/Linux; release entry accessibility and Mac debug results do not establish all these gates.
- [ ] Measure sustained native pipe/Tauri/rendering throughput and end-to-end input latency under large-frame workloads; codec timings and bounded fixture success do not establish GUI responsiveness. Mac v0.1.18 contains matching version-2 app/helper cohorts; Windows/Linux packaging remains pending.
- [x] Verify macOS ARM64 settings JSON cancellation, malformed/out-of-range rejection, valid multiline import, unchanged persisted bytes and visible error focus in light/dark mode; also reject conflicting shortcut saves. [Receipt](docs/testing/text-input-dialogs.md#settings-error-visibility-and-normal-shutdown--2026-10-02).
- [x] Observe native macOS menu Quit exiting successfully and releasing an active local zsh PTY in the isolated debug app. The v0.1.18 ARM64 release copy additionally released its active VNC helper; active SSH/transfer shutdown remains separate.
- [ ] Finish in-flight native transfer cancellation/collision recovery, cross-platform editor discard and dialogue acceptance. [Checks and next acceptance gate](docs/testing/text-input-dialogs.md) separate completed observations from pending evidence. Mac v0.1.18 includes these source changes; Windows/Linux v0.1.12 does not.

### Included in the v0.1.19 Mac preview, after v0.1.18

- [x] Preserve the SSH error state after final disconnected/closed events; distinguish normal shell exit, and replace stale LIVE/active-transport labels with the selected terminal's actual state.
- [x] Verify native macOS ARM64 two-attempt exhaustion, explicit profile recovery and Quit releasing an active SSH session plus local PTY. [Receipt and limits](docs/testing/ssh-reconnect.md).
- [x] Honor cancellation queued after copying and before final promotion on all six transfer paths; verify local original/part preservation and six real-SFTP promotion cases, including cancellation during metadata. [Checks and limits](docs/testing/transfer-cancellation.md).
- [x] Drain session-owned transfer workers before SSH disconnect; verify native 32 MiB SFTP Cancel, byte-matched download Retry and active-upload SSH-tab closure on macOS ARM64. [Receipt and remaining gates](docs/testing/transfer-lifecycle.md).
- [x] Defer normal app exit until SSH sessions drain; reject new work during shutdown and route macOS menu Quit through cleanup. Verify native menu Quit during a 32 MiB SFTP upload and window close during download, preserving originals and removing parts on macOS ARM64. [Receipt and remaining exit gates](docs/testing/transfer-lifecycle.md#application-shutdown-correction--2026-10-02).
- [x] Retire native SSH command queues on loss/closure, cancel queued file/tunnel actions visibly, and give a reconnected shell a fresh queue under the same terminal ID. Verify close refusal, late permits, no input replay, fresh delivery and loopback listener release with deterministic regressions; GUI loss/race acceptance remains pending. [Checks and limits](docs/testing/queued-ssh-commands.md).
- [ ] Align these corrections across the installer cohort: both checked v0.1.25 Mac DMGs include them; Windows/Linux remain v0.1.12. Published older DMGs remain unchanged.

## Next, in priority order

### 1. Maintain the native validation baseline locally

**Completed baseline:** Windows builds, PowerShell/cmd PTY tests, workspace tests, and Clippy pass in the recorded three-platform run. An earlier PowerShell startup timed out; the fixture now uses direct console output and joins its reader during cleanup. GitHub Quality and installer workflows are disabled at the maintainer's request; Quality has no push/PR triggers.

- [x] Add direct console fixture I/O, child/reader cleanup, and a distinct cmd round trip with bounded deadlines.
- [x] Run workspace tests and Clippy to completion on Windows x64.
- [x] Obtain one completed green Quality run across Ubuntu, macOS, and Windows.
- [ ] Run the stricter three-startup PowerShell/cmd checks on a dedicated Windows runtime and exercise zsh/fish on Linux; these additions were locally validated but their follow-up CI run was cancelled when CI was disabled.

**Next gate:** every repeated startup must produce expected output, exit successfully, and release disposable state. Retain local validation receipts; the recorded CI baseline does not prove the newer repeated-startup cases ran.

### 2. Turn shell and platform assumptions into evidence

- [ ] Exercise PowerShell and cmd input/output, resize, cancellation, and exit separately.
- [ ] Validate WSL discovery and terminal startup on a dedicated Windows runtime.
- [x] Record installed bash/zsh/fish variants and explicit skips on Unix; Linux zsh/fish runtime coverage remains pending.
- [ ] Exercise native GUI focus, paste, split-pane lifecycle, and application shutdown on Windows, macOS, and Linux.

The v0.1.20 Mac preview includes startup output ordering corrections across SSH, Telnet and serial
attachment. Native manager regressions verify replay under the publication lock;
[GUI timing and sustained output acceptance remain pending](docs/testing/terminal-attachment.md).
After v0.1.20, main also preserves the latest SSH terminal dimensions across
reconnect backoff without replaying input. Manager and loopback packet checks
cover coalesced sizes and replacement-shell geometry;
[native resize/reconnect acceptance remains pending](docs/testing/ssh-reconnect.md#terminal-size-across-reconnects-on-main).
Main also checks shell/X11 request acceptance before returning an SSH shell or
sending startup input. Loopback regressions cover rejection, silence, early
closure, bounded setup output and ordered pre-acceptance bytes;
[native GUI and wider server acceptance remain pending](docs/testing/ssh-reconnect.md#shell-request-acceptance-on-main).
After v0.1.22, source also drains accepted editor/file and monitor operations
before SSH disconnect; [backend ownership checks and pending native save/exit gates](docs/testing/transfer-lifecycle.md#accepted-fileeditor-operations-during-session-cleanup--2026-10-03) remain distinct.
Both SFTP channels now require the server's subsystem acceptance before INIT,
with explicit refusal/closure, bounded early output and owned failed-channel cleanup;
[native file-browser/editor and broader server acceptance remain pending](docs/testing/ssh-lab.md#sftp-subsystem-acceptance-on-main--2026-10-03).
Both readers also enforce the configured 256 KiB response payload limit before
allocation, reject impossible sequence counts and excess read data, and settle
requests when malformed frames close their SFTP channel. [Regression evidence
and installer limits](vendor/russh-sftp/MOBARUST_PATCH.md) remain separate from
native file-browser/editor acceptance.
These accepted-operation, SFTP setup and response-bound corrections are packaged
in both verified [v0.1.23 Mac previews](docs/release/v0.1.23.md). Windows/Linux
remain v0.1.12; the newer package checks do not close native workflow gates.
After v0.1.23, the shared file reader also preserves unconsumed bytes when a
cancelled read resumes with a smaller buffer, retires old read state on position
changes/closure and clamps negotiated READ sizes to the client packet budget.
[In-memory regression evidence](docs/security/dependency-audit.md#sftp-file-read-cancellation-and-negotiated-limits--2026-10-03)
does not establish native editor/transfer acceptance. Both verified
[v0.1.24 Mac previews](docs/release/v0.1.24.md) contain the reader and negotiated-limit fixes.
Dropped SFTP request futures now release their reply slots immediately, including
file-write and file-drop close acknowledgments. Late ordinary replies after
cancellation/timeout no longer close the usable SFTP stream; initialization and
malformed-frame errors still fail closed. [Request ownership and wire regression
checks](docs/security/dependency-audit.md#sftp-request-lifetime-and-late-replies--2026-10-03)
are included in both v0.1.24 Mac installers; native acceptance remains pending.
The shared SFTP file writer also splits uploads to fit both data and encoded
packet limits, and refuses nonempty writes with no payload room or a closed
handle. [Byte-matched chunk and refusal checks](docs/security/dependency-audit.md#sftp-file-write-packet-budgets-and-closed-handles--2026-10-03)
are included in both v0.1.24 Mac installers; native upload acceptance remains pending.
After v0.1.24, source also retires failed SFTP seek futures and uses checked
unsigned offsets. Tiny in-memory peers verify seek recovery, metadata failures,
the full unsigned range and READ/WRITE refusal before position wrap.
[Offset checks and installer limits](vendor/russh-sftp/MOBARUST_PATCH.md)
remain separate from native file-workflow acceptance. Both v0.1.25 Mac DMGs now include these corrections; historical v0.1.24 downloads remain unchanged.
Source after v0.1.24 also corrects overlapping SFTP file-type classification
and shares a no-follow upload destination guard across SCP, SFTP and recursive
files. [Metadata, link replacement and unsafe-type regressions](docs/adr/0008-sftp-transfer-pipeline.md#upload-destination-regressions--2026-10-03)
verify mode preservation and refusal before rename. Native collision workflows,
other-platform metadata and updated Windows/Linux installers remain pending.
The six desktop transfer paths now share private part creation: SFTP uses
exclusive mode-`0600` files, SCP first reserves an exclusive part and sends
`C0600`, and Unix downloads request mode `0600`. [Ownership, mode and close-boundary regressions](docs/adr/0008-sftp-transfer-pipeline.md#private-transfer-parts-and-close-acknowledgements--2026-10-03)
cover occupied paths, denial and cancellation without promoting partial data.
Native/recursive acceptance, Windows ACL evidence and updated Windows/Linux installers remain pending.
Delete now unlinks final-path symlinks and refuses nonempty directories;
[owned OpenSSH entry checks](docs/adr/0008-sftp-transfer-pipeline.md#no-follow-entry-deletion--2026-10-03)
preserve link targets. Native dialog and other-platform acceptance remain pending.
SFTP downloads now require regular STAT/FSTAT types before file reads, reject
unsafe sources and observe cancellation/close replies. [Source and shutdown checks](docs/adr/0008-sftp-transfer-pipeline.md#guarded-downloads-and-acknowledged-shutdown--2026-10-03)
also cover direct-upload cancellation and failed pending WRITE replies.
Native/recursive acceptance and updated Windows/Linux installers remain pending; this does not close a checklist gate.
The shared SFTP File adapter also retires handle operations as shutdown starts,
avoids duplicate CLOSE on cancelled drop or completed retry, and preserves
failure on subsequent retry. [Marker-gated handle lifecycle checks](docs/security/dependency-audit.md#sftp-handle-retirement-after-v0124--2026-10-03)
cover pending/successful/denied close and pre-close draining without changing
empty I/O or local position semantics. Native and Windows/Linux installer acceptance remain pending.
Native rename, Delete, mkdir and chmod now require a named final entry before
session lookup. The shared guard handles root/dot aliases and trailing slashes;
chmod also refuses live final symlinks or unknown types without opening contents.
[Path and permission regressions](docs/adr/0008-sftp-transfer-pipeline.md#named-mutations-and-explicit-permission-targets--2026-10-03)
preserve targets and retain mode-000/special-entry repair. Native and wider-platform
acceptance and updated Windows/Linux installers remain pending; the checklist is unchanged.
Startup input now drains bounded output while waiting for SSH window credit;
a 256 KiB loopback burst verifies ordered output and exact-once command delivery.
[Native setup checks and remaining rendering/platform gates](docs/testing/ssh-reconnect.md#startup-input-and-output-backpressure-on-main)
separate connected/exact-once and refusal/timeout receipts from full GUI output-order evidence.

**Done when:** the [platform matrix](docs/testing/hardware-interoperability.md) records OS/architecture, shell version, source commit, exact command, result, and limits. CI fixtures and GUI/manual results remain separate.

### 3. Expand SSH interoperability evidence

- [x] Observe desktop automatic reconnect after a controlled loopback transport interruption and refuse approvals from the previous connection generation.
- [x] Observe two-attempt native SSH retry exhaustion against a rejecting loopback relay, explicit fresh-profile recovery, normal `exit` and active SSH/local-shell cleanup on macOS ARM64. [Receipt](docs/testing/ssh-reconnect.md).
- [ ] Exercise zero/maximum GUI budgets, repeatedly flapping short-lived shells and recovery across a real daemon restart; the two-attempt relay check does not establish those cases.
- [x] Verify password and static-response keyboard-interactive acceptance/rejection over real SSH packets, unsafe prompt refusal, trust-before-authentication and timeout/cancellation socket cleanup using a memory-only loopback Rust server on macOS ARM64. [Coverage and limits](docs/testing/ssh-lab.md#portable-authentication-wire-fixture).
- [x] Implement a separate ask-each-challenge mode in quick connect, saved profiles, jump hops and reconnect; verify distinct password/OTP success, wrong-OTP/trust rejection, cancellation and unanswered-responder drop at every endpoint of a two-bastion chain over SSH packets, bounded responder refusal, one-shot native answer ownership and dialogue abort/secret-field clearing. [Source checks and limits](docs/testing/ssh-lab.md#ask-each-challenge-on-main).
- [ ] Finish native authentication acceptance: Mac debug covers saved/Quick-connect password/OTP, trust/OTP rejection, Escape/expiry/focus, reconnect, two-bastion routing and shutdown. Main's two-session prompt queue also passed native overlap, distinct-factor success, cancellation handoff, input and pending-authentication Quit checks. The v0.1.19 ARM64 release copy covers direct login, trust rejection, Escape, reconnect and active-session Quit. Native queued expiry/overflow, release-copy Quick connect/two-bastion/queue acceptance, Windows/Linux and OpenSSH password/PAM remain pending. [Receipts and limits](docs/testing/ssh-lab.md#native-overlapping-reconnects-and-shutdown--2026-10-02).
- [ ] Cover Windows Pageant/alternative agents and additional OpenSSH versions.
- [x] Exercise sustained output, interrupted large transfers, and routed IPv6 in a controlled lab: [16 MiB SFTP/SCP interruption and full retry](docs/testing/interrupted-transfers.md), plus [byte-matched 8 MiB PTY/SFTP streams through two IPv6 bastions](docs/testing/routed-ipv6-streams.md) with ordinary and frequent-rekey cases. Fix nested-stream write starvation and retain shell output sent after process exit. These are macOS ARM64 protocol/PTY checks included in the v0.1.22 Mac installers; native rendering and wider platforms remain separate.

The v0.1.20 Mac preview queues competing SSH challenges instead of cancelling the
second login. Production-handler regressions cover request ownership, queued
expiry, cancellation, overflow, page shutdown and IPC failure; frontend and Mac
debug packaging checks passed. [Two-session Mac debug overlap, cancellation and Quit were also verified](docs/testing/ssh-lab.md#native-overlapping-reconnects-and-shutdown--2026-10-02); broader native acceptance remains pending.

The v0.1.29 Mac installers include the uncertain-startup retry stop; ordinary pre-startup failures remain bounded. [Source checks](docs/testing/ssh-reconnect.md#uncertain-startup-delivery-stops-reconnect-retries-on-main--2026-10-03), [Mac debug acceptance](docs/testing/ssh-reconnect.md#native-reconnect-startup-retry-stop--2026-10-03) and [ARM64 release-copy timeout acceptance](docs/testing/ssh-reconnect.md#v0129-arm64-release-copy-retry-stop--2026-10-03) retain distinct scopes. Native acceptance of other startup-write failures, real OpenSSH/PAM restart and wider platforms remains open; the checklist is unchanged.

**Done when:** each added case has success, rejection/failure, bounded cancellation, and cleanup checks, runnable through the existing lab command. No personal accounts, agents, or production hosts are test prerequisites.

### 4. Align and verify the next preview release

- [ ] Build Windows/Linux previews containing the latest source fixes alongside both Mac architectures.
- [ ] Verify startup, native helper/resources, clean installation/uninstallation, and downloaded SHA-256 manifests per target.
- [x] Validate v0.1.29 Mac notes, versions, architectures, artifact names and all four anonymously downloaded, byte-matched files and SHA-256 manifests; Windows/Linux remain pending.
- [ ] Establish Windows publisher signing, macOS Developer ID/notarization, and a maintainable signed distribution path.

**Done when:** verified artifacts and their limitations are documented per platform. GitHub workflows remain disabled. Signing requires real credentials/infrastructure and is a separate gate from unsigned previews.

### 5. Polish daily terminal and file workflows

On main after v0.1.29, [session operation admission](docs/testing/transfer-lifecycle.md#bounded-session-operation-admission-after-v0129--2026-10-03)
bounds finite file/monitor and active/waiting transfer workers to 32 per SSH
session. Excess work receives an explicit busy failure; input and independent
resize remain available. Native saturation/Retry, many-session/IPC pressure
and wider-platform checks remain pending; published installers are unchanged.

Finite actions and transfer/tunnel starts also use [immediate command queue
admission](docs/testing/transfer-lifecycle.md#immediate-command-queue-admission-after-v0129--2026-10-03):
a full queue refuses explicitly before cancellation controls are registered,
instead of retaining callers waiting for capacity. Terminal input keeps
backpressure. Listener binding, tunnel totals across many sessions and total IPC/process
memory remain separate concerns; the checklist is unchanged.

The shared 32-worker owner now also includes [local/SOCKS/remote tunnel
jobs](docs/testing/tunnel-lifecycle.md). Excess queued starts fail explicitly,
and session cleanup cancels and joins accepted runners before transport
cleanup. Loopback admission and idle/pending-job wire checks do not close
native sustained-traffic or real remote-listener revocation gates.

- [ ] Exercise keyboard navigation, focus return, resize, reconnect, and failure recovery in the native app.
- [ ] Validate multi-file/recursive transfers, collision decisions, native SCP Cancel/retry, OS-originated/other-platform Quit during transfer and progress under realistic workloads. The [single-file SFTP native receipt](docs/testing/transfer-lifecycle.md) and [pre-promotion checks](docs/testing/transfer-cancellation.md) do not close this broader gate.
- [x] Verify a basic native remote-editor save and a byte-matched SFTP download in the disposable macOS ARM64 lab.
- [x] Verify macOS ARM64 remote-editor conflict/discard/reopen recovery and both Save as policies through a complete native UI workflow, including exact UTF-8/Windows-1252 bytes, modes and shutdown. [Source-candidate receipt](docs/testing/native-workflow.md#post-v0125-native-editor-acceptance--2026-10-03); the selector fix is shipped and encoding Save/new Save as rechecked in the [v0.1.26 ARM64 installer copy](docs/testing/native-workflow.md#v0126-arm64-release-copy-editor-acceptance--2026-10-03). Windows/Linux acceptance remains separate.
- [x] Record a new native macOS ARM64 demo with authenticated disposable SSH, SFTP editing/download and a working tunnel; redact account labels and local paths.

On `main` after v0.1.24, [remote-editor encoding conversion](docs/adr/0022-bounded-remote-text-editor.md#encoding-conversion-regression--2026-10-03)
uses byte-level conflict checks for Save and Save as, so Windows-1252 → UTF-8
conversion works without weakening revision refusal. A real loopback OpenSSH
regression covers conversion, unchanged targets after refusals, permissions
and cleanup. The newer Mac source candidate closes the specific UI recovery
gate above; v0.1.26 ships the encoding-selector wire fix and passed the stated
release-copy conversion checks. Windows/Linux execution remains pending.
The v0.1.27 Mac downloads now include explicit UTF-8/Windows-1252
**Open text as** through the existing bounded reader. The verified ARM64
installer copy passed native default-UTF-8 refusal, legacy Open/Save/reopen,
UTF-8 conversion/reopen and normal Quit with exact bytes/modes and complete
fixture cleanup. [Release-copy receipt](docs/testing/native-workflow.md#v0127-arm64-release-copy-legacy-open-acceptance--2026-10-03).
Windows/Linux, warning focus and uncertain promotion recovery remain open. [Behavior and
regressions](docs/adr/0022-bounded-remote-text-editor.md#explicit-legacy-encoding-open-on-main--2026-10-03).
Public v0.1.25 retains its selector defect.

The [editor temporary ownership check](docs/adr/0022-bounded-remote-text-editor.md#temporary-ownership-regression--2026-10-03)
also refuses existing temporary files/links without truncating or removing
them. New editor files are created with mode `0600`, and replacement retains
the original mode. A loopback OpenSSH regression verifies both Save paths;
the Mac candidate adds normal Save as acceptance, while deliberate temporary
collisions through the GUI and Windows/Linux downloads remain separate.

[Committed-save receipts](docs/adr/0022-bounded-remote-text-editor.md#committed-save-receipt-regressions--2026-10-03)
now retain the acknowledged editor buffer/revision without a follow-up read.
Backup cleanup trouble is a visible saved-with-warning result. Three portable
loopback SSH regressions cover nine Save/Save as flows, concurrent changes and
usable retries. Native warning/recovery acceptance and updated Windows/Linux installers
remain pending.

**Done when:** the workflows can be completed and recovered using the keyboard, errors explain the next action, and recordings show the tested product rather than synthetic states. The [v0.1.17 demo provenance](docs/release/desktop-demo.md) and [native lab receipt](docs/testing/native-workflow.md) record the tested scope. Broader failure recovery remains open.

## Experimental work and beta gates

| Area | Already implemented | Gate before broader support claims |
| --- | --- | --- |
| **RDP** | Isolated IronRDP helper, credential/IPC boundaries, framebuffer/input/resize/reconnect loopback tests | Resolve the RSA advisory and certificate-validation boundary; authenticate against a controlled Windows desktop; verify framebuffer, keyboard/mouse, resize, failure recovery, and a 30-minute session. Keep it out of normal bundles until security gates pass. |
| **VNC** | Native helper, controlled RFB/password/JPEG fixtures, input, clipboard opt-in, quality controls, and reconnect | Validate a real controlled VNC server, explicit transport policy, unsupported capability diagnostics, a 30-minute session, and helper packaging on each target. TCP remains unencrypted. |
| **X11** | Opt-in SSH channel bridge to an explicitly selected external display; loopback tests and optional Xvfb fixture | Run real external X-server cases on Linux, macOS, and Windows; verify authentication, DISPLAY setup, cancellation, and cleanup. |
| **Serial** | Native configuration, refresh, sessions, and Unix pseudo-terminal lifecycle/device-loss tests | Test dedicated physical adapters, drivers/permissions, baud/parity/flow control, removal, and explicit reconnection per OS. |

Both [verified v0.1.26 Mac previews](docs/release/v0.1.26.md) include the earlier editor, transfer, named-mutation and SDK handle corrections, plus the encoding wire fix and directory-listing bounds. All four published files were downloaded anonymously and byte-matched. Broader native workflows and Windows/Linux alignment remain pending.

On main after v0.1.25, [directory listing guards](docs/adr/0008-sftp-transfer-pipeline.md#bounded-directory-listings--2026-10-03)
bound filtered entries, cumulative text and the READDIR phase in the shared
browser/recursive-planning reader. Local protocol regressions cover refusal,
acknowledged close and same-connection recovery. Both v0.1.26 Mac installers
include this guard; native limit/refusal and cross-platform acceptance remain
open. The Mac editor candidate and v0.1.26 release copy exercised ordinary
directory listings.

## Implemented foundation

These existing capabilities should be improved rather than recreated:

- [x] Typed Rust connection/transfer state, native PTY batching, tabs, nested split panes, resize, and explicit close.
- [x] Session folders, tags, favorites, recents, search, OpenSSH import with compatibility reporting, and secret-free import/export.
- [x] SSH trust policy, password/key/agent auth, encrypted keys, keyboard-interactive path, per-hop jump transport, and bounded reconnect.
- [x] SFTP browsing/file actions, streaming SCP/SFTP, recursive SFTP, a bounded transfer queue, cancellation, and explicit file promotion.
- [x] Local/remote forwarding, SOCKS5 tunnels, bounded DNS/TCP/ping/traceroute diagnostics, and opt-in remote monitoring.
- [x] OS credential references, a marker-gated encrypted portable vault, atomic settings persistence, and optional bounded audit history.
- [x] Reviewed snippets, variables and preview, cancellable macros/recording, explicit broadcast targets, and emergency disable.
- [x] Remote text editing with conflict detection, syntax highlighting, search/replace, encoding selection, and save-as.
- [x] Telnet transport/UI and native serial transport/UI with controlled fixtures.
- [x] Fuzz targets, synthetic package-layout contracts, and terminal/session benchmark tooling.

## Performance and maintenance

- [ ] Repeat terminal batching and 10K-profile benchmarks under comparable idle conditions; record hardware, compiler, samples, medians, and load.
- [ ] Measure sustained native terminal rendering and UI responsiveness before claiming throughput improvements.
- [ ] Recheck advisories at release time; keep RDP's known-vulnerable dependency path isolated and track the remaining GTK/glib warnings.

The [2026-10-02 benchmark receipt](benchmarks/2026-10-02-local.md) was measured under high CPU contention. It is reproducible evidence of that run, not a before/after performance claim.

On main, `cargo xtask benchmark` now retains five raw timing samples and reports
min/median/max alongside means. [The methodology](benchmarks/README.md) records
the separate timer boundaries and optimizer barriers; historical aggregate
timers are not directly comparable. Quiet-machine and native GUI measurements
remain open gates.
