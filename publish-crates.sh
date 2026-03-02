#!/bin/bash
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

# Function to publish a crate
publish_crate() {
    local crate=$1
    local path=$2
    echo ""
    echo "📦 Publishing $crate..."
    cd "$path"
    cargo publish $DRY_RUN
    cd - > /dev/null
    if [ -z "$DRY_RUN" ]; then
        echo "⏳ Waiting 10s for crates.io to index..."
        sleep 10
    fi
}

# Core crates (in dependency order)
publish_crate "camel-api" "crates/camel-api"
publish_crate "camel-util" "crates/camel-util"
publish_crate "camel-support" "crates/camel-support"
publish_crate "camel-endpoint" "crates/camel-endpoint"
publish_crate "camel-component" "crates/camel-component"
publish_crate "camel-processor" "crates/camel-processor"
publish_crate "camel-health" "crates/camel-health"
publish_crate "camel-dsl" "crates/camel-dsl"
publish_crate "camel-builder" "crates/camel-builder"
publish_crate "camel-core" "crates/camel-core"

# Component crates
publish_crate "camel-timer" "crates/components/camel-timer"
publish_crate "camel-log" "crates/components/camel-log"
publish_crate "camel-direct" "crates/components/camel-direct"
publish_crate "camel-mock" "crates/components/camel-mock"
publish_crate "camel-file" "crates/components/camel-file"
publish_crate "camel-http" "crates/components/camel-http"
publish_crate "camel-redis" "crates/components/camel-redis"
publish_crate "camel-controlbus" "crates/components/camel-controlbus"

echo ""
echo "✅ All crates published successfully!"
