# Design: envguard-global-state

## Approach

**EnvGuard** (new file `crates/components/camel-component-api/src/test_support/env.rs`, module
already `#[cfg(any(test, feature = "test-support"))]`):

```text
static ENV_LOCK: Mutex<()> = Mutex::new(());
pub fn env_lock() -> MutexGuard<'static, ()>   // unwrap_or_else(PoisonError::into_inner)
#[must_use] pub struct EnvGuard {
    undo: Vec<(String, Option<std::ffi::OsString>)>,  // replayed in reverse on drop
    _lock: MutexGuard<'static, ()>,                    // declared last: Drop::drop runs before
}                                                      // field drops, so restore happens while the
                                                       // lock is still held
impl EnvGuard {
    pub fn acquire() -> Self                                  // lock-only; reader tests (rc-yzpb family)
    pub fn set(key: impl Into<String>, value: impl Into<OsString>) -> Self
    pub fn remove(key: impl Into<String>) -> Self
    pub fn and_set(&mut self, key, value) -> &mut Self        // chained multi-key on ONE guard
    pub fn and_remove(&mut self, key) -> &mut Self            // (constructors and chaining have
}                                                             //  DISTINCT names: no Rust overload)
```

- `Drop::drop` replays `undo` in reverse: restore previous `OsString` value or remove if absent.
- All `unsafe { env::set_var/remove_var }` calls live ONLY in this module (edition 2024,
  rust 1.89: set_var is unsafe), each with a SAFETY comment.
- **Soundness preconditions** (documented in `env.rs` module docs, stated honestly):
  `set_var`/`remove_var` are `unsafe` because libc env access is not thread-safe; the safety
  contract requires excluding concurrent access. What the guard guarantees: every Rust-side
  participant in the test process (mutators and guarded readers) is serialized by `ENV_LOCK`.
  What it CANNOT guarantee: non-Rust readers (libc DNS resolver, locale init, FFI) may read
  the environment concurrently and cannot be excluded inside a multithreaded test process —
  this residual is the documented, accepted limitation of the legacy exception (epic rc-99d5
  adjudication R4 as amended: "env lock plus restore narrows but is not hermetic"). The module
  docs state the mitigation hierarchy, not a soundness proof: (a) direct config injection
  first (no env mutation), (b) out-of-process child with explicit env for tests whose SUBJECT
  is environment behavior, (c) guarded in-process mutation as the legacy exception for
  config-driven tests.
- Panic-safe: unwind still runs `Drop`, restoring env while holding the lock; poisoning is
  neutralized with `PoisonError::into_inner` (the lock guards ordering, not an invariant).
- `camel-config` gains `camel-component-api` as a **dev-dependency** with `test-support`
  (no cycle: component-api does not depend on camel-config). Its `pub(crate) env_lock()` in
  `config.rs` and the ad-hoc `ENV_LOCK` in `camel-bench` / `TRUSTSTORE_ENV_LOCK` in
  `camel-cli` compile/runtime tests are replaced by the shared guard.

**Cluster migration** (rg inventory on base 789c84faa: 182 `set_var|remove_var` lines / 44 files):
high-risk clusters first, named: (1) camel-config ambient `CAMEL_PROFILE` reads
(config_tests/, tests/), (2) log-capture / tracing-adjacent, (3) `CAMEL_*` mutations in
camel-dsl tests (env_interpolation 26 sites, discovery, yaml, env_escape_regression),
(4) `RUST_LOG`/`XDG_*`/`HOME` mutations (camel-http tls_tests, camel-test support, camel-cli),
(5) camel-bench/camel-jms local-lock sites. Remainder: migrate-as-touched or mark each
residual raw site with an adjacent `// env-guard-exception: <reason>` line and seed it into
the lint ledger.

