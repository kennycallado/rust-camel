# Proposal: rediscmds

## Why

The component exposes 98 `RedisCommand` variants, but the string family has no
`SET` options (`NX|XX|EX|PX|EXAT|PXAT|KEEPTTL|GET`) and no `GETDEL`/`GETEX`/
`SETRANGE`/`GETRANGE`; the 6.2/7.0 collection operations (`LMPOP`/`BLMPOP`,
`LMOVE`/`BLMOVE`, `LPOS`, `SMISMEMBER`, `SINTERCARD`, `ZMPOP`/`BZMPOP`,
`ZRANDMEMBER`, `ZDIFF`/`ZINTER`/`ZUNION`, `ZRANGEBYLEX`, `HRANDFIELD`,
`HINCRBYFLOAT`) are absent; and the generic key lifecycle lacks `UNLINK`,
`COPY`, `TOUCH`. The gap analysis (`.opencode/fleet/inbox/reference/
redis7-gap-analysis-2026-11-13.md`) classifies all of these as cheap enum
variants on the body-only payload contract landed in `rc-lv8jk` (main
`b9a276770`). Redis phase 1b part 1 closes this gap. Part 2 (bitmaps,
HyperLogLog, `WAIT`) is `rc-nukx2`, blocked by this change.

## What Changes

- `camel-component-redis`: 23 new `RedisCommand` variants with `from_str`/
  `as_str` parsing; `SET` option headers (`CamelRedis.SetCondition`,
  `CamelRedis.KeepTtl`, `CamelRedis.ReturnOldValue`, `CamelRedis.ExpireUnit`)
  with fail-closed conflict rules; `GETEX` mirrors the expiry surface;
  resolvers/executors in `commands/{string,list,set,zset,hash,key}.rs`;
  `BLMPOP`/`BZMPOP`/`BLMOVE` join the `BLPOP`/`BRPOP` deadline-bypass list in
  `is_blocking_endpoint`; `is_idempotent_command` classified per command —
  including one deliberate reclassification: `SET` becomes non-idempotent
  because its new option forms lose replay data (design.md D3).
- `docs/src/components/redis.md`: one table row per command plus a `SET`/
  `GETEX` option mapping table in the existing Expiration-table style.
- New `redis-commands` capability (delta spec): one requirement per family,
  a scenario per command argument mapping plus its named-error case.

Out of scope: bitmaps, HyperLogLog, `WAIT` (`rc-nukx2`), cluster, `EVAL`/
`FCALL`, SCAN family, `WEIGHTS`/`AGGREGATE` for `ZDIFF`/`ZINTER`/`ZUNION`
(deferred, justified in design.md), any new value source, any option that
bypasses the body-only payload contract.

## Acceptance criteria

- Every new command sends exactly one wire form, proven by a fake-server wire
  test (`value_fake_tests.rs` style), one per command.
- Every rejected argument fails closed before send with a named error that
  names the header, and has a test.
- `BLMPOP`/`BZMPOP`/`BLMOVE` complete a reply delayed past the redis-rs
  default 500 ms response timeout on a single connection (bypass test).
- `is_idempotent_command` matches the design table; pop/move family is never
  retried after an unconfirmed success.
- `docs/src/components/redis.md` carries one row per new command and the
  option mapping table.
- `openspec validate rediscmds --type change` and
  `cargo xtask archive rediscmds --check` pass.

## Risk budget

Additive with one deliberate behavior change: `SET` reclassifies to
non-idempotent (its new option forms lose replay data; design.md D3) — plain
`SET` routes relying on component-level retry must move to route-level
redelivery, documented in `docs/src/components/redis.md`. No existing variant
changes wire form, no payload contract change, no queue-adapter change
(`QueuePopCommand` stays `BLPOP`/`BRPOP`). Risk concentrates in
option-conflict validation, which must fail closed locally (ADR-0032:
exchange data is untrusted).

Bd: rc-tic86
