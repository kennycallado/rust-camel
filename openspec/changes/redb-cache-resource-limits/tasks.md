# Tasks: redb-cache-resource-limits

## Phase 1: close the OOM class (incident fix)

### camel-core (cache backend)

#### Task 1.1: RedbCacheRepository required cache_size, builder open, accessors

**Files:**
- `crates/camel-core/src/cache/redb.rs` (modified)
- `crates/camel-config/src/context_ext.rs` (modified — keeps workspace green)

**Steps:**
1. Add fields `cache_size: usize` and `sweep_interval: std::time::Duration` to `RedbCacheRepository` (the sweep task currently consumes the interval without the struct retaining it).
2. Add public accessors `pub fn cache_size(&self) -> usize`, `pub fn sweep_interval(&self) -> std::time::Duration`, and `pub fn stale_retention(&self) -> std::time::Duration` returning the recorded values — the propagation seam required by the eip-cache spec (add a `stale_retention` struct field too if the struct does not already retain it).
3. Change `RedbCacheRepository::new` signature: insert `cache_size: usize` after `max_entries`. Inside the existing `spawn_blocking` block, replace `redb::Database::create(&path_for_db)` with `redb::Builder::new().set_cache_size(cache_size).create(&path_for_db)`, mapping errors to `CamelError::Io` exactly as the current open does. There is no `Option` — every caller states a budget.
4. Update the in-file test helper `new_repo` (redb.rs:498) and every existing in-file constructor call to pass an explicit size (use `256 * 1024 * 1024` in the helper).
5. Update the sole cross-crate caller `crates/camel-config/src/context_ext.rs:247` to pass an interim literal `256 * 1024 * 1024` in the new parameter position, with a `// interim literal: task 1.5 wires the parsed cache_repo.cache_size here` comment — keeps `cargo build --workspace` green after this task.

**Tests** (in-file `mod tests`, run via `cargo test -p camel-core --lib cache::redb`):
- `cache_size_recorded_and_accessible`: construct via `new_repo` variant with `cache_size = 536870912` → `repo.cache_size() == 536870912`
- `sweep_interval_recorded_and_accessible`: construct with `sweep_interval = Duration::from_secs(1800)` → `repo.sweep_interval() == Duration::from_secs(1800)`
- `stale_retention_recorded_and_accessible`: construct with `stale_retention = Duration::from_secs(3600)` → `repo.stale_retention() == Duration::from_secs(3600)`
- `explicit_cache_size_round_trip`: construct with an explicit size, `set("k", entry, Some(1h))` → `get("k")` returns the entry (database opened through the builder serves full round-trip)
- Existing reopen/sweep/shutdown/error tests continue to pass unmodified except for the `new_repo` signature change

**Acceptance:**
- `cargo test -p camel-core --lib cache::redb` exits 0
- `cargo clippy -p camel-core -- -D warnings` exits 0
- `cargo fmt --check` clean

- [x] 1.1

#### Task 1.2: cgroup memory-limit guardrail

**Files:**
- `crates/camel-core/src/cache/redb.rs` (modified)

**Steps:**
1. Add `pub(crate) fn memory_limit_from_paths(v2: &Path, v1: &Path) -> Option<u64>`: read v2 file, trim content; `"max"` or unparseable-as-u64 content → fall through to v1 (no error); parseable → `Some(bytes)`. v1 file: parse u64; values strictly greater than 16 TiB (`> 17_592_186_044_416`) are the cgroup v1 unlimited sentinel → treated as no limit (`None`), matching the blessed spec's "above 16 TiB" wording. Missing/unreadable files at either path → fall through / `None`. All I/O via `std::fs::read_to_string(path).ok()` — no panics, no unwrap.
2. Add `pub(crate) fn emit_memory_guardrail(cache_size: usize, v2: &Path, v1: &Path)` that calls `memory_limit_from_paths` and, when `Some(limit)` and `cache_size > limit`, emits exactly one `tracing::warn!` naming both numbers (message includes the decimal values of cache size and container limit).
3. Call `emit_memory_guardrail(cache_size, Path::new("/sys/fs/cgroup/memory.max"), Path::new("/sys/fs/cgroup/memory/memory.limit_in_bytes"))` from `new()` before spawning the sweep task. Diagnostic only — never fails construction.

