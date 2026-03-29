#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
COVERAGE_FILE="$ROOT_DIR/coverage.toml"

if [ ! -f "$COVERAGE_FILE" ]; then
  echo "ERROR: coverage.toml not found at $COVERAGE_FILE"
  exit 1
fi

MINIMUM=$(grep 'minimum_line_coverage' "$COVERAGE_FILE" | head -1 | sed 's/.*= *//' | tr -d ' ')

if [ -z "$MINIMUM" ]; then
  echo "ERROR: Could not parse minimum_line_coverage from coverage.toml"
  exit 1
fi

echo "=== Coverage Baseline Check ==="
echo "Minimum required: ${MINIMUM}%"
echo ""

if [ "${1:-}" = "--full" ]; then
  echo "Running full workspace coverage (including integration tests)..."
  cargo llvm-cov --workspace --summary-only 2>&1 | tee /tmp/llvm-cov-output.txt
else
  echo "Running unit-only coverage (--lib)..."
  cargo llvm-cov --workspace --lib --summary-only 2>&1 | tee /tmp/llvm-cov-output.txt
fi

TOTAL_LINE=$(grep "^TOTAL" /tmp/llvm-cov-output.txt | tail -1)

if [ -z "$TOTAL_LINE" ]; then
  echo "ERROR: Could not find TOTAL line in coverage output"
  exit 1
fi

# Extract line coverage percentage (3rd percentage in TOTAL row: Regions, Functions, Lines)
ACTUAL=$(echo "$TOTAL_LINE" | awk '{n=0; for(i=1;i<=NF;i++) if($i ~ /^[0-9]+\.[0-9]+%$/) {n++; if(n==3) {gsub(/%/,"",$i); print $i}}}')

if [ -z "$ACTUAL" ]; then
  echo "ERROR: Could not extract line coverage percentage"
  exit 1
fi

echo ""
echo "=== Results ==="
echo "Actual coverage:   ${ACTUAL}%"
echo "Minimum required:  ${MINIMUM}%"
echo ""

# Compare using awk (avoids bc dependency)
PASS=$(awk "BEGIN {print ($ACTUAL >= $MINIMUM) ? 1 : 0}")

if [ "$PASS" = "1" ]; then
  echo "PASS: Coverage ${ACTUAL}% meets minimum ${MINIMUM}%"
  exit 0
else
  echo "FAIL: Coverage ${ACTUAL}% is below minimum ${MINIMUM}%"
  echo "Add tests or adjust baseline in coverage.toml (with discussion)."
  exit 1
fi
