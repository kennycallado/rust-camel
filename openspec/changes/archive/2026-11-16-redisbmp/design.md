# Design: redisbmp

## Approach

Additive command surface in `camel-redis`, copying the part 1 / `redisfc`
patterns exactly: new `RedisCommand` variants in `config.rs`, two new family
modules (`commands/bitmap.rs`, `commands/hll.rs`) plus `WAIT` in
`commands/other.rs`, dispatch wiring in `executor.rs::dispatch_command`, and
metadata parity in `metadata.rs`. All arguments arrive as `CamelRedis.*`
headers (or, for `PFADD`, the body per ADR-0098) and are validated before
send with named `CamelRedis.*` errors. Replies map through the existing
`reply_to_body` / `redis_value_to_body` rules unchanged.

## Decisions (explicit, not inherited)

1. **BITFIELD overflow is explicit; there is no default.** The mode comes from
   the `CamelRedis.BitFieldOverflow` header (`WRAP|SAT|FAIL`), which sets the
   initial mode, and/or inline `OVERFLOW <mode>` tokens inside
   `CamelRedis.BitFieldOps` entries (per-op, translated verbatim). An
   effective mode SHALL be in force *before every mutating entry*
   (`SET`/`INCRBY`): a trailing `OVERFLOW` does not retroactively cover a
   preceding mutation, so `["INCRBY u8 0 1", "OVERFLOW SAT"]` is refused
   before send with a `CamelRedis.BitFieldOverflow` error (otherwise the
   mutation would execute under the server's implicit `WRAP`). `GET` entries
   need no mode. Rationale: the server default is `WRAP`, which silently
   wraps counters (corruption in counter/ID use); an implicit `SAT` would
   still hide intent. `BITFIELD_RO` accepts only `GET` entries and rejects
   the overflow header.
2. **WAIT reply semantics and scope.** `WAIT` returns the integer count of
   replicas that acknowledged the writes sent on the *invoking connection*
   up to the `WAIT` command; per the body-only contract that integer becomes
   the exchange body (JSON number). WAIT is a **connection-local
   acknowledgement probe, not a durability guarantee for earlier route
   writes**: producers share a multiplexed connection pool, and a reconnect
   resets the write offset, so a WAIT served on a different (or re-created)
   connection acknowledges nothing about earlier exchanges. Routes that need
   write durability must co-locate the write and the WAIT on the same
   connection or use application-level confirmation. The command is
   classified **idempotent**: no server state change; a replay re-probes the
   current connection's offset and may return a different count (count-style
   reply, `DEL` precedent). **Bounding:** `CamelRedis.WaitTimeout`
   (milliseconds, required, `> 0` — Redis `WAIT ... 0` blocks forever and is
   refused before send) bounds the server-side wait per attempt, and a
   WAIT-only aggregate deadline of `CamelRedis.WaitTimeout + config.timeout`
   bounds the whole exchange (connection checkout, every retry attempt,
   backoff, replies) — `execute_with_retry` is not otherwise bounded by
   `config.timeout`, so the deadline is enforced explicitly around the WAIT
   dispatch. On deadline expiry the exchange fails with a named timeout
   error naming `CamelRedis.WaitTimeout`; a server-side `WAIT` that outlives
   the deadline is harmless (read-only probe) and the connection is released
   normally. It can never hang forever.
3. **Retry classification** (added to `is_idempotent_command`):
   idempotent — `GETBIT`, `BITCOUNT`, `BITPOS`, `BITFIELD_RO`, `PFCOUNT`
   (its single-key form may refresh the HLL's cached cardinality — a
   converging internal write whose result is the same observable count; the
   multi-key form only reads its sources), `WAIT` (see decision 2 for the
   connection-local scope under which replay is safe); not idempotent —
   `SETBIT`, `BITOP`, `BITFIELD` (worst-case form governs: it may
   `SET`/`INCRBY`), `PFADD`, `PFMERGE`. Never defaulted: each is listed
   explicitly.
