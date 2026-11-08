# Proposal: camel-test-len-zero

## Why

`cargo clippy -p camel-test --all-targets --features integration-tests -- -D warnings`
fails on `main` (`f21935b73`) with four `clippy::len_zero` errors in
`crates/camel-test/tests/sql_test.rs` (lines 1423, 1424, 1554, 1555 — bd rc-0e0n3's
line numbers were stale). Every mission running this clippy variant inherits the red
and must prove the redness is not its own.

## What Changes

Replace `.len() >= 1` with `!is_empty()` at the four flagged sites, taking clippy's
exact suggestion. Comparison-only change: surrounding logic and messages stay
byte-identical. No `crates/**/src/**` files touched; total diff 4+/4−.

## Capability not affected

Test-only mechanical lint fix; no spec deltas (`skip_specs: true`).

## Impact

- `crates/camel-test/tests/sql_test.rs`: 4 comparison sites
- Gates: the camel-test clippy variant returns to green
- Out of scope: gate registry changes (rc-e6kz2), camel-component-api rustdoc link
  (rc-5ifo3), any other crate or non-`len_zero` assertion shape
