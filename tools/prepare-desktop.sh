#!/bin/sh
set -eu

pnpm build
cargo run --manifest-path ../../xtask/Cargo.toml -- stage-helpers
