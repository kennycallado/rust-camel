#!/usr/bin/env bash
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

if ! command -v opencode >/dev/null 2>&1; then
  nix profile install .#opencode
fi

nix develop --command true