**Tests** (in-file `mod tests`, temp files via the existing `tempfile` dev-dependency; tracing capture via a `tracing_subscriber` fmt subscriber with a `MakeWriter` into a shared `Vec<u8>` installed with `tracing::subscriber::with_default` — the crate is already a regular dependency of camel-core):
- `cgroup_v2_limit_parsed`: temp file containing `"805306368\n"` as v2, missing v1 → `Some(805306368)`
- `cgroup_v2_max_means_unlimited`: v2 file `"max"`, v1 file missing → `None`
- `cgroup_v2_malformed_falls_through`: v2 file `"not-a-number"`, v1 file `"1073741824"` → `Some(1073741824)` (fall-through, no panic)
- `cgroup_v1_sentinel_unlimited`: v2 missing, v1 file `"9223372036854771712"` → `None`
- `cgroup_files_missing`: both paths point at nonexistent temp paths → `None`
- `guardrail_warns_when_exceeds`: captured tracing output of `emit_memory_guardrail(1_073_741_824, v2_with_805306368, missing_v1)` contains `1073741824` and `805306368`, and the warn line appears exactly once
- `guardrail_silent_when_fits`: captured output of `emit_memory_guardrail(268_435_456, v2_with_805306368, missing_v1)` is empty
- `guardrail_silent_when_files_missing`: captured output of `emit_memory_guardrail(1_073_741_824, missing_v2, missing_v1)` is empty

**Acceptance:**
- `cargo test -p camel-core --lib cache::redb` exits 0
- `cargo xtask lint-unwrap` reports no new unwrap in the touched code
- `cargo clippy -p camel-core -- -D warnings` exits 0

- [x] 1.2

### camel-config (config plane)

#### Task 1.3: byte-size parser

**Files:**
- `crates/camel-config/src/config.rs` (modified)

**Steps:**
1. Add `pub(crate) fn parse_byte_size(s: &str) -> Result<usize, String>` near `default_stale_retention`. Semantics: trim input; match a case-insensitive suffix from `{b: 1, kb: 1_000, kib: 1_024, mb: 1_000_000, mib: 1_048_576, gb: 1_000_000_000, gib: 1_073_741_824}`; the numeric part parses as `u128` (empty or non-numeric → Err); multiply in `u128`; result must fit `usize` (overflow → Err with "overflow"); no space allowed between number and suffix. Error strings start with `"cache_repo.cache_size"` so validation errors name the field.
2. Add an in-file `#[cfg(test)] mod byte_size_tests` (config.rs has no existing in-file test module; this creates one co-located with the private function).

**Tests** (`cargo test -p camel-config --lib byte_size_tests`):
- `parses_plain_bytes`: `"4096"` → `Ok(4096)`
- `parses_decimal_and_binary_mb`: `"384MB"` → `Ok(384_000_000)`; `"512MiB"` → `Ok(536_870_912)`
- `parses_case_insensitive_suffix`: `"256mib"` → `Ok(268_435_456)`
- `parses_gb_and_gib`: `"1GB"` → `Ok(1_000_000_000)`; `"1GiB"` → `Ok(1_073_741_824)`
- `rejects_garbage`: `"thirty"` → `Err` containing `"cache_repo.cache_size"`
- `rejects_unknown_suffix`: `"5XB"` → `Err`
- `rejects_space_between_number_and_suffix`: `"512 MiB"` → `Err`
- `rejects_overflow`: `"18446744073709551616B"` (2^64) → `Err` containing `"overflow"`

**Acceptance:**
- `cargo test -p camel-config --lib byte_size_tests` exits 0
- `cargo clippy -p camel-config -- -D warnings` exits 0

- [x] 1.3

#### Task 1.4: CacheRepoConfig fields + strict validation

**Files:**
- `crates/camel-config/src/config.rs` (modified)
- `crates/camel-config/tests/cache_repo_config.rs` (modified)