**`::global()` disposition audit**: every test-scope `::global()` callsite in the inventory
(the ticket's ~72; production `::global()` reads are recorded as excluded, not redesigned)
gets a disposition: (a) migrated to per-test state, (b) hoisted under `#[cfg(test)]` AND
reset/guarded between tests (`#[cfg(test)]` moves visibility, it does NOT isolate sibling
tests — a shared test-only static needs a `Mutex<Option<T>>` + reset or per-test
reinitialization), or (c) production static touched by tests → test-side store/restore via
EnvGuard-equivalent discipline, zero prod redesign. The dispositions table is a deliverable
recorded in the change dir.

**Lint** (`scripts/xtask/src/lint_no_env_mutation_unguarded.rs`, mirroring lint-no-free-port):
- Lexical scan for `env::set_var` / `env::remove_var` tokens in test-scope files
  (path contains `/tests/`, or filename `*_tests.rs`/`test_*.rs`, or file contains `#[cfg(test)]`).
  `EnvGuard::set` does not contain the token, so migrated code passes trivially; production
  spawn-path usage (camel-bridge process/download, camel-cli main) is not test-scope.
- **Per-site adjudication**: a raw site is legal only with an adjacent (same or preceding line)
  `env-guard-exception: <non-empty reason>` marker. Missing/malformed marker → failure naming
  file and line. Markers prevent stealth swaps: a NEW unmarked site always fails.
- **Seeded monotone ratchet ledger** `scripts/xtask/src/lint_no_env_mutation_unguarded_allowlist.txt`:
  per-file `count reason` lines for the marked residual sites. Gate fails when a file's marked
  count EXCEEDS its entry (regression) or is BELOW it (entry must be lowered in the same
  change — the seed only ever decreases; the ledger baseline never rises). Seed snapshot is
  taken AFTER cluster migration, so the landing state is green without demanding zero.
- Wire into `.github/workflows/ci.yml` next to lint-no-free-port (line ~331) and add to the
  AGENTS.md `## QUALITY GATES` fenced block.

**Hostile-ambient proof** (cluster: camel-config profile loading, rc-yzpb family): new test in
`config_tests/profile_loading_tests.rs`. A guard holds `CAMEL_PROFILE=production` (the hostile
ambient value that broke CI run 33887809098 — with `[default]`-only configs, an ambient
production makes `from_file` fail "Unknown profile: production") for the whole test, and the
load goes through the existing explicit-profile entry point
`CamelConfig::from_file_with_profile(path, Some("default"))` — explicit selection wins over
ambient (`profile.or(env_profile)`), so the loader never consults the hostile value. The
config is the original `[default]`-only placeholder document; the ORIGINAL deterministic
assertions are retained verbatim (routes default, `base_url`, `plugin`, `token` fallback
resolution). Pass/fail is decided by explicit profile selection + guard serialization, not by
following the hostile value.

## Affected crates

- camel-component-api: new `test_support::env` (feature-gated, no prod change).
- camel-config: dev-dep on camel-component-api; migrate config_tests/ + tests/ env sites; drop
  local `env_lock` usage in tests.
- camel-dsl, camel-http, camel-jms, camel-bench, camel-cli, camel-test, camel-core: migrate
  cluster sites; camel-bench drops local ENV_LOCK.
- scripts/xtask: new lint + allowlist ledger.
- .github/workflows/ci.yml, AGENTS.md: gate registration.

## Architecture boundaries

Test-support only: no production crate gains behavior; the DSL env interpolation,
Config loading, and Components runtime are untouched (their `#[cfg(test)]` modules and
`tests/` dirs are). The lint is an xtask gate, same tier as lint-no-free-port. Services
(camel-bridge, camel-auth) spawn-path env use stays production-legit and out of test scope.

## Phases

Single-phase: EnvGuard → migrations → audit → lint+ledger → gates, as one ordered task list.
(The seed snapshot must post-date migration; a phase split would not change the ordering
constraints, only add a review boundary mid-chain.)

Bd: rc-zkfub
