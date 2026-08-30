# Fuzzing and property testing

MobaRust keeps parser and wire-boundary fuzzing separate from the normal
workspace. The fuzz targets consume only bytes supplied by libFuzzer; they do
not open sockets, read configuration files, inspect `~/.ssh`, use an SSH agent,
access a keychain, launch a helper, or touch a clipboard.

## Property tests

The `mobarust-core` test suite uses `proptest` for secret-free session JSON
round-trips and the bounded `ServerAliveInterval` invariant:

```text
cargo test -p mobarust-core --test property_invariants --locked
```

Generated cases use synthetic ASCII fixture values and never contain real
credentials. A failing case may be written by Proptest under the crate's
`proptest-regressions/` directory; review such a case before committing it.

The standalone fuzz package has a compile-only repository check:

```text
cargo xtask check-fuzz
```

## Fuzz targets

Install `cargo-fuzz` separately if it is not already available. The sanitizer
flags used by `cargo-fuzz` require a Nightly toolchain on the current Rust
setup, while the application itself remains pinned to Stable. Run the fuzzers
with an explicit `+nightly` override from the `fuzz/` directory:

```text
cd fuzz
cargo +nightly fuzz run session-json -- -runs=1000
cargo +nightly fuzz run helper-frame -- -runs=1000
```

The default corpus and artifacts are disposable and must not contain exported
profiles, private keys, passwords, clipboard text, or files from the host
system. Keep fuzz output under `fuzz/` and do not copy it into application
logs. This repository records the targets and commands, not synthetic claims
about fuzz coverage or a completed production security audit.
