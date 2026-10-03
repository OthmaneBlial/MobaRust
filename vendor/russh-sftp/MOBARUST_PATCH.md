# Repository-local russh-sftp patch

Baseline: [russh-sftp 2.4.0](https://crates.io/crates/russh-sftp/2.4.0),
crates.io archive SHA-256
`9de67aace74530a29086db0671fa200c470a58eb380081f28ad512ffb0c5356b`.
Packaged source commit: `e145c1f7ece99f41f558949ef59731f2cd1a9dfe`.
Apache-2.0; the original LICENSE, README and all 48 production source files
are retained, with line endings normalized to LF. Examples, benchmarks,
development dependencies and archive
metadata are omitted. Library tests/doctests are disabled and this dependency
is excluded from workspace test targets; MobaRust's regressions exercise its
public APIs. No dependency version, feature or production source file changes
except the five files below and line-ending normalization.

- `src/client/mod.rs`: route the existing configuration to the packet reader,
  enforce the default 256 KiB payload limit before allocation, stop on invalid
  frames, and cancel the other worker even when its write is backpressured.
  The public low-level `run` entry point uses the same default limit.
- `src/client/rawsession.rs`: settle pending requests when the reader exits,
  reject requests racing with shutdown, and reject DATA longer than the
  requested read. This applies to both high-level file sessions and the raw
  directory-listing session. Request timeouts and server limits remain distinct.
- `src/de.rs`: reject a sequence count greater than the remaining packet bytes
  before exposing the count to a reserving visitor. Strings and byte buffers
  already check their length against the available bytes.
- `src/client/fs/file.rs`: retain a cancelled read's unconsumed reply in one
  bounded standard-library cursor, advance the file position only by delivered
  bytes, and discard old read state after seek, accepted write or handle closure.
  Empty reads/writes do not issue requests; closed handles refuse reads. Clamp
  requested DATA to both the negotiated read limit and the packet payload budget;
  refuse a configuration with no room for file data instead of reporting EOF.
- `src/client/session.rs`: clamp a server's 64-bit packet limit before narrowing
  it to the client's 32-bit configured limit, avoiding truncation on conversion.

Regression commands:

```text
cargo test --locked -p mobarust-ssh --test sftp_bounds
cargo xtask test-ssh
cargo xtask check
```

The in-memory checks use small packets, header-only oversized declarations,
EOF, malformed frames, impossible sequence counts, a blocked writer and
normal/excess DATA lengths. They never allocate a multi-gigabyte packet.
File checks cover cancellation followed by smaller buffers, exact byte order
and offsets, empty I/O, pending/buffered seek and write, acknowledged handle
closure, four negotiated limit combinations and a deliberately tiny budget.
The generated loopback SSH fixture checks both actual SFTP channel paths,
channel closure before disconnect and continued use of the authenticated SSH
transport. The OpenSSH lab exercises normal transfers and directory listings.
These checks are not an independent security audit or native GUI acceptance.

Remove this copy when an upstream release addresses these same boundaries and
the regressions pass unchanged. Advisory tools check the baseline version;
they do not review this local patch. Both [v0.1.23 Mac previews](../../docs/release/v0.1.23.md)
include the first three response-bound changes. The later file-reader and
64-bit-limit corrections are source after that release, pending new installers.
Older Mac downloads and v0.1.12 Windows/Linux installers contain neither cohort.
