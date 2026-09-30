# Proposal: redissrc — single-source CamelRedis.Value in camel-redis src (mission 332)

## Why

Sweep follow-up rc-in4hl (child of epic rc-62mdr, mission 313 redissweep):
`crates/components/camel-redis/src/` has no public header-name const; the
`"CamelRedis.Value"` literal appears 59 times across the 6 command files
(hash.rs 6, list.rs 15, mod.rs 1, set.rs 13, string.rs 11, zset.rs 13),
plus one more textual occurrence embedded in the `require_value` error
message (`"Missing required header: CamelRedis.Value"`). Mechanical dedup
class, redis zone conventions, zero behavior change.

## What Changes

1. Add `pub const HEADER_VALUE: &str = "CamelRedis.Value"` to
   `crates/components/camel-redis/src/lib.rs` (crate root, matching the
   camel-container `HEADER_*` house convention). Public so camel-test can
   later adopt it, superseding `tests/support/mod.rs REDIS_VALUE_HEADER`
   (landed rc-158g3, commit cf666945).
2. Swap all 59 quoted literals in the 6 command files to `HEADER_VALUE`
   via `use crate::HEADER_VALUE;`.
3. Rewrite the embedded `require_value` error message with
   `format!("Missing required header: {HEADER_VALUE}")` so the literal
   string survives only at the const definition.

Out of scope: the 10 `"CamelRedis.Values"` (plural) literals — a distinct
header for hash field-value maps; recorded as a sweep spinout on epic
rc-62mdr if not already tracked.

## Impact

- crates: camel-component-redis only
- bd: rc-in4hl
- risk: none (same string, same call shapes, additive public const)
