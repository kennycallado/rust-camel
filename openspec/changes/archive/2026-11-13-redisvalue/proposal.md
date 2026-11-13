# Proposal: redisvalue

## Why

The Redis producer reads every command value exclusively from the `CamelRedis.Value` JSON header (`commands/mod.rs`: `require_value` / `value_to_redis_arg`) and replaces every reply body with `Body::Json`. Two consequences: an exchange body cannot be written to Redis without being copied into a JSON header, and arbitrary bytes can be neither stored nor read (GET/MGET are typed `Option<String>`, so non-UTF-8 values fail conversion). XADD is the only exception — it already binds the body (`resolve_xadd_body`, `commands/stream.rs`). This is G0 of the Redis 7 plan (e_opus ruling 2026-11-13, bd `rc-01ka4`): a Redis-only application hits value binding before SCAN or cluster.

## What Changes

- **Value-source precedence**: the command's value header — `CamelRedis.Value`, or `CamelRedis.Member` for GEOADD — when present (every existing route keeps working), otherwise the exchange body. No other sources.
- **One codec table** (`commands/mod.rs::resolve_value_bytes`) used by every value-writing command: SET, SETEX, SETNX, APPEND, LPUSH, RPUSH, LPUSHX, RPUSHX, LINSERT, LSET, HSET, HSETNX, SADD, ZADD, GEOADD, PUBLISH. XADD keeps its one-`body`-field rule but goes through the same codec, so `Body::Bytes` works for it too. HMSET keeps the `CamelRedis.Values` map header and additionally accepts a JSON-object body as the field→value map when the header is absent.
- **Binary-safe plumbing**: values reach the wire as byte arguments (`&[u8]` redis args); no per-command String round-trips.
- **Reply rule**: a reply that carries a value becomes `Body::Text` when the bytes are valid UTF-8, `Body::Bytes` otherwise; nil stays `Body::Json(null)`; counts/flags/acks stay `Body::Json`. Applied by one shared helper for the covered getters GET and MGET (all other getters keep their current JSON replies).
- **ADR-0098** records the precedence, codec table, reply rule, and exclusions (values are not streamed, chunked, or compressed).
- **Docs**: component README and `docs/src/components/redis.md` state the body fallback and the reply rule.
- **Excluded**: `rc-tic86` (SET options, GETDEL/GETEX, 6.2/7.0 collections, bitmaps/HLL); MSET keeps the `CamelRedis.Values` map header only (not in the mission's command list); no new command variants.

## Acceptance criteria

- The six mission-named fake-RESP-server tests pass (exact names listed in tasks.md), covering body binding, header precedence, bytes round-trip, the reply rule, fail-before-send, and XADD bytes.
- `openspec validate redisvalue --type change --json` passes with the new `redis-values` capability delta.
- Gates: `cargo fmt --check --all`, `cargo clippy -p camel-component-redis --all-targets --all-features -- -D warnings`, `cargo test -p camel-component-redis --lib`, `cd docs && mdbook test`, `bash scripts/gates-local.sh`.

## Risk budget

- Behavior changes only where the header is absent (body fallback) and in GET reply bodies (GET now `Text`/`Bytes`/`Json(null)` instead of `Json(string|null)`); MGET stays `Json` for all-text replies and gains a lossless bytes envelope for binary elements, which previously errored. The GET change is observable even with headers present, so the merge commit carries `!` plus a migration note.
- No Docker, no live Redis: the scripted fake RESP server already covers wire frames; live integration stays with CI/camel-test.
