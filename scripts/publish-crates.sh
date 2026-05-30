#!/usr/bin/env bash
# Publish all workspace crates to crates.io in topological order.
# Usage:
#   ./scripts/publish-crates.sh           # publish for real
#   ./scripts/publish-crates.sh --dry-run # dry run
#
# Publish order is computed automatically via `cargo xtask publish-order`
# which performs a topological sort over workspace crate dependencies.
set -e
if [ "$1" == "--dry-run" ]; then
  exec cargo xtask publish --dry-run
else
  exec cargo xtask publish
fi
