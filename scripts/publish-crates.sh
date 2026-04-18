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
		echo "⏳ Waiting 30s for crates.io to index..."
		sleep 30
	fi
}

# Core crates (in dependency order)
publish_crate "camel-api" "crates/camel-api"
publish_crate "camel-util" "crates/camel-util"
publish_crate "camel-support" "crates/camel-support"
publish_crate "camel-endpoint-macros" "crates/camel-endpoint-macros"
publish_crate "camel-endpoint" "crates/camel-endpoint"

# Language crates (needed by camel-core; camel-language-js also needs camel-processor)
publish_crate "camel-language-api" "crates/languages/camel-language-api"
publish_crate "camel-language-simple" "crates/languages/camel-language-simple"
publish_crate "camel-language-rhai" "crates/languages/camel-language-rhai"



publish_crate "camel-component-api" "crates/components/camel-component-api"
publish_crate "camel-processor" "crates/camel-processor"

publish_crate "camel-language-js" "crates/languages/camel-language-js"
publish_crate "camel-language-jsonpath" "crates/languages/camel-language-jsonpath"
publish_crate "camel-language-xpath" "crates/languages/camel-language-xpath"

# Container component (depends on camel-api, camel-component-api, camel-endpoint)
publish_crate "camel-component-container" "crates/components/camel-container"

# Bean crates (needed by camel-core)
publish_crate "camel-bean-macros" "crates/camel-bean-macros"
publish_crate "camel-bean" "crates/camel-bean"

# Component crates required by camel-core (must come before it)
publish_crate "camel-component-timer" "crates/components/camel-timer"
publish_crate "camel-component-log" "crates/components/camel-log"
publish_crate "camel-component-mock" "crates/components/camel-mock"

# Core engine
publish_crate "camel-core" "crates/camel-core"

# Additional component crates (needed for tests / camel-builder)
publish_crate "camel-component-direct" "crates/components/camel-direct"

# Remaining core crates
publish_crate "camel-health" "crates/camel-health"
publish_crate "camel-platform-kubernetes" "crates/platforms/camel-platform-kubernetes"
publish_crate "camel-dsl" "crates/camel-dsl"
publish_crate "camel-builder" "crates/camel-builder"

# OTel (must come before camel-config which depends on it)
publish_crate "camel-otel" "crates/services/camel-otel"

# Prometheus depends on camel-builder + camel-component-timer
publish_crate "camel-prometheus" "crates/services/camel-prometheus"

# Component crates (needed by camel-config which has optional deps on most of them)
publish_crate "camel-component-file" "crates/components/camel-file"
publish_crate "camel-component-http" "crates/components/camel-http"
publish_crate "camel-component-redis" "crates/components/camel-redis"
publish_crate "camel-component-kafka" "crates/components/camel-kafka"
publish_crate "camel-component-sql" "crates/components/camel-sql"
publish_crate "camel-component-controlbus" "crates/components/camel-controlbus"
publish_crate "camel-component-validator" "crates/components/camel-validator"
publish_crate "camel-component-ws" "crates/components/camel-ws"
publish_crate "camel-bridge" "crates/services/camel-bridge"
publish_crate "camel-component-jms" "crates/components/camel-jms"

# camel-config depends on camel-otel, camel-prometheus and many component crates
publish_crate "camel-config" "crates/camel-config"

# Test utilities
publish_crate "camel-test" "crates/camel-test"

# Cli
publish_crate "camel-cli" "crates/camel-cli"

echo ""
echo "✅ All crates published successfully!"
