# Repository-local russh-sftp patch

Baseline: [russh-sftp 2.4.0](https://crates.io/crates/russh-sftp/2.4.0),
crates.io archive SHA-256
`9de67aace74530a29086db0671fa200c470a58eb380081f28ad512ffb0c5356b`.
Packaged source commit: `e145c1f7ece99f41f558949ef59731f2cd1a9dfe`.
Apache-2.0; the original LICENSE, README and all 48 production source files
are retained, with line endings normalized to LF. Examples, benchmarks,
development dependencies and archive
metadata are omitted. Default library tests/doctests are disabled and this
dependency is excluded from workspace test targets. The local SSH/full-check
commands explicitly run its private library tests, alongside MobaRust's
public-API regressions. No dependency version, feature or production source file changes
except the five files below and line-ending normalization.

- `src/client/mod.rs`: route the existing configuration to the packet reader,
  enforce the default 256 KiB payload limit before allocation, stop on invalid
  frames, and cancel the other worker even when its write is backpressured.
  The public low-level `run` entry point uses the same default limit.
- `src/client/rawsession.rs`: settle pending requests when the reader exits,
  reject requests racing with shutdown, and reject DATA longer than the
  requested read. This applies to both high-level file sessions and the raw
  directory-listing session. A private request future owns each reply slot;
  dropping it removes the slot immediately, including queued write acknowledgments
  and fire-and-forget handle closes. After VERSION, unmatched ordinary replies
  are discarded without retaining tombstones or closing unrelated requests.
  Pre-VERSION responses and unsolicited/duplicate VERSION remain errors.
  Request timeouts and server limits remain distinct; public APIs are unchanged.
- `src/de.rs`: reject a sequence count greater than the remaining packet bytes
  before exposing the count to a reserving visitor. Strings and byte buffers
  already check their length against the available bytes.
- `src/client/fs/file.rs`: retain a cancelled read's unconsumed reply in one
  bounded standard-library cursor, advance the file position only by delivered
  bytes, and discard old read state after seek, accepted write or handle closure.
  Empty reads/writes do not issue requests; closed handles refuse reads. Clamp
  requested DATA to both the negotiated read limit and the packet payload budget;
  refuse a configuration with no room for file data instead of reporting EOF.
  Cap WRITE chunks by both data and packet limits, accounting for the encoded
  packet prefix in the raw server limit; refuse zero-data-budget and closed-handle
  writes without advancing position or issuing a request.
  Use checked unsigned seek arithmetic, retire failed end-seek futures before
  reporting their error, and bound READ/WRITE by representable offset space.
  Out-of-range positions fail before another data request; empty I/O stays empty.
- `src/client/session.rs`: clamp a server's 64-bit packet limit before narrowing
  it to the client's 32-bit configured limit, avoiding truncation on conversion.

Regression commands:

```text
cargo test --locked -p russh-sftp --lib
cargo test --locked -p mobarust-ssh --test sftp_bounds
cargo xtask test-ssh
cargo xtask check
```

The in-memory checks use small packets, header-only oversized declarations,
EOF, malformed frames, impossible sequence counts, a blocked writer and
normal/excess DATA lengths. They never allocate a multi-gigabyte packet.
File checks cover cancellation followed by smaller buffers, exact byte order
and offsets, empty I/O, pending/buffered seek and write, acknowledged handle
closure, negotiated read/write limit combinations and deliberately tiny budgets.
Upload checks reconstruct a multi-chunk byte sequence, assert exact offsets and
encoded packet sizes, and refuse writes after acknowledged closure.
Offset checks use tiny packets at the unsigned boundary, including signed-minimum
seek deltas, rejected or missing FSTAT sizes, failed-seek recovery, partial I/O
at the last representable byte and refusal without an extra data request.
Private library checks cover immediate async/nowait slot release, timeout,
late/unmatched replies while another request is pending, initialization refusal,
and preservation of a newer live receiver when an ID is reused. Public in-memory
checks also exercise late-reply recovery and initialization failure through both
stream workers, including EOF without dropping the session value.
The generated loopback SSH fixture checks both actual SFTP channel paths,
channel closure before disconnect and continued use of the authenticated SSH
transport. The OpenSSH lab exercises normal transfers and directory listings.
These checks are not an independent security audit or native GUI acceptance.

Remove this copy when an upstream release addresses these same boundaries and
the regressions pass unchanged. Advisory tools check the baseline version;
they do not review this local patch. Both [v0.1.23 Mac previews](../../docs/release/v0.1.23.md)
include the first three response-bound changes. Both verified
[v0.1.24 Mac previews](../../docs/release/v0.1.24.md) also include the later file-reader,
64-bit-limit, request-lifetime and file-write corrections. Native workflow
acceptance remains pending.
The subsequent seek-recovery and offset-exhaustion fixes are source after
v0.1.24, pending new installers and native acceptance.
Mac downloads before v0.1.23 and v0.1.12 Windows/Linux installers contain
neither of the first two cohorts.
