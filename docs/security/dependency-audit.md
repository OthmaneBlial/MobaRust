# Dependency audit record

This is a record of the repository-local `cargo audit --no-fetch` checks run on
2026-08-30 with the cached RustSec advisory database. It is an engineering
snapshot, not a permanent guarantee: refresh the advisory database and rerun
the checks before a release.

## Results

| Lockfile | Result | Interpretation |
| --- | --- | --- |
| Workspace `Cargo.lock` | Exit 0; no vulnerability reported | 17 allowed maintenance/unsoundness warnings remain in the transitive GTK3/`glib` stack used by the Tauri/Wry Linux path. |
| `tools/vnc-helper/Cargo.lock` | Exit 0; no warning or vulnerability reported | The isolated VNC helper passed the cached advisory check. This does not replace cross-platform interoperability evidence. |
| `tools/rdp-helper/Cargo.lock` | Exit 1; one vulnerability | `rsa 0.10.0-rc.18`, `RUSTSEC-2023-0071` (Marvin timing attack), is pulled through the pinned IronRDP/`picky` chain and has no fixed upgrade available in this candidate. |

The RDP helper is therefore not staged into normal application bundles and is
not a production RDP claim. Its separate lockfile and audit are intentional.

## Reproduce locally

```text
cargo audit --no-fetch
cargo audit --no-fetch --manifest-path tools/vnc-helper/Cargo.toml
cargo audit --no-fetch --file tools/rdp-helper/Cargo.lock
```

The commands are read-only with respect to the repository. They inspect lock
files and the cached advisory database; they do not read SSH files, query the
SSH agent, access Keychain entries, or connect to a remote protocol server.

## Follow-up policy

- Do not suppress the RDP advisory merely to make the quality command green.
- Reconsider the RDP engine only after an audited dependency path, certificate
  policy, and real Windows interoperability evidence exist.
- Track the transitive GTK3/`glib` warnings when Tauri/Wry offers a compatible
  maintained replacement; do not replace the desktop shell without a measured
  platform and packaging review.
- Treat a clean advisory result as one input to release review, not as a full
  security audit of application code or protocol behavior.
