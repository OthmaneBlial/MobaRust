# Offline Linux ARM64 SSH runtime receipt — 2026-10-03

Source `ecb382ff6e19f6b7a0da5ef0122e9934e0215184` passed seven SSH test
executables in an offline Ubuntu ARM64 guest: **128 passed, 0 failed, 5 ignored**.
The executables were cross-compiled on macOS for
`aarch64-unknown-linux-musl`, then actually run under Linux. This establishes
backend runtime evidence; it does not establish a Linux desktop build,
WebKitGTK/GUI behavior, a glibc build, or an installer pass.

## Environment and image provenance

| Component | Observed configuration |
| --- | --- |
| Host | Apple M2, macOS 26.6 |
| Compiler | Rust 1.95.0; locked dependencies; two Cargo jobs; offline caches |
| C/assembly and linker | Zig 0.16.0_1; Rust's bundled rust-lld; matching Zig UBSan/compiler runtime archives |
| VM | QEMU 11.1.1, HVF, host CPU, two vCPUs, 1,536 MiB RAM |
| Guest | Ubuntu 24.04.4 LTS ARM64, kernel `6.8.0-137-generic` |
| SSH tools | OpenSSH 9.6p1 Ubuntu-3ubuntu13.18; OpenSSL 3.0.13 |

