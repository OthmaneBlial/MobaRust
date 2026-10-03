# Windows GNU compile receipt — 2026-10-03

Current source `549b3066234f2e7775ed0350b421897f8a9d5ab7` passed locked
Windows x64 GNU compile checks on a macOS host. This is source portability
evidence; it does not execute Windows tests or produce a Windows installer.

## Host and toolchain

- Apple M2, macOS 26.6, host `aarch64-apple-darwin`.
- Rust 1.95.0 (`59807616e`), target `x86_64-pc-windows-gnu`.
- Homebrew MinGW GCC 16.2.0 and NASM 3.02.
- Two Cargo build jobs, offline cached dependencies, unchanged lockfiles.

The initial workspace attempt stopped in `aws-lc-sys 0.44.0` because NASM
was missing. Installing NASM resolved that prerequisite; no cryptographic
features, assembler bypasses, dependencies or source code were changed.
The existing unused-variable warning in vendored russh's `pkcs5.rs` remained
visible. These checks do not establish a warning-free Clippy result.

## Commands and results

These are the Cargo arguments used inside the isolated environment:

```bash
cargo check --locked --workspace --target x86_64-pc-windows-gnu
cargo check --locked --workspace --all-targets --target x86_64-pc-windows-gnu
cargo check --locked --all-targets --manifest-path tools/vnc-helper/Cargo.toml \
  --target x86_64-pc-windows-gnu --target-dir target
```

| Check | Result | Elapsed time |
| --- | --- | --- |
| Workspace | Exit 0 | 275.99 seconds |
| Workspace, including test/benchmark/example targets | Exit 0 | 57.83 seconds |
| Separate VNC helper, including tests | Exit 0 | 81.75 seconds |

`--all-targets` type-checks Windows-specific test code, including the repeated
PowerShell/cmd ConPTY fixtures. It does **not** link or run those tests.
The separate VNC and experimental RDP helper workspaces are not included in
`--workspace`; the third command checks VNC explicitly. Experimental RDP was
not cross-checked and remains excluded from normal app bundles.

## Isolation and reproducibility

Each invocation received a new temporary HOME/ZDOTDIR/XDG directory inside
the ignored project `target` directory. The environment was rebuilt from an
allowlist: compiler PATH, public Cargo/rustup caches, locale, build settings,
and the temporary home. SSH-agent variables were empty; `ENV`, `BASH_ENV`,
and global Git configuration pointed to `/dev/null`. The temporary home was
removed after the terminal Cargo result.

The cross-tool configuration used these variables, pointing to the installed
Homebrew executables:

| Variable | Executable |
| --- | --- |
| `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER` | `x86_64-w64-mingw32-gcc` |
| `CC_x86_64_pc_windows_gnu` | `x86_64-w64-mingw32-gcc` |
| `AR_x86_64_pc_windows_gnu` | `x86_64-w64-mingw32-ar` |
| `WINDRES` | `x86_64-w64-mingw32-windres` |

NASM must be available on that compiler PATH. Reproduce the check with the
same target/toolchain and a disposable environment following the
[safe testing policy](../security/safe-testing.md). Cached dependencies are
required for offline execution. No SSH fixture, personal SSH configuration,
key, agent, public listener or Windows VM was used for these compile checks.

## Remaining acceptance

The earlier [native Quality run](https://github.com/OthmaneBlial/MobaRust/actions/runs/36998538983)
passed at `ac70e39c98b54d8f2a444b0c4b3232486fd6e3bf`. Its source scope is
separate from this receipt. Quality and installer workflows remain disabled.

Current Windows MSVC builds, test execution, repeated ConPTY startup,
WSL discovery/launch, WebView2 GUI behavior, application shutdown and
install/uninstall still require a dedicated Windows runtime. Windows
downloads remain v0.1.12. This receipt does not complete the cross-platform
roadmap gate or change any release tag/asset.
