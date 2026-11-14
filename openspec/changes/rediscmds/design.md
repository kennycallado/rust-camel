# Design: rediscmds

## Approach

Follow the established per-command pattern: a `RedisCommand` variant with
`from_str`/`as_str` in `config.rs`, a dispatch arm in `executor.rs` routing to
the family module, a resolver per argument (named errors that name the header
and echo a bounded value preview), an executor that sends one round trip, and
a fake-server wire test. All values ride the exchange body through
`body_value_bytes`; all value replies use the binary-safe `reply_to_body`
early return (`Get` is the template). No value header is reintroduced.

## Design decisions

### D1. Option surface (SET/GETEX)

Operand headers stay per-argument; no generic options blob, no JSON options
header. Four new headers serve `SET`, three of them `GETEX`:

| Header | Values | Default | Used by |
| --- | --- | --- | --- |
| `CamelRedis.SetCondition` | `NX` \| `XX` (case-insensitive) | none | SET |
| `CamelRedis.KeepTtl` | boolean | false | SET |
| `CamelRedis.ReturnOldValue` | boolean | false | SET |
| `CamelRedis.ExpireUnit` | `SECONDS` \| `MILLISECONDS` | `SECONDS` | SET, GETEX |
| `CamelRedis.Persist` | boolean | false | GETEX |

`EX`/`PX` share `CamelRedis.Timeout` and `EXAT`/`PXAT` share
`CamelRedis.Timestamp`, exactly as `SETEX` vs `PEXPIRE` and `EXPIREAT` vs
`PEXPIREAT` share them today; the unit discriminator defaults to `SECONDS`,
matching `SETEX`/`EXPIREAT` intuition. Relative and absolute expiry are
mutually exclusive (`Timeout` + `Timestamp` → named error). `KEEPTTL` with any
expiry → named error. An `ExpireUnit` with no expiry header → named error
(misconfigured route template). NX + XX is structurally unrepresentable:
`SetCondition` is a single-valued enum header, which is stricter than a
runtime named error and is documented as such. Present-but-malformed boolean
or enum headers fail closed (ADR-0032). One mapping table goes into
`docs/src/components/redis.md` beside the existing Expiration table.

### D2. Blocking commands

`BLMPOP`, `BZMPOP`, `BLMOVE` join `RedisCommand::Blpop | RedisCommand::Brpop`
in `executor.rs::is_blocking_endpoint`, which selects `Deadline::Free` and
`set_response_timeout(None)` — the exact mechanism that keeps `BLPOP`/`BRPOP`
alive past the redis-rs 1.7.1 default 500 ms response timeout. Blocking
`Timeout` follows the existing pop convention: missing or `0` blocks forever.
A wire test per command proves the bypass: the fake server replies after
> 500 ms and the producer completes on one connection. `QueuePopCommand`
(queue adapter) stays `BLPOP`/`BRPOP`-only; the new blocking commands are
producer endpoints.

### D3. Retry classification

Rule: a command is retry-safe (`is_idempotent_command` → true) iff a retry
after an unconfirmed success converges server state AND does not lose data
the first attempt reported (a payload reply, a popped item, or a read whose
result a retry can no longer reproduce). Two accepted deviations keep the
classifier coherent with what already shipped: count-style replies may differ
across attempts (`DEL`/`UNLINK` report 1 then 0 — accepted, state converges),
and random-sample reads may return a fresh draw (`SRANDMEMBER` precedent —
state untouched). The classifier is option-blind, so a command whose worst
case option form violates the rule classifies false. Classified per command:

| Verdict | Commands | Reason |
| --- | --- | --- |
| true (reads) | `GETRANGE`, `SMISMEMBER`, `SINTERCARD`, `ZDIFF`, `ZINTER`, `ZUNION`, `ZRANGEBYLEX`, `ZRANDMEMBER`, `HRANDFIELD`, `LPOS`, `TOUCH` | read-only; random-sampling reads follow `SRANDMEMBER` |
| true (converging writes) | `SETRANGE` (same bytes re-written at a fixed offset), `UNLINK` (mirrors `DEL`, count reply accepted) | retry re-applies an overwriting delete/write |
| false | `LMPOP`, `BLMPOP`, `ZMPOP`, `BZMPOP`, `LMOVE`, `BLMOVE` | pop/move family: a retry reports an empty pop or a different item and silently loses the popped one |
| false | `SET` (reclassified) | the new option forms break the rule and the classifier cannot see them: `ReturnOldValue` makes a retry report the post-write value as the old value, and `NX` reports failure after an unconfirmed success. Worst-case form governs. This reclassifies the existing plain `SET` too. There is no per-route override of the classifier (the producer always consults it), so routes that relied on component-level plain-`SET` retry must move to route-level redelivery, which re-executes the whole exchange and may duplicate other side effects — docs say so. `SETEX`/`SETNX`/`GETSET` are already false. |
| false | `GETDEL`, `GETEX` | `GETDEL` consumes the key: a retry reports nil and the payload is gone. `GETEX` re-arms the expiry; if the value lapses between attempts the retry reports nil, so the first reply is not reproducible. |
| false | `COPY` | without `REPLACE` a retry reports 0 after an unconfirmed success; with `REPLACE` the retry re-reads the source, which may have changed between attempts |
| false | `HINCRBYFLOAT` | increment family: response encodes cumulative state (mirrors `INCRBY` being false) |

