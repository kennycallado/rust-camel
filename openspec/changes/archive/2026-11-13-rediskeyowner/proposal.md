# Proposal: rediskeyowner

## Why

Two defects in `camel-redis` sit at the same seams the Redis 7 atomicity work
(phase 0, e_opus 2026-11-13) builds on, so they land before it.

**A. The exchange header beats the endpoint's own URI key (`rc-kcpuw`).**
`apply_default_key` (`producer.rs:65-72`) writes the endpoint `key=` into
`CamelRedis.Key` only when the header is absent. An earlier step's header
therefore wins over this endpoint's own configuration:
`from(BLPOP key=in).to(XADD key=out)` appends into `in` (WRONGTYPE on a list
key, or a list-to-list route that loops). The BLPOP consumer sets the same
header (`consumer.rs:735`), so any consumer followed by a producer inherits
the consumed key.

**B. Three commands are wrongly classified idempotent (`rc-bvfzk`).**
`is_idempotent_command` (`config.rs:1550-1560`) lists `Rename`, `Renamenx`
and `Move` as repeatable. A retry after an unconfirmed success turns success
into a visible failure: `RENAME` answers `ERR no such key`, `RENAMENX` and
`MOVE` answer `0`. Nothing else in that list is wrong.

**C. The canonical spec documents the broken precedence.**
`openspec/specs/redis-streams-producer/spec.md` says twice "named by
`CamelRedis.Key` (falling back to the URI `key=`)" and `docs/src/components/redis.md`
repeats it. The producer test `test_apply_default_key_preserves_existing_header`
asserts the broken semantics and must change, not be preserved.

## What Changes

- Key resolution inverts: **endpoint URI `key=` wins; `CamelRedis.Key` is the
  fallback** when the URI declares no key. Breaking for routes that relied on
  header-over-URI; commit carries `!` with a migration note.
- The effective key is published on `CamelRedis.Key` after resolution (a later
  step can observe it; never a stale value). `apply_default_channels` is out
  of scope.
- Classifier: remove the three variants only — no classifier split, no
  endpoint flags (phase 2).
- Spec delta for `redis-streams-producer` + docs updates.

Affected crates: `camel-redis` only.

## Acceptance criteria

- URI `key=` beats a pre-existing `CamelRedis.Key` header; the frame targets
  the URI key and the header carries the effective key afterwards.
- `Rename`, `Renamenx`, `Move` classify non-idempotent; a transient failure
  on them is attempted exactly once with no reconnect.
- `openspec validate rediskeyowner --type change --json` passes;
  `cargo xtask archive rediskeyowner --check` passes.
- Docs and spec no longer state the old precedence.

## Risk budget

Breaking change accepted under ADR-0090 (pre-1.0). Migration is one sentence:
omit `key=` from the URI to keep per-message key routing. No ADR (spec delta
under ADR-0090). No behavior change for other commands in the classifier.

Bd: rc-kcpuw, rc-bvfzk
