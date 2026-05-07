#!/usr/bin/env bash
# Verify camel-all.wit is structurally consistent with the individual WIT files.
# Strategy: reconstruct what camel-all.wit should contain by extracting
# interfaces/worlds from split files and verifying they appear verbatim in the merged file.
set -e

CANONICAL=crates/camel-wit/wit/camel-all.wit
PLUGIN=crates/camel-wit/wit/camel-plugin.wit
BEAN=crates/camel-wit/wit/camel-bean.wit

fail() { echo "FAIL: $1"; exit 1; }

# Check canonical exists
[ -f "$CANONICAL" ] || fail "$CANONICAL not found"
[ -f "$PLUGIN" ]    || fail "$PLUGIN not found"
[ -f "$BEAN" ]      || fail "$BEAN not found"

# Exactly one package declaration in canonical
pkg_count=$(grep -c "^package " "$CANONICAL" || true)
[ "$pkg_count" -eq 1 ] || fail "camel-all.wit has $pkg_count package declarations (expected 1)"

# Extract normalized content (strip comments, blank lines) for comparison
normalize() {
  sed 's|//.*||g' "$1" | grep -v "^[[:space:]]*$" | sed 's/^[[:space:]]*//' | sort
}

# All non-package, non-import lines from split files must appear in canonical
# Compare sorted normalized token sets
plugin_tokens=$(normalize "$PLUGIN" | grep -v "^package ")
bean_tokens=$(normalize "$BEAN" | grep -v "^package ")
canonical_tokens=$(normalize "$CANONICAL")

check_tokens() {
  local src="$1"
  local label="$2"
  while IFS= read -r line; do
    [ -z "$line" ] && continue
    if ! echo "$canonical_tokens" | grep -qF "$line"; then
      fail "Line from $label not found in camel-all.wit: '$line'"
    fi
  done <<< "$src"
}

check_tokens "$plugin_tokens" "camel-plugin.wit"
check_tokens "$bean_tokens" "camel-bean.wit"

echo "OK: camel-all.wit is in sync with individual WIT files"
