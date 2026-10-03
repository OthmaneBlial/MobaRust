# Dependency audit record

## SFTP file-read cancellation and negotiated limits — 2026-10-03

After v0.1.23, a small in-memory peer reproduced the high-level file reader
panicking when a cancelled four-byte read resumed into a one-byte buffer.
Seeking with the retained request produced the same overflow into a two-byte
buffer. An empty read incorrectly waited for a response, and a server-advertised
512-byte read limit made a 128-byte-configured client request 256 DATA bytes
instead of its 119-byte payload budget. No large allocation or socket was used.

The shared vendored file reader now keeps at most one response in a standard
`Cursor<Vec<u8>>`, copies only what fits and advances position by the delivered
bytes. Consuming a reply frees that buffer. Successful seek and accepted write
discard pending/buffered read state; handle-close completion discards it and
subsequent reads fail with a static closed-file error. Empty reads/writes leave
pending and buffered data untouched. Requested DATA is capped by the server's
read limit and the effective packet payload budget; a zero payload budget fails
explicitly rather than returning false EOF. The server's 64-bit packet limit is
clamped before narrowing to the client's 32-bit configuration.

```sh
cargo test --locked -p mobarust-ssh --test sftp_bounds
cargo xtask check
```

All 11 focused checks pass. The new file cases verify exact byte order/offsets,
empty I/O, pending/buffered seek and write, acknowledged closure, client/server
limit combinations, a 64-bit limit above `u32::MAX`, and a tiny budget that leaves
no room for DATA. The closure case first caught the intermediate buffered reader
returning bytes after shutdown; it now fails closed. Test peers use bounded
in-memory pipes and tiny legal replies. No dependency version, feature or lockfile
changed; the patch record now identifies five changed production files.

An initial complete check failed at the existing X11 fixture's five-second
channel wait, with xauth reporting `finished=0`. The unchanged X11 case passed
on recheck. No X11 deadline or assertion was changed; its intermittent cause
remains unproven and this is not an X11 runtime correction.

The final unchanged-source `cargo xtask check` completed successfully on macOS
ARM64 / Apple M2: workspace tests and Clippy, frontend tests/type checking/lint/
build, release/lab tooling, isolated RDP/VNC fixtures, package-layout contracts
and fuzz compilation. It includes 105 desktop tests, 42 SSH unit tests, 15
automated authentication cases, these 11 boundary checks and 16 OpenSSH cases.
Three manual labs remain ignored; real Xvfb reports its prerequisite skip.

This is public-API regression evidence, not native cancellation/retry acceptance
or an independent security audit. These later source changes are not in the
v0.1.23 Mac or v0.1.12 Windows/Linux installers. Wire-request bookkeeping still
retires on reply, timeout or stream closure; immediate removal for every dropped
request future was still a separate audit target in that source snapshot; the
next section records its subsequent correction. GitHub CI remains disabled.

## SFTP request lifetime and late replies — 2026-10-03

The next audit reproduced reply slots remaining registered after dropped async
read futures and abandoned nowait write/close acknowledgments. A zero-second
request timeout removed its slot, but its valid late DATA reply then stopped the
reader and cancelled another live request. Three private baseline checks and a
public in-memory late-reply recovery check failed before the correction.

Each raw request now owns a private future wrapping the existing oneshot receiver.
Dropping that future closes its receiver and removes its abandoned reply slot;
a conditional removal preserves a newer live receiver if an ID has been reused.
This applies to shared async requests, pending file writes and the close request
sent by file drop. There is no new dependency, cleanup task, global sweep or
retained cancellation-ID list. Public signatures and dependency locks are unchanged.

After successful VERSION negotiation, a parsed ordinary reply without a matching
request is discarded. It cannot satisfy a different pending request. This also
covers arbitrary unmatched ordinary IDs; they are not distinguished from late
responses. Frame-size and deserialization limits still run before dispatch.
Responses before VERSION, unsolicited/duplicate VERSION, malformed frames and
oversized declarations still stop both workers and settle pending requests.
Cancellation releases local bookkeeping; it cannot undo operations already
transmitted to a remote server. File close/flush must still be awaited to observe
write errors and acknowledged closure.