**Steps:**
1. Add `pub cache_size: Option<String>` and `pub sweep_interval: Option<String>` to `CacheRepoConfig`; extend `Default` with `None` for both; update the struct doc-comment TOML example to include `cache_size = "256MiB"` and mention `sweep_interval`.
2. Extend the `cache_repo` block of `CamelConfig::validate()` (config.rs ~1176). When `backend == "redb"`:
   - `cache_size` None → `Err(CamelError::Config("cache_repo.cache_size must be set when backend is \"redb\" (e.g. \"256MiB\", \"384MB\", plain bytes)")`-style message)
   - `cache_size` present → `parse_byte_size` must succeed, else Err naming `cache_repo.cache_size`
   - `sweep_interval` present → `humantime::parse_duration` must succeed, else Err naming `cache_repo.sweep_interval`; parsed value must be > 0, else Err stating the interval must be positive
   - `stale_retention` present → `humantime::parse_duration` must succeed, else Err naming `cache_repo.stale_retention` (removes today's silent 7d fallback for malformed values; `None` still means "wiring applies 7d")
3. Update existing integration tests that build redb TOML (e.g. `redb_registered_when_backend_redb`) to include `cache_size = "256MiB"`.

**Tests** (`cargo test -p camel-config --test cache_repo_config`):
- `missing_cache_size_on_redb_rejected`: redb TOML without `cache_size` → `configure_context` errs with message containing `cache_repo.cache_size`
- `malformed_cache_size_rejected`: `cache_size = "thirty"` → err naming `cache_repo.cache_size`
- `overflowing_cache_size_rejected`: `cache_size = "18446744073709551616B"` → err naming `cache_repo.cache_size`
- `malformed_sweep_interval_rejected`: `sweep_interval = "1x"` → err naming `cache_repo.sweep_interval`
- `zero_sweep_interval_rejected`: `sweep_interval = "0s"` → err naming `cache_repo.sweep_interval` and stating positive
- `malformed_stale_retention_rejected`: `stale_retention = "forever-ish"` → err naming `cache_repo.stale_retention`
- `redb_with_cache_size_builds`: redb TOML with `cache_size = "256MiB"` → `configure_context` succeeds, `persistent` and `memory` both registered (updates the existing test 1)
- Existing empty-path and memory-backend tests still pass

**Acceptance:**
- `cargo test -p camel-config` exits 0
- `cargo clippy -p camel-config -- -D warnings` exits 0

- [x] 1.4

#### Task 1.5: wiring propagation via concrete repository factory

**Files:**
- `crates/camel-config/src/context_ext.rs` (modified)
- `crates/camel-config/tests/cache_repo_config.rs` (modified)

**Steps:**
1. Extract the `"redb"` arm body into a private async factory `async fn build_persistent_cache_repo(ccfg: &CacheRepoConfig, shutdown_token: CancellationToken) -> Result<camel_core::cache::RedbCacheRepository, CamelError>` in `context_ext.rs`. It resolves, with strict error propagation (`CamelError::Config` naming the field — defense in depth, unreachable post-validation, never silent): `cache_size` via `crate::config::parse_byte_size` (None → error naming `cache_repo.cache_size`); `sweep_interval` via `humantime::parse_duration` where `None` → `Duration::from_secs(3600)` and malformed or zero → error; `stale_retention` via `humantime::parse_duration` where `None` → `Duration::from_secs(7 * 24 * 3600)` and malformed → error; `max_entries` default 1_000_000.
2. The `"redb"` wiring arm calls the factory and registers the returned repository under `"persistent"` unchanged.
3. Replace the interim literal from task 1.1: the factory passes the parsed `cache_size` to `RedbCacheRepository::new` (remove the interim comment).
4. Add an in-file `#[cfg(test)] mod tests` in `context_ext.rs` calling the factory directly (tempdir path) and asserting on the concrete repository through its accessors — this is the direct verification of the eip-cache "reach the redb backend" / "defaults to one hour" scenarios (registration erases the concrete type behind `Arc<dyn CacheRepository>`, so the factory is the assertion point).

**Tests** (`cargo test -p camel-config --lib context_ext`, plus integration `--test cache_repo_config`):
- `factory_passes_cache_size_and_sweep_interval` (in-file, `#[tokio::test]`): `CacheRepoConfig` with `backend = "redb"`, tempdir path, `cache_size = Some("512MiB")`, `sweep_interval = Some("30m")` → factory repo has `cache_size() == 536_870_912` and `sweep_interval() == Duration::from_secs(1800)`
- `factory_defaults_sweep_interval_to_one_hour` (in-file): same config with `sweep_interval = None` → `sweep_interval() == Duration::from_secs(3600)`
- `factory_defaults_stale_retention_to_seven_days` (in-file): `stale_retention = None` → `stale_retention() == Duration::from_secs(7 * 24 * 3600)`
- `factory_rejects_malformed_sweep_interval` (in-file): `sweep_interval = Some("1x")` → `Err(CamelError::Config)` naming `cache_repo.sweep_interval`
- `factory_rejects_zero_sweep_interval` (in-file): `sweep_interval = Some("0s")` → `Err(CamelError::Config)`
- `factory_rejects_malformed_cache_size` (in-file): `cache_size = Some("thirty")` → `Err(CamelError::Config)` naming `cache_repo.cache_size`
- `factory_rejects_missing_cache_size` (in-file): `cache_size = None` → `Err(CamelError::Config)` naming `cache_repo.cache_size`
- `redb_builds_with_cache_size_and_sweep_interval` (integration, tests/cache_repo_config.rs): redb TOML with `cache_size = "512MiB"` and `sweep_interval = "30m"` → `configure_context` succeeds and `persistent` is registered
- `redb_builds_without_sweep_interval_using_default` (integration): redb TOML with `cache_size`, no `sweep_interval` → `configure_context` succeeds (wiring applies the 1h default without error)
- `redb_builds_without_stale_retention_using_default` (integration): redb TOML with `cache_size`, no `stale_retention` → succeeds (7d wiring fallback intact)

**Acceptance:**
- `cargo test -p camel-config` exits 0 (lib + all integration tests)
- `cargo clippy -p camel-config -- -D warnings` exits 0
- No `Duration::from_secs(3600)` literal remains outside the factory's sweep-interval `None` default; no interim comment remains at the constructor call site

- [x] 1.5

#### Task 1.6: ADR-0056 amendment + example comments

**Files:**
- `docs/adr/0056-cache-repository-port.md` (modified)
- `examples/cache-example/src/main.rs` (modified)

**Steps:**
1. In ADR-0056 Decision 6 (line ~118), keep the original text and append a dated amendment note: `> Amendment (2026-08-18): the 60s default documented above never shipped — the wiring hardcoded 1h. `sweep_interval` now makes the interval configurable via `[default.cache_repo]`; the default stays 1h because an O(N) sweep over a large persistent cache costs more than delayed reclamation.`
2. Update the example comment TOML snippet in `examples/cache-example/src/main.rs` (lines ~33-37) to include `cache_size = "256MiB"`.

**Tests:** none (documentation-only task).

**Acceptance:**
- `grep -n "60s" docs/adr/0056-cache-repository-port.md` shows the original mention only inside the quoted Decision 6 body followed by the amendment note
- `rg -l 'backend = "redb"' examples/` lists `examples/cache-example/src/main.rs` and its snippet contains a `cache_size` line; `rg -n 'cache_repo' examples/health-demo/src/main.rs` confirms no redb TOML snippet exists there (no edit needed)

- [x] 1.6

> **Phase 1 complete — inter-phase r_glm review REQUIRED before Phase 2 begins**
> (Phase 1 has ≥2 tasks; per the conductor flow, the phase diff is reviewed against the
> spec before the next phase-group starts. Phase 2 is single-task and skips this gate.)

## Phase 2: dead-config rejection on cache_repo

### camel-config (config plane)

#### Task 2.1: serde None default + cross-backend field rejection

**Files:**
- `crates/camel-config/src/config.rs` (modified)
- `crates/camel-config/tests/cache_repo_config.rs` (modified)

**Steps:**
1. Change `fn default_stale_retention() -> Option<String>` from `Some("168h".to_string())` to `None` (the serde attribute on the field already calls this function; omitted must deserialize as `None` so memory configs do not materialize a redb-only field). The `Default` impl already uses `None` — no change there.
2. Extend `validate()`: when `backend == "memory"`, reject any of `path`, `stale_retention`, `max_entries`, `cache_size`, `sweep_interval` being `Some` with an error naming the field and stating it does not apply to the `memory` backend. When `backend == "redb"`, reject `max_capacity` being `Some` with the same shape.
3. Wiring unchanged: `stale_retention: None` on redb already falls back to 7d in `context_ext.rs` (task 1.5 step 3).

**Tests** (`cargo test -p camel-config --test cache_repo_config`):
- `cache_size_on_memory_rejected`: `backend = "memory"` + `cache_size = "512MiB"` → err naming `cache_repo.cache_size` as not applicable
- `path_on_memory_rejected`: `backend = "memory"` + `path = "data/cache.redb"` → err naming `cache_repo.path`
- `stale_retention_on_memory_rejected`: `backend = "memory"` + `stale_retention = "168h"` → err naming `cache_repo.stale_retention`
- `max_entries_on_memory_rejected`: `backend = "memory"` + `max_entries = 100` → err naming `cache_repo.max_entries`
- `sweep_interval_on_memory_rejected`: `backend = "memory"` + `sweep_interval = "30m"` → err naming `cache_repo.sweep_interval`
- `max_capacity_on_redb_rejected`: redb TOML (with `cache_size`) + `max_capacity = 5000` → err naming `cache_repo.max_capacity`
- `omitted_stale_retention_stays_none_on_memory`: `backend = "memory"` + `max_capacity = 5000` only → `configure_context` succeeds (omitted field does not trip rejection)
- `omitted_stale_retention_falls_back_in_wiring_for_redb` (in-file, reuses task 1.5's factory): `CacheRepoConfig` redb with tempdir path + `cache_size = Some("256MiB")`, no `stale_retention` → factory repo has `stale_retention() == Duration::from_secs(7 * 24 * 3600)` (the dead-config-policy scenario, asserted on the concrete repository)
- Existing memory `max_capacity` test still passes

**Acceptance:**
- `cargo test -p camel-config` exits 0
- `cargo clippy -p camel-config -- -D warnings` exits 0
- `grep -n 'Some("168h"' crates/camel-config/src/config.rs` returns nothing

- [x] 2.1
