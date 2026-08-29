# Design: camel-config-env-override-hygiene

## Approach

Four workstreams, two delivery phases.

**1. Poison-safe test lock (rc-fd4f).** Every `ENV_OVERRIDE_LOCK`
acquisition is `#[cfg(test)]`-only (`config.rs:2980-3002`); the lock
coordinates env-var mutation between tests and guards no production state.
Add one helper `fn env_lock() -> MutexGuard<'static, _>` using
`.unwrap_or_else(std::sync::PoisonError::into_inner)` and replace all 48
direct `.lock().unwrap()` acquisitions. Safe because each env test restores
variables before result assertions. `lint-unwrap` excludes `#[cfg(test)]`
scopes (`scripts/xtask/src/main.rs:1459-1471`), so the helper is
gate-compliant.

**2. Typed override contract for legacy String vars (rc-xq3t).** New const
`LEGACY_STRING_ENV_OVERRIDES = [CAMEL_CACHE_REPO_BACKEND, _PATH,
_STALE_RETENTION]`; the merge-loop dispatch sends them through the same
verbatim-string arm as `STRING_ENV_OVERRIDES`. They are NOT appended to
`STRING_ENV_OVERRIDES` — that would break the `STRING ⊆ EMPTY_SCALAR`
subset assertion (`config.rs:6465`) and the
`empty_preexisting_typed_override_still_fails` scope pin — and NOT added to
`EMPTY_SCALAR_ENV_OVERRIDES` (its doc comment pins pre-existing vars to
their exact current behavior). New test assertions: the legacy list is
disjoint from `STRING`/`CSV`/`EMPTY_SCALAR`. Effect: bare numerics now fail at field validation instead of
deserialization — `PATH=007` stays `Some("007")` (override succeeds),
`BACKEND=123` yields the unknown-backend validation error, and
`STALE_RETENTION=604800` reaches the `humantime::parse_duration` mapping at
`config.rs:1987` (runs for every non-memory backend). That mapping's
current message (`invalid duration '604800'`) does NOT name the required
format, so Phase 1 also upgrades both cache_repo duration validation errors
(`config.rs:1987` for `stale_retention`, `:2131` for `sweep_interval`) to a
stable unit-bearing message:
`cache_repo.{field}: invalid duration '{value}' — use a unit-bearing form
such as '7d' or '24h'`. Per e_gpt: unitless durations remain unsupported; no
implied-seconds semantics.

**3. Docs sync.** `docs/src/configuration/schema.md` env-override table
(stale: misses the 8 newer vars) → full 13-var table with typed/empty
semantics and the duration-unit rule. `crates/camel-config/CONTEXT.md`
allowlist section is cited as authority by the docs — update with the typed
contract. Capability delta spec (this change) is the normative record.

**4. Test extraction (rc-nucw).** Move inline `#[cfg(test)]` modules
(`config.rs:1102`, `2987+`) to `src/config_tests/*.rs` siblings via
`#[path = ...] mod ...;` — precedent `camel-core/src/cache/disk_offload.rs:673-675`,
`camel-processor/src/multicast_segment.rs:346-348`. Keeps private items
(`ENV_OVERRIDE_LOCK`, the consts) reachable without visibility widening;
unit tier stays inside the crate, so ADR-0064's two-tier boundary and
camel-core's architecture gate are untouched. `tests/` integration files
stay as-is.

## Affected crates

- `camel-config`: `config.rs` (helper, legacy const + dispatch, extraction),
  new `src/config_tests/*`
- `docs/src/configuration/schema.md`, `crates/camel-config/CONTEXT.md`
- `openspec/specs/cache-repo-configuration` via this change's delta

## Architecture boundaries

Pure config-load path; no runtime component, data plane, or control plane
interaction. The L-C2 security invariant (`config.rs:2452-2466`: no
security-sensitive field overridable via env) is untouched — credential
denial tripwires remain the regression watch. ADR-0063/0065 cache-repository
context is unaffected. Typing behavior changes for three legacy vars
(`STALE_RETENTION`, `PATH`, `BACKEND`); error-message wording changes only
for the two duration fields.

## Phases

### Phase 1: typed contract + poison safety + docs

- **Goal:** override typing contract live, poison cascade eliminated, docs
  truthful.
- **Dependencies:** rc-2o9e merged (`9f59069a`).
- **Externally-visible types/interfaces:** none new; typed-behavior changes
  for `STALE_RETENTION`/`PATH`/`BACKEND` bare numerics (verbatim string
  passthrough) and unit-bearing error messages for the two duration fields.
- **Deliverable:** helper + const + dispatch + regression tests + docs/spec
  updates.
- **Exit-criteria:** new regression tests green; credential tripwires green;
  phase gate: `cargo check -p camel-config`, `cargo test -p camel-config
  --lib`, `cargo fmt --check`, `cargo clippy -p camel-config -- -D
  warnings`, `cargo xtask lint-unwrap` all green.

### Phase 2: mechanical test extraction

- **Goal:** `config.rs` inline test modules extracted to `src/config_tests/`.
- **Dependencies:** Phase 1 (extracts the stabilized test set, including the
  new regression tests).
- **Externally-visible types/interfaces:** none.
- **Deliverable:** `config.rs` line-count drop; zero test-count change.
- **Exit-criteria:** `cargo test -p camel-config --lib` test count parity
  pre/post (counts recorded in the task); no visibility widening in the
  diff; same phase gate as Phase 1 (`cargo check`, `--lib` tests, fmt,
  clippy, `lint-unwrap`).

## Alternatives considered

- **Extend `STRING_ENV_OVERRIDES` in place** — rejected: breaks the subset
  assertion and couples legacy empty semantics to the skip list (e_gpt
  risk 1).
- **Move tests to `tests/` integration files** — rejected: private items
  would need `pub`/`#[doc(hidden)]` exposure (e_gpt risk 2).
- **Keep `.unwrap()` in production paths, recover only in tests** — moot:
  zero production lock sites exist.
