# Desktop packaging boundary

MobaRust packages the native RDP and VNC helpers as isolated resources. The
desktop process resolves them from its Tauri resource directory and never
launches a helper from an arbitrary user-provided path.

## Local staging

From `apps/desktop`, the Tauri build hook runs:

```text
sh ../../tools/prepare-desktop.sh
```

That command builds the two separate helper workspaces in release mode and
copies only their current-platform executables into the ignored
`apps/desktop/src-tauri/helpers/` directory. Staging is repository-scoped and
does not start an application protocol session, read credentials, or inspect
personal configuration. Cargo may use its configured package registry when a
dependency is not cached. The generated files must never be committed.

`cargo xtask stage-helpers` can be run directly when inspecting the staging
step. `cargo xtask check` remains the normal validation command and does not
turn the application into portable mode.

`cargo xtask package-check` builds an unsigned current-platform debug `.app`
bundle and verifies that the Tauri resource step completes. It is a packaging
smoke test, not code-signing, notarization, or interoperability evidence.

## Distribution matrix

| Target | Helper build | Package evidence | Signing evidence |
| --- | --- | --- | --- |
| Windows x64 | Required on Windows | Pending real Windows build | Pending owner certificate |
| Linux x64 | Required on Linux | Pending distro/package checks | Pending signing policy |
| macOS ARM64 | Builds on this host when staged | Pending notarized artifact | Pending Developer ID/notarization |
| Windows ARM64 | Cross-build/toolchain required | Pending | Pending |
| Linux ARM64 | Cross-build/toolchain required | Pending | Pending |
| macOS x64 | Cross-build/toolchain required | Pending | Pending |

Packaging is not a claim that RDP/VNC interoperability is complete. Real
server tests, platform-specific input/clipboard/display behavior, dependency
licenses, signed artifacts, checksums, and clean-install verification remain
release gates. No signing keys or certificates belong in this repository.
