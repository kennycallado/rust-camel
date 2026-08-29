# Proposal: camel-config-env-override-hygiene

## Why

The env-override activation for `cache_repo` (bd rc-2o9e, squash `9f59069a`)
left three follow-up defects in `crates/camel-config/src/config.rs` (6,500
lines, the workspace's largest file):

1. **rc-fd4f** — all 48 `ENV_OVERRIDE_LOCK.lock().unwrap()` sites are
   `#[cfg(test)]`-only; one assert panic poisons the lock and cascades into
   ~40 downstream `PoisonError` test failures (noise-only, never false
   green).
2. **rc-xq3t** — legacy String-typed allowlisted vars (`STALE_RETENTION`,
   `PATH`, `BACKEND`) lack the verbatim passthrough the newer vars got, so a
   bare-numeric value fails with the cryptic deserialization error
   `invalid type: integer` instead of a field-specific validation error.
3. **rc-nucw** — inline `#[cfg(test)]` modules bloat `config.rs`; extraction
   is overdue.

e_gpt pre-research (2026-08-29) ruled GO-WITH-CHANGES: unitless numeric
durations stay unsupported (no implied-seconds contract); numeric fields
(`MAX_CAPACITY`, `MAX_ENTRIES`, `DB`) stay strictly typed; the empty-scalar
skip scope must not change.

## What Changes

- Poison-safe test-only `env_lock()` helper replacing every direct lock
  acquisition (rc-fd4f).
- Verbatim string passthrough for the three legacy String vars via a new
  `LEGACY_STRING_ENV_OVERRIDES` const, plus a unit-bearing upgrade of the
  two duration validation error messages (today they say only
  `invalid duration '<value>'`). Bare numerics then fail at downstream
  validation: `STALE_RETENTION` with the duration-format error, `BACKEND`
  with the unknown-backend error; `PATH=007` succeeds verbatim (rc-xq3t).
- Docs sync: `docs/src/configuration/schema.md` env table (8 newer vars
  missing), `crates/camel-config/CONTEXT.md` allowlist authority section,
  OpenSpec capability delta for the typed contract.
- Mechanical extraction of inline test modules to `src/config_tests/*.rs`
  sibling modules (rc-nucw).

Excluded: any change to the `EMPTY_SCALAR_ENV_OVERRIDES` skip scope, any new
allowlist var, any public API change, any cross-crate change.

## Acceptance criteria

- An assert panic while holding `ENV_OVERRIDE_LOCK` no longer poisons
  downstream tests; a regression test proves lock recovery.
- `CAMEL_CACHE_REPO_STALE_RETENTION=604800` fails with the stale-retention
  duration error naming accepted units; `=7d` applies; `CAMEL_CACHE_REPO_PATH=007`
  stays verbatim `Some("007")`; credential-denial tripwires stay green.
- The schema doc table lists all 13 override vars with typed/empty semantics.
- `config.rs` inline test modules live in `src/config_tests/` with zero
  test-count change (parity recorded pre/post).

## Risk budget

Acceptable: mechanical test-code movement, typing-behavior change for three
legacy vars, error-message wording for the two duration fields. Out of bounds: empty-value semantic drift (e_gpt risk 1),
widening item visibility for extraction (risk 2), weakening the L-C2
credential-denial invariant (risk 3).

Affected crates: `camel-config` (+ `docs/`, `openspec/specs/`). bd: rc-fd4f,
rc-xq3t, rc-nucw (follow-ups of rc-2o9e).