The base image was `ubuntu-24.04-server-cloudimg-arm64.img` from the
[official 20260814 image directory](https://cloud-images.ubuntu.com/releases/noble/release-20260814/).
Its 618,370,560 bytes matched SHA-256
`4a281a921b8d7db952895ab619736f10efe9f63e111fa5b5779ed18f023818aa`.
Before boot, `SHA256SUMS.gpg` verified `SHA256SUMS` with the pinned Ubuntu
cloud-image key `D2EB44626FDDC30B513D5BB71A5D6C4C7DB87C81` in a new private
GPG home. The fingerprint and verification procedure are documented in
[Ubuntu's image verification guide](https://ubuntu.com/docs/public-images/public-images-how-to/verify-image-checksum/).

The guest received a read-only NoCloud seed ISO containing the test
executables and their checksums. Both the ISO payload and its root-owned
execution copies passed checksum verification before any harness ran.

## Build and execution

These were the Cargo arguments used in the isolated cross-compiler environment:

```bash
cargo test --locked -p mobarust-ssh --no-run \
  --target aarch64-unknown-linux-musl \
  --config profile.dev.debug=0 --config profile.test.debug=0 \
  --message-format=json
```

Compilation completed with exit 0 in 186.15 seconds. Each resulting static
AArch64 ELF test executable ran as a disposable, non-root guest account with
`--test-threads=1 --nocapture`, a 300-second deadline and a sanitized environment.
Every harness had its own verified begin/end markers and exit status 0.

| Harness | Passed | Failed | Ignored | Harness time |
| --- | --- | --- | --- | --- |
| `authentication` | 16 | 0 | 4 | 40.69 s |
| `forwarding` | 4 | 0 | 0 | 24.53 s |
| `local_sshd` | 25 | 0 | 1 | 45.24 s |
| `mobarust_ssh` unit tests | 43 | 0 | 0 | 0.02 s |
| `remote_editor` | 15 | 0 | 0 | 24.05 s |
| `sftp_bounds` | 22 | 0 | 0 | 0.01 s |
| `sftp_metadata` | 3 | 0 | 0 | 0.00 s |
| **Total** | **128** | **0** | **5** | |

The five ignored tests are opt-in native desktop acceptance fixtures. The
optional real-Xvfb test additionally reported a missing-prerequisite skip;
its harness-level `ok` is included above, but no real X server ran. IPv6
known-hosts and both two-bastion 8 MiB shell/SFTP cases executed, including
the frequent-rekey variant. Nested child summaries used by permission tests
were not counted again in the totals.

The complete VM invocation took 271.65 seconds, including boot and shutdown.
That duration is a lab receipt, not an application performance benchmark.
With no network adapter, Ubuntu's network-online wait timed out during boot;
the separate test markers, statuses and final success marker verified the run.
No unexpected UBSan runtime diagnostics appeared.

## Network and credential boundary

QEMU ran with `-nic none`, no host forwarding, no shared host directories,
no monitor and a local serial console. [QEMU documents `-nic none`](https://www.qemu.org/docs/master/system/invocation.html)
as disabling network devices. The guest interface inventory contained only
loopback. All SSH, tunnel and fixture traffic used guest `127.0.0.1` or `::1`;
the host QEMU process had no TCP/UDP sockets in the recorded check.

VM preparation created an unlocked disposable guest account with a generated
password hash; the hash stayed in private seed/disk state. Repository tests
generated their own credentials. Guest system SSH services were masked, and
the resolver service was stopped for the test run. Preparation changed only
the guest, not host Remote Login, accounts, firewall or router settings.

Each harness received a private HOME/TMPDIR/ZDOTDIR/XDG tree, blank SSH-agent
variables and `/dev/null` for shell startup hooks. Compiler invocations also
used temporary homes and an allowlisted environment with public tool caches.
No personal SSH files, keys, agent or credential store were used.

Before shutdown, no processes remained for the guest test account and
`ss -ltnup` reported no TCP/UDP listeners. The guest powered off and QEMU
exited with status 0. All owned VM process IDs were subsequently absent;
no owned disk/seed file remained open. The three disposable disk overlays,
seed directories/ISOs and writable firmware copies were removed, including
the generated guest credential and host-key state. Public image/compiler
caches and nonsecret local receipts were retained.

## Setup failures and reproducibility limits

Initial cross-build attempts exposed Zig's different target spelling,
duplicate Zig/Rust startup objects, then missing Zig debug UBSan symbols.
Temporary compiler wrappers translated the target and used Rust's linker
with the matching Zig runtime archives. Assembly, cryptographic features,
sanitizer instrumentation, source and lockfiles were preserved.

The first runtime attempt stopped before tests because the ISO checksum
filename was mapped to lowercase. A new seed corrected that filename and
removed an unnecessary cloud-init configuration field. Failed setup
receipts were retained separately; they are not test passes or source failures.

The VM drivers and cross-tool wrappers are currently ignored local lab
prototypes, not a maintained repository command. The ordinary reproducible
entry point remains [`cargo xtask test-ssh`](ssh-lab.md) on a prepared dedicated
Unix runner. Packaging the offline VM procedure into a supported command is
a separate task. Linux native desktop compilation, bash/zsh/fish PTY coverage,
X11/Wayland GUI acceptance, updated installers and hardware checks remain open.
GitHub workflows remain disabled; this receipt does not close those roadmap gates.

## Executable identities

These hashes identify the exact static test binaries run in the guest;
they are local debug artifacts, not release assets.

| Harness | SHA-256 |
| --- | --- |
| `authentication` | `6237eff3adb00156d6ba6bb122b7f7f02a2ca1358c845bd6f53d11dd4ddbd02d` |
| `forwarding` | `4415b0dd3cf0a0decaaedd159fc204d8ba4da8fd7c4a0904db0bb71a0a3d3298` |
| `local_sshd` | `cb7f93ab4654abbbc0727a40a75aa6071c18af15cd8a3f422c08b3ebc2f42813` |
| `mobarust_ssh` | `457ad673800b7afd6834569ef5f1de19c20ad9fbe31d3ae42524a5562d673d70` |
| `remote_editor` | `1e8950ae38fb19583a3c5bc448c6ba2d5441d0928463cbdb5d2dca8497170ffe` |
| `sftp_bounds` | `9ff1787d5408910817d8d790a0c802deb20a77f4ee03e8db747ed926bc58a66b` |
| `sftp_metadata` | `99e376c367f7b376865ce7b85415ed61c60ab1e89d886bcb98e7288d9e721c5f` |

The successful seed ISO SHA-256 was
`566acb4d98e4b4d23abfb78a3d8f71281e0d90f60fed0e994e4f55b9abca783b`;
the retained local serial log SHA-256 was
`41b032aa4556a6ab2dd84c1797ee268f03a53cfd14b11a3f50a825e328d377f1`.
Raw logs and generated credential state are not published.
