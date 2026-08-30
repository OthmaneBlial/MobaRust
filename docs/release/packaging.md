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
bundle and verifies that the Tauri resource step completes. The bundle
configuration includes the versioned MobaRust mark from
`apps/desktop/src-tauri/icons/icon.png`, so the application icon is generated
into the platform bundle rather than remaining a source-only asset. On macOS
it also checks the app executable and the shippable VNC helper resource is a
regular file inside the bundle, while asserting that the unshippable RDP
candidate is absent. It writes a `MobaRust.sha256` manifest beside the generated
bundle and verifies it immediately; the manifest uses the portable two-space
SHA-256 format accepted by both `sha256sum` and macOS `shasum`, and covers every
regular file in the artifact scope. It is a packaging smoke test, not code-signing,
notarization, or interoperability evidence.

An already assembled artifact can be checked without rebuilding it:

```text
cargo xtask verify-checksum <artifact-dir> <manifest>
```

Both paths are explicit. The verifier requires a regular manifest file, rejects
symlinks and unsupported file types anywhere in the artifact, and recomputes
the complete deterministic manifest before reporting success. It proves local
artifact integrity only; it is not a signature verifier and does not establish
publisher authenticity.

On 2026-08-30 this smoke test passed on the local macOS ARM64 host. The bundle
was created at `target/debug/bundle/macos/MobaRust.app`; its main executable
and the staged VNC helper resource (`mobarust-vnc-helper`) were verified as
executable Mach-O arm64 files. The RDP candidate was intentionally not staged
because its separate lockfile currently fails the RSA timing-advisory audit.
This is repository-local assembly evidence only; it does not prove a clean
install, notarization, or remote-desktop interoperability.

At runtime, the desktop parent also opens helper resources with
`symlink_metadata` and rejects symlinks, directories, and other non-regular
files before spawning a protocol process. This complements the bundle smoke
test; it is an integrity boundary against local resource substitution, not a
replacement for signed distribution or operating-system protection.

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

Portable vault writes also refuse a symlink or directory at the vault path
before reading or replacing it. This protects the marker-gated portable data
directory from redirecting credential storage to an unintended local target;
it does not protect against a fully compromised operating system.

The checksum manifest is integrity evidence for an assembled artifact, not an
authenticity guarantee. Release automation must sign the manifest (or an
equivalent release metadata file) with the project-approved signing system,
publish it next to the exact package, and verify it from a clean install
environment. No generated manifest or signature is committed by the local
`package-check` command.