```sh
cargo test --locked -p russh-sftp --lib
cargo test --locked -p mobarust-ssh --test sftp_bounds
cargo xtask test-ssh
cargo xtask check
```

The dependency remains excluded from workspace test discovery, so `test-ssh`
and the Rust/full-check path explicitly run its private library tests in the
existing disposable HOME/environment. The five private checks exercise slot
ownership, queued acknowledgments, timeout/late-reply isolation, initialization
refusal and receiver replacement. The public pipe checks exercise actual worker
continuation and fail-closed initialization, alongside the existing response
and file-reader boundaries. No sockets, credentials or filesystem state are
needed by these focused checks.

The full `cargo xtask check` passed on macOS ARM64 / Apple M2, including
all five private SDK tests, all 13 SFTP boundary cases, 42 SSH unit tests,
15 automated authentication cases and all 16 OpenSSH integration cases.
Workspace Clippy, frontend tests/type checking/lint/build, release/lab tooling,
RDP/VNC fixtures, package-layout contracts and fuzz compilation also passed.
The X11 loopback case passed on this run without changed deadlines/assertions;
this does not establish the cause of its earlier intermittent failure. Three
manual labs remain ignored; real Xvfb reports its explicit prerequisite skip.

These are source changes after v0.1.23, pending installers and native GUI
acceptance. They do not establish remote operation rollback, general server
compatibility or an independent security audit. GitHub CI remains disabled.

## SFTP file-write packet budgets and closed handles — 2026-10-03

The subsequent upload audit reproduced a 128-byte-configured file writer choosing
all 256 caller bytes when the server advertised a 512-byte write limit. A tiny
configuration with no room for WRITE data accepted a nonempty write as zero bytes,
and a successfully closed file still accepted another byte for transmission.
All three new regression cases failed before the correction.

The shared file writer now caps each chunk by the negotiated data limit, the
effective client packet payload budget and the server's complete encoded-packet
limit. Its overhead includes the opaque handle length; the existing raw server
limit also includes the four-byte packet length prefix. Limit arithmetic stays
in `u64` until the final configured-client bound permits narrowing. A nonempty
write with no payload room returns `InvalidInput` without sending WRITE or
advancing position; a closed handle returns `BrokenPipe`. Empty writes still
return zero without issuing a request.

```sh
cargo test --locked -p mobarust-ssh --test sftp_bounds
cargo xtask check
```

All 16 focused boundary checks pass. The new upload peer checks five client/server
limit combinations, including a server packet limit above `u32::MAX`, a smaller
data limit and an absent data limit. A 256-byte sequence is split into exact
100-, 32- or 8-byte chunks as appropriate, acknowledged, reconstructed byte for
byte and closed. Every encoded WRITE is checked against the packet limit, handle,
exact offset and remaining byte count. Tiny-budget and closed-handle refusal
checks also verify unchanged position and no extra wire request. All peers are
bounded in-memory pipes; no sockets, credentials or personal files are used.
No dependency version, feature, lockfile or public API changed.

The subsequent full `cargo xtask check` passed on macOS ARM64 / Apple M2,
including five private SDK tests, these 16 public boundary cases, 42 SSH unit
tests, 15 automated authentication cases and all 16 OpenSSH cases. Workspace
Clippy, frontend tests/type checking/lint/build, release/lab tooling, RDP/VNC
fixtures, package-layout contracts and fuzz compilation passed. Three manual
labs remain ignored, and real Xvfb retains its explicit prerequisite skip.
No X11 deadline or assertion was changed.

These are source changes after v0.1.23, pending new installers and native upload
acceptance. They do not establish GUI throughput or broader server compatibility.
GitHub CI remains disabled.

## SFTP inbound response bounds on main — 2026-10-03

