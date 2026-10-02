# Contributing to MobaRust

Start with the [roadmap](ROADMAP.md), [architecture](docs/architecture.md), and
[safe testing policy](docs/security/safe-testing.md). Finish an existing
workflow or reproduce a concrete bug before adding a new protocol surface.

## Setup

Install Rust stable (1.90 or newer), Node.js 22, pnpm 10, and the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS.
Clone the repository and install the locked frontend dependencies:

```bash
git clone https://github.com/OthmaneBlial/MobaRust.git
cd MobaRust
pnpm install --dir apps/desktop --frozen-lockfile
pnpm --dir apps/desktop dev
```

The browser preview uses synthetic data. To work on native IPC and protocols,
run `pnpm --dir apps/desktop tauri dev`. Keep development profiles and secrets
in a dedicated disposable environment; never use production hosts for tests.

The required Windows resource icon is included in the checkout. When changing
the source `icon.png`, regenerate it with
`pnpm --dir apps/desktop tauri icon src-tauri/icons/icon.png` and commit the
updated `icon.ico`; other platform icon derivatives remain generated assets.

## Validate a change

```bash
cargo xtask check
cargo xtask pre-push-check
```

`check` runs formatting, workspace tests/Clippy, frontend unit tests,
TypeScript/lint/build, isolated helper checks, package-layout fixtures, and
fuzz-target compilation. Every child receives an isolated HOME/XDG and a
sanitized credential environment. The full suite needs the local OpenSSH/X11
prerequisites in the [SSH lab guide](docs/testing/ssh-lab.md); it can also need
network access for uncached dependencies. It does not install system packages.

For a focused SSH change, run `cargo xtask test-ssh`. For frontend changes,
use the `test:unit`, `check`, `lint`, and `build` scripts in `apps/desktop`.
Release asset validation is `node --test tools/release-assets.test.mjs`.
The [fuzzing guide](docs/testing/fuzzing.md) and [benchmark guide](benchmarks/README.md)
describe the existing isolated tools.

`cargo xtask check-rust` runs just workspace formatting, helper staging,
tests, and Clippy. GitHub Quality and installer workflows are disabled at the
maintainer's request. Quality has no push/PR triggers; use the local commands
above. The preserved [Quality recipe](.github/workflows/quality.yml) combines
frontend/release-asset checks with native Ubuntu, macOS, and Windows checks
if explicitly re-enabled for future manual validation.
The Linux recipe installs zsh and fish; Unix PTY checks
record each installed shell's version and explicitly report missing variants.
PowerShell and cmd each perform three consecutive ConPTY round trips; every
startup must pass, with no retry after a failure.
Successful test output is retained so optional fixture skip messages
remain visible in CI logs. The full local command additionally checks
experimental helpers and fuzz targets. CI fixtures do not establish GUI, hardware, installer, or
external-server interoperability. Quality never publishes installers.

The payload audit checks the established main-branch workflow and secret/path
boundaries; it does not push. Contributors working on topic branches can use
the focused tests and `git diff --check`, and leave the main-branch payload
audit to the maintainer.

## Pull requests and reports

- Explain the concrete failure or workflow and resulting behavior. Include
  reproduction steps, tests run, and any platform or fixture limitations.
- Add a regression that fails before a nontrivial bug fix. Test failure,
  cancellation, trust, and cleanup paths when they are part of the change.
- Keep changes scoped. Reuse native/Rust primitives and existing dependencies;
  preserve typed IPC, bounded I/O, host verification, and explicit approvals
  for remote execution or destructive file actions.
- Update documentation when behavior changes. A passing fixture does not
  justify a claim of production readiness or hardware interoperability.
- Never include credentials, private keys, personal SSH configuration, real
  server inventories, terminal transcripts, or unredacted logs in a PR/issue.

Bug reports should include OS/architecture, app version or commit, expected
and observed behavior, and a minimal reproduction with synthetic targets.
For security reports, follow [SECURITY.md](SECURITY.md).
