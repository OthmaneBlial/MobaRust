# Dependency audit record

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
