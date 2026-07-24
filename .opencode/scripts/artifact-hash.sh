#!/bin/bash
# Compute a deterministic hash of an OpenSpec change's artifact bundle.
# Usage: artifact-hash.sh openspec/changes/<change-name>
# Output: sha256:<hash>

set -euo pipefail

CHANGE_DIR="$1"

if [ ! -d "$CHANGE_DIR" ]; then
  echo "ERROR: directory not found: $CHANGE_DIR" >&2
  exit 1
fi

# Collect and hash all artifact files (proposal, design, specs/**, tasks)
# Exclude .attestation.json, .openspec.yaml, and archive metadata
FILES=$(find "$CHANGE_DIR" \
  -type f \
  \( -name "proposal.md" -o -name "design.md" -o -name "tasks.md" -o -name "*.md" \) \
  -not -name ".attestation.json" \
  -not -name ".openspec.yaml" \
  -not -path "*/archive/*" \
  | sort)

if [ -z "$FILES" ]; then
  echo "ERROR: no artifact files found in $CHANGE_DIR" >&2
  exit 1
fi

# Concatenate file contents with paths for deterministic hashing.
# Normalize checkbox state (- [x] → - [ ]) so progress tracking
# does NOT invalidate the attestation — only content changes do.
HASH=$(echo "$FILES" | while read -r f; do
  echo "=== $f ==="
  sed 's/- \[[xX]\]/- [ ]/g' "$f"
  echo ""
done | sha256sum | awk '{print $1}')

echo "sha256:$HASH"
