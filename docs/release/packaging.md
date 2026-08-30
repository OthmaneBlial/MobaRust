# Desktop packaging boundary

MobaRust packages the native VNC helper as an isolated resource. The desktop
process resolves it from its Tauri resource directory and never launches a
helper from an arbitrary user-provided path. The IronRDP candidate remains an
isolated development helper and is deliberately excluded from normal bundles
until its dependency audit is clean.

## Local staging

From `apps/desktop`, the Tauri build hook runs:

```text
sh ../../tools/prepare-desktop.sh
```

That command builds the VNC helper workspace in release mode and copies only
its current-platform executable into the ignored
`apps/desktop/src-tauri/helpers/` directory. Staging is repository-scoped and
does not start an application protocol session, read credentials, or inspect
personal configuration. Cargo may use its configured package registry when a
dependency is not cached. The generated files must never be committed.

`cargo xtask stage-helpers` can be run directly when inspecting the staging
step. `cargo xtask check` remains the normal validation command and does not
turn the application into portable mode.

`cargo xtask package-check` builds an unsigned current-platform debug app
bundle and verifies that the Tauri resource step completes. On macOS it also
checks the app executable and both native helper resources are regular files
inside the bundle. It is a packaging smoke test, not code-signing,
notarization, or interoperability evidence.

On 2026-08-30 this smoke test passed on the local macOS ARM64 host. The bundle
was created at `target/debug/bundle/macos/MobaRust.app`; its main executable
and the staged VNC helper resource (`mobarust-vnc-helper`) were verified as
executable Mach-O arm64 files. The RDP candidate was intentionally not staged
because its separate lockfile currently fails the RSA timing-advisory audit.
This is repository-local assembly evidence only; it does not prove a clean
install, notarization, or remote-desktop interoperability.

## Distribution matrix

| Target | Helper build | Package evidence | Signing evidence |
| --- | --- | --- | --- |
| Windows x64 | Required on Windows | Pending real Windows build | Pending owner certificate |
| Linux x64 | Required on Linux | Pending distro/package checks | Pending signing policy |
| macOS ARM64 | Local unsigned `.app` smoke test passed; clean install pending | Pending notarized artifact | Pending Developer ID/notarization |
| Windows ARM64 | Cross-build/toolchain required | Pending | Pending |
| Linux ARM64 | Cross-build/toolchain required | Pending | Pending |
| macOS x64 | Cross-build/toolchain required | Pending | Pending |

Packaging is not a claim that RDP/VNC interoperability is complete. Real
server tests, platform-specific input/clipboard/display behavior, dependency
licenses, signed artifacts, checksums, and clean-install verification remain
release gates. No signing keys or certificates belong in this repository.
