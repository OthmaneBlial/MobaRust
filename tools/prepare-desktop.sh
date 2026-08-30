#!/bin/sh
set -eu

SCRIPT_DIRECTORY=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPOSITORY_ROOT=$(CDPATH= cd -- "$SCRIPT_DIRECTORY/.." && pwd)

cd "$REPOSITORY_ROOT/apps/desktop"
pnpm build

cd "$REPOSITORY_ROOT"
cargo run --locked --manifest-path "$REPOSITORY_ROOT/xtask/Cargo.toml" -- stage-helpers
