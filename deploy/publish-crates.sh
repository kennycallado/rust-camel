#!/usr/bin/env bash
# Script to publish rust-camel crates to crates.io
# Usage: ./publish-crates.sh [--dry-run]

set -e

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

# Function to publish a crate
publish_crate() {
  local crate=$1
  local path=$2
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

  # Check if this specific version already exists
  local existing_version=$(cargo search "$crate" 2>&1 | grep "^$crate = " | sed -n 's/.*"\([^"]*\)".*/\1/p')
  
  if [ "$existing_version" = "$current_version" ]; then
    echo "⚠️  $crate@$current_version already exists on crates.io, skipping..."
    cd "$WORKSPACE_ROOT" >/dev/null
    return 0
  fi

  # Check if older version exists
  if [ -n "$existing_version" ]; then
    echo "📦 Updating $crate from $existing_version to $current_version..."
  else
    echo "📦 Publishing new crate $crate@$current_version..."
  fi

  cargo publish $DRY_RUN
  cd "$WORKSPACE_ROOT" >/dev/null
  if [ -z "$DRY_RUN" ]; then
    echo "⏳ Waiting 10s for crates.io to index..."
    sleep 10
  fi
}

# Core crates (in dependency order)
publish_crate "camel-container" "crates/components/camel-container"
publish_crate "camel-api" "crates/camel-api"
publish_crate "camel-util" "crates/camel-util"
publish_crate "camel-support" "crates/camel-support"
publish_crate "camel-endpoint" "crates/camel-endpoint"
publish_crate "camel-component" "crates/camel-component"
publish_crate "camel-processor" "crates/camel-processor"

# Language crates (needed by camel-core)
publish_crate "camel-language-api" "crates/languages/camel-language-api"
publish_crate "camel-language-simple" "crates/languages/camel-language-simple"
publish_crate "camel-language-rhai" "crates/languages/camel-language-rhai"

# Bean crates (needed by camel-core)
publish_crate "camel-bean-macros" "crates/camel-bean-macros"
publish_crate "camel-bean" "crates/camel-bean"

# Core engine
publish_crate "camel-core" "crates/camel-core"

# Component crates needed for tests (publish before camel-builder)
publish_crate "camel-component-timer" "crates/components/camel-timer"
publish_crate "camel-component-direct" "crates/components/camel-direct"
publish_crate "camel-component-mock" "crates/components/camel-mock"

# Remaining core crates
publish_crate "camel-health" "crates/camel-health"
publish_crate "camel-dsl" "crates/camel-dsl"
publish_crate "camel-config" "crates/camel-config" # Fixed path
publish_crate "camel-builder" "crates/camel-builder"

# OTel and Prometheus (depend on camel-builder + camel-component-timer)
publish_crate "camel-otel" "crates/components/camel-otel"
publish_crate "camel-prometheus" "crates/components/camel-prometheus"

# Remaining component crates
publish_crate "camel-component-log" "crates/components/camel-log"
publish_crate "camel-component-file" "crates/components/camel-file"
publish_crate "camel-component-http" "crates/components/camel-http"
publish_crate "camel-component-redis" "crates/components/camel-redis"
publish_crate "camel-component-kafka" "crates/components/camel-kafka"
publish_crate "camel-component-controlbus" "crates/components/camel-controlbus"

# Test utilities
publish_crate "camel-test" "crates/camel-test"

# Cli
publish_crate "camel-cli" "crates/camel-cli"

echo ""
echo "✅ All crates published successfully!"
