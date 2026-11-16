# Proposal: redisbmp

## Why

Phase 1b part 2 of the Redis 7 plan (`rc-pahez`). Part 1 (`rc-tic86`) landed
string options, generics and 6.2/7.0 collections on the body-only value
contract (ADR-0098); `redisfc` landed eval/fcall/multi plus the three-bucket
retry classifier (ADR-0013). The everyday bitmap and HyperLogLog families and
the `WAIT` durability probe named in the gap analysis are still missing, so
routes cannot do bit packing, cardinality estimation or Sentinel failover
durability checks. bd: `rc-nukx2`.

## What Changes

Additive command surface in `crates/components/camel-redis` (and its live
tests in `camel-test`):

- **Bitmaps**: `SETBIT`, `GETBIT`, `BITCOUNT` (BYTE|BIT ranges), `BITPOS`,
  `BITOP` (AND|OR|XOR|NOT over N sources into a destination), `BITFIELD` and
  `BITFIELD_RO` (GET/SET/INCRBY with signed/unsigned types and widths).
- **HyperLogLog**: `PFADD`, `PFCOUNT` (single and multi-key), `PFMERGE`.
- **Durability**: `WAIT <numreplicas> <timeout_ms>`.

Excluded: the `SCAN` family (`rc-r3m6x`, separate change to avoid touching
`commands/mod.rs`, `config.rs` and the docs table twice); any change to the
`redisfc` classifier bucket contract (it is reused as-is); cluster work.

## Acceptance criteria

- All 11 commands dispatch end-to-end with `CamelRedis.*` argument headers
  validated before send (named errors, ADR-0032 fail-closed).
- BITFIELD overflow is always explicit (WRAP|SAT|FAIL), never inherited from
  the server default; the decision is documented.
- `WAIT` reply semantics, body effect, retry class and bounding are decided
  and documented; it can never hang forever.
- Per-command retry classification is explicit in `is_idempotent_command`.
- One docs row per command in `docs/src/components/redis.md`.
- Spec delta on `redis-commands` with scenarios per family; unit tests for
  validation and classification; live tests (a representative command per
  family) green against `redis:7-alpine`.

## Risk budget

Additive only: no behavior change to existing commands or the classifier
contract. Acceptable risk: new header names and a small BITFIELD op-list
grammar. Out of bounds: any break of the body-only contract, any default
overflow inheritance, unbounded `WAIT` blocking.