The workspace now uses a [repository-local russh-sftp 2.4.0 copy](../../vendor/russh-sftp/MOBARUST_PATCH.md).
The original client reader passed `u32::MAX` to its allocation helper rather
than the existing configured packet limit. It also continued after malformed
frames and retained pending request senders after reader termination. The local
correction enforces the default 256 KiB payload limit before allocation,
rejects impossible sequence counts before visitor reservation, settles pending
requests and stops both stream workers. DATA must fit the requested read length.
Both SFTP channel paths use the same corrected dependency.

`sftp_bounds` reproduced acceptance of a valid 33-byte VERSION against a
32-byte configured limit, delivery of an impossible sequence count to a visitor,
and a header-only response leaving INIT pending beyond the two-second regression
deadline. A separate check reproduced acceptance of five DATA bytes for a
four-byte request. These are small in-memory tests; no excessive allocation or
public network listener was used. Provenance and changed-file scope are recorded
with the patch. This source correction is included in both [v0.1.23 Mac previews](../release/v0.1.23.md);
older Mac and v0.1.12 Windows/Linux installers do not include it. It is not a
new RustSec advisory claim.

`cargo audit --no-fetch --json` checks the resulting lockfile against the
cached 1,288-advisory database: zero reported vulnerabilities, with the same
`RUSTSEC-2024-0370` and `RUSTSEC-2024-0429` warnings. The lockfile changes only
russh-sftp's registry source/checksum to the local path; dependency versions
and features are unchanged. This version check does not audit the local patch.

The five in-memory response-bound checks pass. The generated encrypted SSH
fixture also rejects a header-only oversized packet on both SFTP channel paths,
requires their closure before transport disconnect and successfully opens a
fresh echo shell on the same authenticated connection. The complete local
`cargo xtask check` passes, including all 16 OpenSSH cases, workspace Clippy,
frontend checks, protocol fixtures, package-layout contracts and fuzz compilation.
The real Xvfb check reports its prerequisite skip. Native GUI and cross-platform
runtime acceptance remain pending; no roadmap checkbox was closed by this patch.

## SFTP seek recovery and offset exhaustion — 2026-10-03

Source after v0.1.24 corrects another boundary in the shared SFTP file adapter.
Relative seeks previously narrowed unsigned offsets or metadata sizes to `i64`
before adding the delta. Valid large positions could be rejected, while signed
addition could panic or wrap. A failed end-relative seek also retained its
completed future; querying the position or seeking again reproduced
`async fn resumed after completion`.

Seeks now use checked unsigned arithmetic and return `InvalidInput` for overflow
or movement before zero. Completed seek state is retired on both success and
failure; failed metadata or arithmetic leaves the old position and read state
intact. READ/WRITE chunks also fit the remaining representable offset space.
Nonempty I/O at the maximum position fails before another data request, while
empty I/O remains a no-op. There is no new dependency or public signature.

```text
cargo test --locked -p mobarust-ssh --test sftp_bounds
cargo xtask check
```

Both seek regressions failed on the baseline: one rejected a valid unsigned
position and the other panicked when polling a completed failed future. A third
baseline check exposed a two-byte READ where only one byte could advance the
offset safely. All 19 public in-memory boundary cases pass with the correction.
The new cases use tiny generated packets, signed-minimum deltas, FSTAT refusal
and absent/maximum sizes, exact offsets and no-extra-request assertions. No huge
file, socket, credentials or filesystem fixture is involved.

The complete local `cargo xtask check` also passed on macOS ARM64 / Apple M2:
105 desktop tests, five private SDK checks, all 19 public SFTP boundary cases,
15 automated authentication cases and all 16 OpenSSH cases, plus workspace
Clippy, frontend tests/type checking/lint/build, release/lab tooling, isolated
RDP/VNC fixtures, package-layout contracts and fuzz compilation. Three manual
authentication labs remain ignored; real Xvfb reports its prerequisite skip.
No existing fixture assertion, payload or deadline was relaxed.

