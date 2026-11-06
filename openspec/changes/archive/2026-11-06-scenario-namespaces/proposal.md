# Proposal: scenario-namespaces

## Why

Scenario documents leave Redis keys whose names are generated at run time
(`112:state:event:<clock>`, `resolve:<id>`, per-run counters). A literal
`redis: del` list can neither pre-clean nor post-clean them, and a global
flush is wrong under parallel execution. Mission 511 (bd rc-ql8eq) was torn
down at the spec bless because its mint format was not prefix-free: the
prefix of document 10 also matched the keys of document 100. This change
re-lands the feature on the binding e_opus ruling (2026-11-06): a
prefix-free per-document namespace minted by the harness, and one narrow,
provably-scoped pattern delete (`unlinkPrefix`) allowed only in `finally:`.

## What Changes

- New top-level document key `namespaces: [{bindVar, base}]`. The harness
  mints `BASE~it~RUN~ORD~` per document (BASE `[A-Za-z0-9][A-Za-z0-9_-]{0,31}`,
  RUN 8 lowercase hex from the OS CSPRNG once per `camel test` invocation,
  ORD the 1-based zero-padded 4-digit index in the deterministic discovered
  order before filters). Prefix-free by two independent guarantees: the
  trailing `~` and the fixed widths. Load-time validation: bindVar unique
  and reserved against `env:` keys, passthrough names, and partner bindVars;
  base valid and unique per document; >9999 documents is a load error.
- The minted value binds in the harness layer of `LayeredEnv`, so
  `${env:NAME}` resolves to it; it never appears in the document env map.
- New redis action field `unlinkPrefix` under the existing `redis:` key,
  one-of with `del`, allowed ONLY in `finally:`. Template is exactly
  `${env:NAME}` plus an optional suffix; NAME must be a declared namespace
  bindVar; no `:-default`; no second `${`; suffix free of Redis glob
  metacharacters.
- Runtime (feature `redis`): resolve NAME from the harness layer only,
  byte-re-check against the minted value before any SCAN, pattern is the
  escaped namespace-plus-suffix plus exactly one unescaped `*`, SCAN
  `COUNT 500`, per-key byte re-check before UNLINK (violating key fails the
  batch with nothing unlinked), UNLINK batches of at most 500 within a
  10 s deadline and a 100 000-key cap. Breaches report under the existing
  `teardown-failure` class with the reason and keys deleted so far. No new
  failure class. Cluster datasources refused.
- ADR-0069 §4 and §8.4 amendments; `CONTEXT-MAP.md` index entry; docs pages
  `scenario-documents.md` and `scenario-redis.md`.

Out of scope: no cross-run reaper (it would delete a concurrent CI job's
keys; the ~2^-32 concurrent-job collision probability is ADR-recorded), no
ephemeral/testcontainer Redis provisioning, no cluster support, no new
command surface. `del`, `KEYS`, `FLUSHDB`, `FLUSHALL`, and generic command
runners keep their current contracts (`KEYS`/`FLUSHDB`/`FLUSHALL` stay
forbidden).

## Acceptance criteria

- The eleven mission acceptance tests pass (mint prefix-freeness across
  documents 10 and 100, per-document binding and `${env:NAME}` visibility,
  per-invocation run id, duplicate bindVar/base and env-collision load
  errors, ordinal zero-padding and the >9999 bound, the four unlinkPrefix
  load rules, in-namespace-only deletion with surviving out-of-namespace
  sentinel, unaligned-SCAN batch refusal, deadline/cap `teardown-failure`).
- ADR-0069 §4 carries the namespace grammar paragraph; §8.4 carries the
  unlinkPrefix exception after the rc-ql8eq deferral.
- `cargo test -p camel-integration-test` (featureless, `--features redis`,
  `--features sql`) and the mission gate list pass.

## Risk budget

The safety contract is the point of the change: byte re-checks, escaping,
batching, deadline, and cap are all load-bearing and none may be weakened
for convenience. Acceptable risk: stale namespaces from crashed runs
accumulate (accepted, no reaper). Out of bounds: weakening the prefix-free
mint grammar, adding any new failure class, sweeping across runs.

Affected crates: camel-integration-test, camel-cli. References bd rc-ql8eq.
