#!/usr/bin/env bash
# trace-absent: feature-gate PROOF for the http-server fixture
# (e_opus ruling D1 guardrail, 2026-09-16; bd rc-h42s6).
#
# FAILS if the given built fixture binary contains the string
# `BENCH_HTTP_REQUEST` — the smoke-trace path (log received +
# process id=<n>) must be absent from every M1–M4 binary. The
# harness M1–M4 builds NEVER enable the `bench-trace` feature, so
# the default-features binary MUST pass this check. Building with
# `--features bench-trace` MUST fail this check (that proves the
# check detects the boundary).
#
# Self-contained: inspects bytes of an already-built binary. No
# bench run, no fixture launch.
#
# Usage: trace-absent.sh [path-to-binary]
#   Default path: the canonical target/release binary the harness
#   resolves from the repository root.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# checks -> harness -> benchmarks -> <repo/worktree root>
ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
BIN="${1:-$ROOT/target/release/rust-camel-lib-fixture}"

if [[ ! -f "$BIN" ]]; then
    echo "FAIL: binary not found at '$BIN' (build it first: env -u CARGO_TARGET_DIR cargo build --release -p rust-camel-lib-fixture)"
    exit 1
fi

# grep -a: treat the binary as text (strings-equivalent byte scan).
# grep exits 1 on zero matches (prints "0"); any other failure
# (unreadable file) leaves count empty — treat empty as UNKNOWN and
# fail closed rather than silently passing.
count=$(grep -ac "BENCH_HTTP_REQUEST" "$BIN" 2>/dev/null || true)
if [[ -z "$count" ]]; then
    echo "FAIL: cannot scan '$BIN' (unreadable or grep failed) — failing closed" >&2
    exit 1
fi
if [[ "$count" -gt 0 ]]; then
    echo "FAIL: $BIN contains the smoke-trace string 'BENCH_HTTP_REQUEST' ($count byte-region match(es)) — trace path leaked into a non-trace build (e_opus ruling D1 guardrail, bd rc-h42s6)"
    exit 1
fi

echo "PASS: $BIN contains no 'BENCH_HTTP_REQUEST' trace path (e_opus ruling D1 guardrail: minimal-bare fixture, bench-trace feature never enabled by harness M1-M4 builds; bd rc-h42s6)"
exit 0