4. **Argument headers** (validated before send, named errors):
   `SETBIT`/`GETBIT`: `CamelRedis.Key`, `CamelRedis.BitOffset` (u64,
   `0..=4294967295` — the 512 MB bitstring cap); `SETBIT` also
   `CamelRedis.BitValue` (0|1). `BITCOUNT`/`BITPOS`: key plus optional range
   headers `CamelRedis.Start`/`CamelRedis.End` (i64, negative indexes from
   the end, Redis-compatible bounds) and `CamelRedis.BitRangeUnit`
   (`BYTE|BIT`, default `BYTE` — the Redis default is a range-unit default
   with no value semantics, so it is documented, not accidental). Grammar
   per Redis 7: `BITCOUNT` requires `Start` and `End` together; `BITPOS` may
   give `Start` alone but a unit requires both endpoints; violations fail
   before send with named errors. `BITPOS` also `CamelRedis.BitValue`.
   `BITOP`: `CamelRedis.BitOperation` (`AND|OR|XOR|NOT`),
   `CamelRedis.Destination`, `CamelRedis.Keys` (SINTERSTORE convention);
   `NOT` takes exactly one source key (named error otherwise), `AND|OR|XOR`
   take ≥1. `BITFIELD`/`_RO`: `CamelRedis.Key`, `CamelRedis.BitFieldOps`
   (string list; entries `GET|SET|INCRBY <[iu]width> <offset|#n> [value]`
   or `OVERFLOW <WRAP|SAT|FAIL>`); widths: signed 1..=64, unsigned 1..=63
   (`u64` invalid — "u64 is not supported but i64 is"); absolute offsets are
   non-negative and must fit i64 per Redis parsing; `#<n>` offsets are
   evaluated independently as `n × <this entry's width>` bits from the start
   (the Redis rule: the multiplier is the current entry's encoding width, not
   a running position — two `GET u8 #1` entries address the same bits), with
   checked multiplication that fails before send with a named error on
   overflow. Unknown ops and malformed types/offsets/values fail before
   send. `PFADD`: key header, element in
   the body. `PFCOUNT`: `CamelRedis.Keys` (≥1; single-key is a one-element
   list). `PFMERGE`: `CamelRedis.Destination` + `CamelRedis.Keys` (≥1).
   `WAIT`: `CamelRedis.NumReplicas` (u64), `CamelRedis.WaitTimeout` (u64 ms,
   distinct from `CamelRedis.Timeout`, which is TTL). Both are wire-checked:
   Redis parses `WAIT`'s arguments as signed 64-bit, so values above
   `i64::MAX` (e.g. `u64::MAX`) fail before send with named errors;
   `CamelRedis.WaitTimeout` must also be `> 0`. The aggregate deadline
   (`CamelRedis.WaitTimeout + config.timeout`) is constructed with checked
   arithmetic; an unrepresentable deadline fails before send with a named
   error instead of panicking.
5. **Reply mapping** (existing rules): `SETBIT`/`GETBIT`/`BITCOUNT`/`BITPOS`/
   `BITOP`/`PFADD`/`PFCOUNT`/`WAIT` integers → JSON number; `BITFIELD`/`_RO`
   array of integers/nils → JSON array of numbers/nulls
   (`redis_value_to_body`); `PFMERGE` status → text.

## Affected crates

- `camel-redis`: `commands/bitmap.rs`, `commands/hll.rs` (new),
  `commands/other.rs` (WAIT), `commands/mod.rs` (shared helpers/tests),
  `config.rs` (variants + classifier), `metadata.rs`, `executor.rs` (wiring).
- `camel-test`: live tests in `redis_test` / `redis_repositories_test`.
- `docs`: one row per command in `docs/src/components/redis.md`.

## Architecture boundaries

Components layer only (ADR-0001): no runtime/DSL changes. Exchange data is
untrusted input (ADR-0032): every header is validated fail-closed before
send. Values stay on the body-only contract (ADR-0098): `PFADD`'s element is
a body value; no payload headers are added. Retry behavior reuses the
`redisfc` outcome classification (ADR-0013) unchanged.

## Alternatives considered

- `BITFIELD` op list as one options blob string: rejected; a string list of
  tokenized entries keeps per-op `OVERFLOW` expressible and each entry
  validatable with a named error.
- Encoding `BITOP` as four URI commands: rejected; one wire command, one
  variant, operation as a validated header.
- Defaulting overflow to `SAT`: rejected; "safe by accident" still hides
  intent and diverges from Redis semantics silently.
- `PFCOUNT` via `CamelRedis.Key`: rejected in favor of `CamelRedis.Keys`
  (≥1) so single and multi-key forms share one validation path.
