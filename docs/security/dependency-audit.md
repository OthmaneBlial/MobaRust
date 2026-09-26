# Dependency audit record

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
| Workspace `Cargo.lock` (656 packages) | Exit 0; no vulnerability reported | Seven warnings remain: six unmaintained transitive Unicode/proc-macro crates and the GTK3/`glib 0.18.5` unsoundness advisory `RUSTSEC-2024-0429`. No yanked release remains in this lockfile. |
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