This is protocol-adapter evidence, not native app or hardware acceptance. The
published v0.1.24 Mac DMGs and v0.1.12 Windows/Linux installers are unchanged;
the correction awaits a subsequent installer cohort. Baseline-version advisory
scanners do not independently audit this repository-local patch.

## SFTP file-type classification after v0.1.24 — 2026-10-03

The local SDK used overlapping POSIX type codes as flags, misclassifying links
as regular files and sockets/block devices as directories. Assignment/removal
and Unix metadata conversion could also corrupt the type. A private OpenSSH
regression reproduced mode `0600` becoming `0755` when replacing a link;
three public metadata regressions failed, and an authenticated memory-only
SFTP fixture reproduced promotion without type metadata.

Exact type-field handling and a shared no-follow upload guard now preserve
regular-file modes, replace final-path links themselves and refuse unsupported
occupied types before rename. [Commands, coverage and limitations](../adr/0008-sftp-transfer-pipeline.md#upload-destination-regressions--2026-10-03)
separate protocol checks from pending native/cross-platform acceptance. The
dependency version/features are unchanged and published downloads do not
contain this source correction. This is not an independent audit or new
RustSec advisory claim.

## SFTP file-I/O status preservation after v0.1.24 — 2026-10-03

The file adapter flattened READ/FSTAT/WRITE/fsync/CLOSE failures into strings,
discarding the status code. An authenticated SFTP fixture reproduced a close
permission denial being reported as generic SFTP I/O failure. The local SDK
now retains the structured client error inside `io::Error`; MobaRust recovers
its existing typed/redacted status classification without inspecting server text.
Read/write denial, close denial across transfer/editor callers, cancellation
during close and the existing 19 packet/offset regressions pass locally.
[Commands, ownership checks and limits](../adr/0008-sftp-transfer-pipeline.md#private-transfer-parts-and-close-acknowledgements--2026-10-03)
remain separate from native/cross-platform acceptance. Versions/features and
published downloads are unchanged; this is not a new advisory or independent audit.

## v0.1.24 release preparation audit — 2026-10-03

A fresh `cargo audit --json` lookup reports RustSec commit
`f8dee89e1b2f2f1eaf548312df7655fe5202a302`, containing 1,288 advisories.
The version-aligned workspace (638 packages) reports zero vulnerabilities and
retains `RUSTSEC-2024-0370` (`proc-macro-error`, unmaintained) and
`RUSTSEC-2024-0429` (`glib`, unsoundness). Checks of the VNC helper (82 packages)
and fuzz lockfile (52 packages) against the same refreshed database report no
vulnerability or warning. The isolated RDP helper (374 packages) still reports
`RUSTSEC-2023-0071` (`rsa`) plus unmaintained `atomic-polyfill` and
`rustls-pemfile`; it remains excluded from normal installers. No advisory was
suppressed. Semantic comparison of all four lockfiles confirms that only local
package versions changed from 0.1.23 to 0.1.24; third-party entries are unchanged.

The SFTP corrections have focused regression evidence. Advisory scanning of the
baseline dependency version does not independently review the local source patch.
This audit does not establish installer publication or native GUI acceptance.
The separate [v0.1.24 release receipt](../release/v0.1.24.md) records both verified
Mac DMGs and anonymously downloaded, byte-matched public files. Native GUI and
Windows/Linux acceptance remain open.

## v0.1.22 release recheck — 2026-10-03

`cargo audit --json` refreshed RustSec to commit
`f8dee89e1b2f2f1eaf548312df7655fe5202a302`, containing 1,288 advisories.
The version-aligned workspace reports zero vulnerabilities and the same two
warnings: `RUSTSEC-2024-0370` (`proc-macro-error`, unmaintained) and
`RUSTSEC-2024-0429` (`glib`, unsoundness). Cached checks report no vulnerability
or warning for the VNC helper. The isolated RDP helper still reports
`RUSTSEC-2023-0071` and two unmaintained dependencies, and remains excluded
from normal installers. No advisory was suppressed. Semantic comparison of
all four lockfiles confirms unchanged third-party entries; only local package
versions moved from 0.1.21 to 0.1.22. The new SSH packet-loop and shell-output
corrections have regression evidence, not an independent security audit.

## v0.1.21 release recheck — 2026-10-03

`cargo audit --json` refreshed RustSec and reports database commit
`f8dee89e1b2f2f1eaf548312df7655fe5202a302` with 1,288 advisories. The
version-aligned workspace reports no vulnerability and the same two warnings:
`RUSTSEC-2024-0370` (`proc-macro-error`, unmaintained) and `RUSTSEC-2024-0429`
(`glib`, unsoundness). Cached checks against that refreshed database report no
vulnerability or warning for the VNC helper. The separate RDP helper still
reports `RUSTSEC-2023-0071` and two unmaintained dependencies and remains
excluded from normal installers. No advisory was suppressed. A comparison of
all four lockfiles confirms that every third-party package entry is unchanged;
only repository-local package versions moved from 0.1.20 to 0.1.21.

## v0.1.20 release recheck — 2026-10-02

`cargo audit --json` refreshed RustSec to commit
`f8dee89e1b2f2f1eaf548312df7655fe5202a302`, containing 1,288 advisories.
The version-aligned workspace reports zero vulnerabilities and the same two
warnings: `RUSTSEC-2024-0370` (`proc-macro-error`, unmaintained) and
`RUSTSEC-2024-0429` (`glib`, unsoundness). Cached checks against that refreshed
database report no vulnerability or warning for the VNC helper. The isolated
RDP helper retains `RUSTSEC-2023-0071` and two unmaintained dependencies; it is
excluded from normal installers. No advisory was suppressed. Only local package
versions changed in the release lockfiles; third-party dependencies are unchanged.

## v0.1.19 release recheck — 2026-10-02

`cargo audit --json` refreshed RustSec before release preparation. The database
remains at commit `117edb3bed98e9be112f277b7615eea3252e7c43`, with 1,280
advisories. After aligning the workspace, fuzz and helper lockfiles to 0.1.19,
cached checks reported zero workspace vulnerabilities and the same two warnings
(`proc-macro-error` unmaintained and GTK3/`glib` unsoundness). The VNC helper
reported no vulnerability or warning. The isolated RDP helper still reported
one RSA timing vulnerability and two unmaintained dependencies; it remains
excluded from normal installers. No advisory was suppressed. Only local
package versions changed in the lockfiles; third-party dependencies are unchanged.

## Interactive SSH packet-reader patch — 2026-10-02

The workspace uses a [repository-local russh 0.63.3 copy](../../vendor/russh/MOBARUST_PATCH.md)
so its packet reader can process peer disconnects while an interactive response
is pending. The root dependency still disables RSA. Archive provenance,
production-source omissions and the narrow indexed-parser marker allowance
are documented with the patch. This source change was added after v0.1.18 and is included in the v0.1.19
Mac preview. A new `cargo audit --no-fetch --json` check of this lockfile against the cached
1,280-advisory database reports zero vulnerabilities and the same two warnings:
`proc-macro-error` unmaintained and `glib` unsoundness. No advisory was suppressed.
This version/database check does not audit the local patch implementation.

## Tauri fullscreen update — 2026-10-02

The workspace now uses Tauri 2.12.1 with matching frontend API/CLI 2.12.1,
Wry 0.57.0 and the corresponding Tauri runtime/build crates. This fixes the
macOS HTML fullscreen opt-in at the existing runtime boundary; macOS 12.3+
uses WebKit's public element-fullscreen preference. See the
[upstream correction](https://github.com/tauri-apps/tauri/commit/c9a3cb892e901e39ca46aad2ff6b14aac21fea0d).
The new Tauri crates require Rust 1.90; workspace metadata and contributor
instructions now state that minimum. Local validation uses Rust 1.95.0.

RustSec database commit `117edb3bed98e9be112f277b7615eea3252e7c43`
(1,280 advisories) reports **no workspace vulnerability and two warnings**:
unmaintained `proc-macro-error 1.0.4` and GTK3/`glib 0.18.5` unsoundness.
Five unmaintained Unicode dependencies disappeared with this runtime update.
The VNC helper remains free of reported advisories; the unchanged RDP helper
still has the RSA timing advisory and stays excluded from normal bundles.
No advisory was suppressed. This is a dependency check, not a security guarantee.

## Earlier source recheck — 2026-10-02

With RustSec refreshed to 1,280 advisories (database commit
`117edb3bed98e9be112f277b7615eea3252e7c43`), the workspace still reports no
vulnerability and seven warnings (six unmaintained crates and the transitive
GTK3/glib unsoundness advisory). The VNC helper reports no vulnerability or
warning. The RDP helper still reports `rsa 0.10.0-rc.18` /
`RUSTSEC-2023-0071` and two unmaintained crates; it remains excluded from normal
bundles. These read-only checks did not modify any dependency lockfile.

## Desktop preview recheck — 2026-09-26

The refreshed RustSec database contains 1,271 advisories (last updated
2026-09-25). The workspace lockfile now uses `serialport 4.10.1` and `wnaf
0.14.1`, replacing two yanked releases. The RDP helper lockfile now uses
`cryptoki 0.12.1` and `rustls 0.23.45`, which fix the newly reported
`cryptoki` out-of-bounds read and TLS handshake advisories
([RUSTSEC-2026-0286](https://rustsec.org/advisories/RUSTSEC-2026-0286),
[RUSTSEC-2026-0285](https://rustsec.org/advisories/RUSTSEC-2026-0285)).

## Results

| Lockfile | Result | Interpretation |
| --- | --- | --- |
| Workspace `Cargo.lock` (638 packages, Tauri update) | Exit 0; no vulnerability reported | Two warnings remain: unmaintained `proc-macro-error 1.0.4` and GTK3/`glib 0.18.5` unsoundness (`RUSTSEC-2024-0429`). No yanked release remains in this lockfile. |
| `tools/vnc-helper/Cargo.lock` (82 packages) | Exit 0; no warning or vulnerability reported | The isolated VNC helper passes the advisory check. This does not replace cross-platform interoperability evidence. |
| `tools/rdp-helper/Cargo.lock` (374 packages) | Exit 1; one vulnerability | `cryptoki 0.12.1` and `rustls 0.23.45` clear the two newly fixed advisories. `rsa 0.10.0-rc.18` still triggers `RUSTSEC-2023-0071` (Marvin timing attack); no fixed release is available. Two transitive crates are also reported as unmaintained. |

The RDP helper is therefore not staged into normal application bundles and is
not a production RDP claim. Its separate lockfile and audit are intentional.
No advisory is suppressed to make these results appear clean.

## Reproduce locally

```text
cargo audit
cargo audit --no-fetch --file tools/vnc-helper/Cargo.lock
cargo audit --no-fetch --file tools/rdp-helper/Cargo.lock
```

The commands are read-only with respect to the repository. They inspect lock
files and the cached advisory database; they do not read SSH files, query the
SSH agent, access Keychain entries, or connect to a remote protocol server.

## Historical checks

The 2026-09-07 check found 18 workspace warnings, including the now-replaced
yanked `wnaf 0.14.0`. On 2026-08-31, the advisory database contained 1,226
entries and the RDP lockfile still had the same RSA advisory. These are
historical snapshots; use the current results above for release decisions.

## Follow-up policy

- Do not suppress the RDP advisory merely to make the quality command green.
- Reconsider the RDP engine only after an audited dependency path, certificate
  policy, and real Windows interoperability evidence exist.
- Track the transitive GTK3/`glib` warnings when Tauri/Wry offers a compatible
  maintained replacement; do not replace the desktop shell without a measured
  platform and packaging review.
- Treat a clean advisory result as one input to release review, not as a full
  security audit of application code or protocol behavior.
