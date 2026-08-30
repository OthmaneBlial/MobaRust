# ADR 0034: disable vulnerable RSA key support until a fixed SSH dependency exists

## Status

Implemented in the main workspace.

## Context

The local dependency audit on 2026-08-30 reported `RUSTSEC-2023-0071`
(Marvin timing attack) for `rsa 0.10.0-rc.18`. The crate was pulled into the
main application only by `russh`'s default RSA feature. The
[RustSec advisory](https://rustsec.org/advisories/RUSTSEC-2023-0071.html)
still has no patched release.

MobaRust's saved-session contract requires a private-key reference, but does
not require the legacy RSA algorithm specifically. Keeping the vulnerable
implementation enabled would make the security boundary weaker for every SSH
user, including users who only use passwords, agents, Ed25519, or ECDSA.

## Decision

The main workspace uses `russh` with default features disabled and enables only
`aws-lc-rs` and `flate2`. Ed25519 and ECDSA key paths remain available. RSA
private-key files are rejected before authentication with the actionable
`UnsupportedKeyAlgorithm` error; no key bytes are included in that error.

The isolated RDP helper has its own dependency lockfile and is not changed by
this decision. Its dependency audit remains a separate release check. That
audit currently reports the same Marvin advisory through
`picky -> ironrdp-connector -> ironrdp-client`, so the RDP candidate is not
staged into normal application bundles. Upstream's [CredSSP dependency cleanup
issue](https://github.com/Devolutions/IronRDP/issues/1433) remains open, so
there is no safe feature toggle in the selected connector that removes the
chain today. It remains available for isolated, repository-local development
checks only.

## Trade-off and migration

Existing RSA SSH profiles will not authenticate through the main workspace
until a maintained SSH stack with a fixed RSA implementation is selected. The
UI should present the unsupported-algorithm error and suggest an Ed25519 or
ECDSA key where the operator can safely rotate credentials. Re-enable RSA only
after a fresh advisory audit and an explicit review of the timing behavior.

No key files, SSH agents, Keychain entries, or remote hosts are needed to
verify this policy. The unit test constructs only a synthetic unsupported-key
error and asserts that raw material does not escape.

## Verification

```text
cargo check --workspace
cargo audit --no-fetch
```

The audit may still report transitive unmaintained GTK warnings from the
desktop stack; those are warnings, not the removed RSA vulnerability, and
remain a separate platform dependency review item.

The RDP helper must pass its own audit before it can be packaged or called a
production protocol implementation. The packaging task removes any stale,
repository-generated RDP helper from the staging directory before assembling a
bundle.
