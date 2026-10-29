# Proposal: redis-streams-pending-recovery

## Why

Mission 431 landed XREADGROUP slice 1 (bd rc-euud8). A `>` read never replays, so
stream entries left pending by a pipeline failure, cancellation, or an
unknown-outcome ack transient stay in the Pending Entries List until an operator
intervenes. Slice 2 (bd rc-x8i4k) closes that gap: the consumer inspects the PEL
and recovers stale entries with XAUTOCLAIM, and the delivery count stops being a
pinned `1` so operators can see redelivery pressure.

## What Changes

- An XPENDING inspection path: one bounded summary per recovery pass, surfaced
  through the existing log style. Every component-issued request carries an
  explicit bound (positional count on XPENDING ranges, `COUNT` on XAUTOCLAIM;
  the XPENDING summary is the protocol's aggregate form and has no count
  parameter).
- An XAUTOCLAIM recovery loop with a configurable minimum idle time and a bounded
  batch size. Claimed entries go through the SAME `send_and_wait` delivery
  pipeline as `>` reads, keep the deadline-free connection rules, stop promptly
  on shutdown/cancel, and never spin on an empty claim cursor. XACK stays
  after-success only; a failed delivery stays pending for the next pass.
- `CamelRedis.StreamDeliveryCount` populated from the recovery data instead of
  the pinned `"1"`; first `>` deliveries stay `"1"`. Header meaning documented.
- New options `streamRecoverEnabled` (default `false`), `streamRecoverMinIdleMs`
  (default `60000`), `streamRecoverBatch` (default `10`), with fail-closed range
  validation. Metadata and component docs updated.
- Delta on `openspec/specs/redis-streams-consumer`.

Affected crates: `camel-component-redis` only.

## Acceptance criteria

- `cargo test -p camel-component-redis` green, including new fake-server
  command/response tests for XPENDING and XAUTOCLAIM, delivery-count header,
  ack-after-success / no-ack-on-failure, bounds, shutdown during recovery, and
  empty-claim no-spin.
- `cargo fmt --check --all`; `cargo clippy -p camel-component-redis
  --all-targets -- -D warnings`; xtask lints (`lint-unwrap`,
  `lint-test-sleep`, `lint-unbounded-wait`, `lint-glob-imports`,
  `lint-unsafe-safety`, `lint-missing-docs-ratchet`, `lint-no-deprecated`);
  `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-component-redis --no-deps`.
- `openspec validate --all --strict`; `cd docs && mdbook test`.
- No merge/push/tag/bd-close (Fleet park).

## Risk budget

Acceptable: recovery disabled by default; every request explicitly bounded
(the aggregate XPENDING summary excepted — the protocol's summary form has
no count parameter).
Out of bounds (non-goals): multi-consumer rebalancing beyond XAUTOCLAIM,
trimming (XTRIM/MINID), batching beyond COUNT, blocking-pop behavior changes,
other components.
