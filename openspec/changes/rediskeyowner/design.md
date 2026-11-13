# Design: rediskeyowner

## Approach

**Key resolution (producer.rs).** Replace `apply_default_key` with a pure
resolver mirroring `resolve_command`'s URI-first shape:

```rust
fn resolve_key(exchange: &Exchange, config: &RedisEndpointConfig) -> Option<String>
```

Returns the URI `key` when present, else the `CamelRedis.Key` header as a
string, else `None`. A small publish step sets `CamelRedis.Key` to the
effective value when `Some` — so the header never carries a stale key after
dispatch, matching decision 2 (publish the effective value, never a stale
one). `call()` keeps the same ordering: resolve command → resolve/publish
key → default channels → `execute_with_retry`. `require_key`
(`commands/mod.rs:177`) still reads `CamelRedis.Key`; only precedence and
ordering change. When neither source yields a key, the header is untouched
and `require_key` fails with its existing missing-header error.

When the URI declares no key and the `CamelRedis.Key` header holds a
non-string value, the resolver returns `None`, leaves the header untouched,
and `require_key` surfaces its existing `Missing required header:
CamelRedis.Key` error. A configured URI key overwrites any header value,
including a non-string one.

**Classifier (config.rs).** Delete `Rename`, `Renamenx`, `Move` from the
idempotent-writes arm of `is_idempotent_command`, with a one-line comment:
a retry after an unconfirmed success reports visible failure (`ERR no such
key` / `0`). No classifier split, no endpoint flags — phase 2.

**Tests.** TDD: write the named tests first, watch them fail, then fix.

**Spec delta.** `specs/redis-streams-producer/spec.md` in the change carries
two MODIFIED requirements (key sentence inverted in both) with the XADD
scenario renamed + inverted and a `renamed:` openspec-scenario-ops directive,
plus one ADDED requirement for the classifier. The directive is what makes
`cargo xtask archive --check` pass.

**Docs.** `docs/src/components/redis.md:241` (XADD paragraph) and `:251`
(header table row) state the old precedence; each touched spot gains a
one-sentence migration note. `README.md` is checked for the same claim.

## Affected crates

- `camel-redis`: `producer.rs` (resolver + publish), `config.rs`
  (classifier), `executor.rs` (regression test), `stream_fake_tests.rs`
  (wire-level regression test), `README.md` (if it states old precedence).

## Architecture boundaries

Component-internal only (`Components` layer). The exchange contract and
Tower `Service` shape are unchanged; no `camel-api`/`camel-core` surface
moves. ADR-0032 (endpoint URI is the sole command source) is the precedent
for URI-first resolution; ADR-0090 covers the pre-1.0 breaking change; no
new ADR (mission decision 4). Data/control plane split untouched.

## Alternatives considered

- *Stop writing the default key back as a header* (bd option): rejected —
  loses decision 2's observable effective key.
- *Per-endpoint key-source flag* (bd option): rejected — config surface for
  a phase-0 correctness fix; the URI-first default is the sane semantics.
- *Split classifier into "not executed / outcome unknown"*: phase 2, per
  mission decision 3.
