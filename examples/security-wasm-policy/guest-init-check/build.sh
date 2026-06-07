#!/usr/bin/env bash
# Rebuild the init-check.wasm fixture.
# Must run from this directory.
# Requires: wasm32-wasip2 target available (provided by Nix flake).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
FIXTURES_DIR="$(cd "$SCRIPT_DIR/../fixtures" && pwd)"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$SCRIPT_DIR/target}"

export CARGO_TARGET_DIR

cargo build --target wasm32-wasip2 --release

cp "$CARGO_TARGET_DIR/wasm32-wasip2/release/deps/init_check_policy.wasm" \
   "$FIXTURES_DIR/init-check.wasm"

echo "Fixture updated: $FIXTURES_DIR/init-check.wasm"