### D4. Multi-key sources

`CamelRedis.Keys` already exists (`MGET`) and is reused by `LMPOP`, `BLMPOP`,
`ZMPOP`, `BZMPOP`, `SINTERCARD`, `ZDIFF`, `ZINTER`, `ZUNION`, `TOUCH`. The
existing `get_str_vec_header` silently drops malformed entries, so a new
`require_key_list` helper replaces it at every new consumer: the header must
be a JSON array of strings, non-empty, with every entry valid — a malformed
entry fails closed naming the header and the offending index. `WEIGHTS`/
`AGGREGATE` are deferred: `ZUNIONSTORE`/`ZINTERSTORE` shipped without them, so
deferring keeps parity with the existing surface; they are a bd follow-up,
not silently dropped.

### D5. Spec surface

New `redis-commands` capability, one requirement per family (string options,
string ranges, lists, sets, sorted sets, hashes, generic keys, blocking
bypass, retry classification). No existing capability owns the command table:
`redis-values` owns payloads, `redis-geo`/streams own their families.

## Remaining header names

`CamelRedis.Where` (`LEFT`/`RIGHT`, required — `LMPOP`/`BLMPOP` only),
`CamelRedis.OrderBy` (`MIN`/`MAX`, required — `ZMPOP`/`BZMPOP`; sorted-set
pops order by score, not by list end), `CamelRedis.WhereFrom`/
`CamelRedis.WhereTo` (`LMOVE`/`BLMOVE`, required), `CamelRedis.Rank`
(`LPOS`, non-zero — any positive or negative rank is valid, `RANK 0` is a
named error), `CamelRedis.Limit` (`SINTERCARD`, non-negative),
`CamelRedis.Replace` (`COPY`), `CamelRedis.Offset` (`SETRANGE`, non-negative —
offset 0 is legal). Reused: `Key`, `Keys`, `Destination`, `Timeout`,
`Timestamp`, `Count`, `Min`, `Max`, `Member(s)`, `Field`, `Db` (optional;
absent means the current database, never hard-coded 0), `Increment` (parsed
as f64 for `HINCRBYFLOAT`). The `LPOS` search element is a payload: it rides
the exchange body through `body_value_bytes` (body-only contract, ADR-0098),
not a header. Argument domains: `Timeout` positive for `SET`/`GETEX` expiry
(Redis rejects 0), missing/0 = block forever for the blocking family;
`Count` ≥ 1 for `LMPOP`/`BLMPOP`/`ZMPOP`/`BZMPOP`, any i64 for
`ZRANDMEMBER`/`HRANDFIELD` (negative = with repeats, Redis semantics), and
≥ 0 for `LPOS` (0 = all matches, Redis semantics).
Member/element collection replies follow the `MGET` binary rule: all-UTF-8
replies map to JSON arrays; any binary element switches the whole reply to
the RESP-encoded `Bytes` body.

## Affected crates

- `camel-component-redis`: config, executor, six command modules, tests, docs.
- `camel-test` / `camel-cli`: compile-time only (endpoint surface grows).

## Architecture boundaries

Components only. No runtime, DSL, services, or language changes. The endpoint
URI surface (`command=`) gains values; URI parsing is untouched.

## Phases

- **Phase 1 — string family**: 4 variants, SET option surface, GETDEL/GETEX/
  SETRANGE/GETRANGE, wire tests, docs rows + option table.
- **Phase 2 — 6.2/7.0 collections**: 16 variants, dispatch arms +
  `is_blocking_endpoint`, family resolvers/executors, `require_key_list`,
  wire + blocking-bypass tests, docs rows.
- **Phase 3 — generics and close-out**: `UNLINK`/`COPY`/`TOUCH`, wire tests,
  docs complete, idempotency + blocking lists final, validate + archive check.
