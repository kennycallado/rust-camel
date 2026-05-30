#!/usr/bin/env bash
# Script to publish rust-camel crates to crates.io
# Usage: ./scripts/publish-crates.sh [--dry-run]
#
# Publish order is computed automatically via `cargo xtask publish-order`
# which performs a topological sort over workspace crate dependencies.

set -e

# Ensure cargo is available when script runs outside a login shell
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"

DRY_RUN=""
if [ "$1" == "--dry-run" ]; then
  DRY_RUN="--dry-run"
  echo "🔍 DRY RUN MODE - No actual publishing"
fi

echo "📦 Publishing rust-camel crates to crates.io"
echo "============================================="

# Get workspace root version
WORKSPACE_ROOT=$(pwd)
WORKSPACE_VERSION=$(grep '^version = ' Cargo.toml | cut -d'"' -f2)

# Wait until Cargo's registry index can resolve the freshly published version.
# crates.io's HTTP API can report success before `cargo publish` dependency
# verification sees the version in the registry index.
wait_for_crate_index() {
  local crate=$1
  local version=$2
  local attempts=20
  local delay=15

  echo "⏳ Waiting for $crate@$version to appear in Cargo registry index..."
  for attempt in $(seq 1 "$attempts"); do
    if cargo info "${crate}@${version}" >/dev/null 2>&1; then
      echo "✅ $crate@$version is visible in Cargo registry index"
      return 0
    fi

    if [ "$attempt" -lt "$attempts" ]; then
      echo "   attempt $attempt/$attempts: not visible yet; retrying in ${delay}s..."
      sleep "$delay"
    fi
  done

  echo "❌ Timed out waiting for $crate@$version in Cargo registry index"
  return 1
}

# Function to publish a crate
publish_crate() {
  local crate=$1
  local path=$2
  local extra_flags="${3:-}"
  echo ""
  echo "📦 Publishing $crate..."
  cd "$path"

  # Get current version (check if workspace version)
  local version_line=$(grep '^version' Cargo.toml | head -1)
  local current_version

  if echo "$version_line" | grep -q "workspace"; then
    current_version=$WORKSPACE_VERSION
  else
    current_version=$(echo "$version_line" | cut -d'"' -f2)
  fi

  # Check via crates.io API (authoritative, no cache issues)
  local http_status=$(curl -s -o /dev/null -w "%{http_code}" \
    "https://crates.io/api/v1/crates/${crate}/${current_version}")

  if [ "$http_status" = "200" ]; then
    echo "⚠️  $crate@$current_version already exists on crates.io, skipping..."
    cd "$WORKSPACE_ROOT" >/dev/null
    return 0
  fi

  echo "📦 Publishing $crate@$current_version..."

  # Skip verification in dry-run mode (dependencies don't exist on crates.io yet)
  if [ -n "$DRY_RUN" ]; then
    echo "⚠️  Dry-run: skipping cargo publish verification (would fail due to deps not on crates.io)"
    cd "$WORKSPACE_ROOT" >/dev/null
    return 0
  fi

  # Capture output; treat "already exists" as success (idempotent)
  local publish_output
  publish_output=$(cargo publish $DRY_RUN --allow-dirty $extra_flags 2>&1) || {
    if echo "$publish_output" | grep -q "already exists"; then
      echo "⚠️  $crate@$current_version already exists (race), skipping..."
      cd "$WORKSPACE_ROOT" >/dev/null
      return 0
    fi
    echo "$publish_output"
    cd "$WORKSPACE_ROOT" >/dev/null
    return 1
  }
  echo "$publish_output"

  cd "$WORKSPACE_ROOT" >/dev/null
  if [ -z "$DRY_RUN" ]; then
    wait_for_crate_index "$crate" "$current_version"
    sleep 10
  fi
}

# --- Auto-generated publish order from topological sort ---
# To inspect order: cargo xtask publish-order
# To regenerate shell lines: cargo xtask publish-order --shell
PUBLISH_ORDER=$(cargo xtask publish-order --shell) || {
  echo "❌ Failed to compute publish order:"
  echo "$PUBLISH_ORDER"
  exit 1
}
eval "$PUBLISH_ORDER"

echo ""
echo "✅ All crates published successfully!"
