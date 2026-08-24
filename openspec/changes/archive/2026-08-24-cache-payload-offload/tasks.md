# Tasks: cache-payload-offload

## camel-api

### Task 1.1: CacheEntry.payload_path additive field + struct-literal migration

**Files:**
- `crates/camel-api/src/cache.rs` (modified)
- `crates/camel-processor/src/cache_eip.rs` (modified — ~17 literals)
- `crates/camel-core/src/cache/redb.rs` (modified — ~5 literals)
- `crates/camel-core/src/cache/memory.rs` (modified — ~1 literal)
- `crates/services/camel-redis-repo/src/cache_repo.rs` (modified — ~2 literals)
- `crates/camel-config/tests/cache_repo_config.rs` (modified — ~1 literal)
- `crates/camel-test/tests/integration_test.rs` (modified — ~2 literals)
- `crates/camel-test/tests/redis_repositories_test.rs` (modified — ~2 literals)

**Steps:**
1. In `crates/camel-api/src/cache.rs`, add to `CacheEntry` immediately after `bytes`:
   `/// Relative blob filename when the payload is offloaded to disk; `None` = bytes live inline.`
   `#[serde(default)]`
   `pub payload_path: Option<String>,`
   Keep existing derives (`Debug, Clone, PartialEq, Serialize, Deserialize`) unchanged.
2. Fix every `CacheEntry { .. }` struct literal found by
   `rg -n 'CacheEntry\s*\{' --glob '*.rs'` by adding `payload_path: None,`
   (mechanical; no behavior change). Do NOT touch `crates/services/camel-auth/src/authn_cache.rs`
   — it defines its own local `CacheEntry`.
3. Verify no site was missed: `rg -n 'CacheEntry\s*\{' --glob '*.rs'` must
   show no struct literal WITHOUT `payload_path` (the authn_cache local
   struct is the sole exemption); if an unlisted file appears, apply the same
   mechanical edit and record the extra path in the task result.
3. Add unit tests in `crates/camel-api/src/cache.rs` `mod tests` (create the module
   if absent) per the Tests block below.

**Tests:** (executable spec)
- `legacy_json_without_payload_path_deserializes_as_none`: setup = JSON string
  `{"bytes":[1,2,3],"content_type":"Bytes","expires_at":null}`; action =
  `serde_json::from_str::<CacheEntry>(&json)`; assert = `Ok` with
  `payload_path == None` and `bytes == vec![1,2,3]`.
- `payload_path_round_trips_through_serde`: setup = `CacheEntry` with
  `payload_path: Some("abc.blob".into())`; action = `serde_json::to_string` then
  `from_str`; assert = `payload_path == Some("abc.blob")` and field present in
  the JSON string.
- Command: `cargo test -p camel-api --lib` — expected: pass after step 1-2
  (fails to compile before them).

**Acceptance:**
- `cargo build --workspace` exits 0.
- `cargo fmt --check --all` exits 0.
- `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings` exits 0.
- `cargo test -p camel-api --lib` passes.

- [x] 1.1

## camel-config

### Task 2.1: payload config fields + fail-closed matrix + env interpolation

**Files:**
- `crates/camel-config/src/config.rs` (modified)
- `crates/camel-config/tests/cache_repo_config.rs` (modified)

**Steps:**
1. In `CacheRepoConfig` (config.rs ~line 659+), add four fields mirroring the
   existing `sweep_interval: Option<String>` string-then-parse style:
   - `#[serde(default)] pub payload: Option<PayloadMode>` where
     `#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]`
     `#[serde(rename_all = "lowercase")] pub enum PayloadMode { Inline, Disk }`
     (new public enum next to the config struct),
   - `pub payload_dir: Option<String>`,
   - `pub payload_sweep_interval: Option<String>`,
   - `pub payload_max_ttl: Option<String>`.
2. In the validation site (the same function family that enforces the
   per-field-per-backend matrix around config.rs:1704+ and :1897+), add rules
   failing with errors naming `cache_repo.<field>`:
   - `payload = "disk"` requires non-empty `payload_dir`;
   - `payload = "disk"` rejected when `backend = "memory"`;
   - `payload_dir`, `payload_sweep_interval`, `payload_max_ttl` rejected when
     `payload` is inline/unset, and when `backend = "memory"` (even if payload
     is unset);
   - `payload` value other than `inline`/`disk` rejected at deserialization/
     validation naming `cache_repo.payload`;
   - `payload_sweep_interval`/`payload_max_ttl` must parse via humantime and be
     non-zero (mirror existing `sweep_interval` zero/malformed handling).
3. `payload_dir` inherits `${env:}` strict interpolation automatically
   (`cache_repo` is in STRICT_PREFIXES) — no new interpolation code.
4. Add tests per the Tests block to `crates/camel-config/tests/cache_repo_config.rs`.

**Tests:** (executable spec — all follow the existing matrix-test style in that file)
- `disk_payload_without_payload_dir_rejected`: setup = valid redis url config with
  `payload = "disk"`, no `payload_dir`; action = validate; assert = Err naming
  `cache_repo.payload_dir`.
- `memory_backend_rejects_disk_payload`: memory + `payload = "disk"` → Err naming
  `cache_repo.payload`.
- `payload_fields_rejected_under_inline`: redis config, payload unset, one of
  `payload_dir`/`payload_sweep_interval`/`payload_max_ttl` set → Err naming that
  `cache_repo.<field>` (three assertions or a loop over the three fields).
- `payload_fields_rejected_under_memory`: memory backend + each of the three
  fields set → Err naming the field.
- `malformed_payload_value_rejected`: `payload = "spool"` → Err naming
  `cache_repo.payload`.
- `malformed_payload_sweep_interval_rejected`: disk + `"thirty"` → Err naming
  `cache_repo.payload_sweep_interval`.
- `zero_payload_sweep_interval_rejected`: disk + `"0s"` → Err naming the field.
- `malformed_payload_max_ttl_rejected`: disk + `"forever"` → Err naming
  `cache_repo.payload_max_ttl`.
- `zero_payload_max_ttl_rejected`: disk + `"0s"` → Err naming the field.
- `payload_dir_env_placeholder_resolves`: config string with
  `payload_dir = "${env:CACHE_PAYLOAD_DIR}"` + env var set to a temp dir
  (mirror the existing placeholder tests in the config suite) → interpolation
  resolves and validation passes.
- Command: `cargo test -p camel-config --test cache_repo_config` — expected:
  fail before step 2, pass after.

**Acceptance:**
- `cargo test -p camel-config` passes (full crate).
- `cargo clippy -p camel-config -- -D warnings` exits 0.
- `cargo xtask schema --check` exits 0 (schema derives pick up new fields).

- [x] 2.1

## camel-core

### Task 3.1: DiskOffloadRepository — struct, filename helpers, set + get/peek_stale re-injection

**Files:**
- `crates/camel-core/src/cache/disk_offload.rs` (new)
- `crates/camel-core/src/cache/mod.rs` (modified — add `pub mod disk_offload;` +
  `pub use disk_offload::DiskOffloadRepository;`)
- `crates/camel-core/Cargo.toml` (modified — add `blake3.workspace = true`
  under `[dependencies]` and `filetime = "0.2"` under `[dev-dependencies]`)

**Steps:**
1. Define in `disk_offload.rs`:
   `pub type OffloadClock = Arc<dyn Fn() -> std::time::SystemTime + Send + Sync>;`
   and `fn default_offload_clock() -> OffloadClock` returning `SystemTime::now`
   closure (mirrors camel-redis-repo `ClockFn` at cache_repo.rs:25-29).
2. Define `pub struct DiskOffloadRepository { inner: Arc<dyn CacheRepository>,
   dir: PathBuf, stale_retention: Duration, sweep_interval: Duration,
   payload_max_ttl: Duration, clock: OffloadClock,
   sweep_handle: Mutex<Option<tokio::task::JoinHandle<()>>> }` implementing
   `#[async_trait] CacheRepository`. Constructors:
   `pub fn new(inner: Arc<dyn CacheRepository>, dir: PathBuf,
   stale_retention: Duration, sweep_interval: Duration,
   payload_max_ttl: Duration,
   shutdown_token: CancellationToken) -> Self` (default clock; spawns the
   sweeper introduced in Task 3.3 — until that task lands, leave the
   `sweep_handle` field as `Mutex::new(None)` with the comment
   "sweeper attached in 3.3"; the Drop impl arrives with 3.3) and
   `pub fn with_clock(inner: Arc<dyn CacheRepository>, dir: PathBuf,
   stale_retention: Duration, sweep_interval: Duration,
   payload_max_ttl: Duration, shutdown_token: CancellationToken,
   clock: OffloadClock) -> Self` for tests.
   Delete-path methods (`invalidate`/`invalidate_prefix`/`clear`/`stats`/`name`)
   are implemented in Task 3.2 — for THIS task implement them as plain
   delegations to `inner` (they are correct as delegations; 3.2 only adds
   `clear`'s dir-unlink + tests).
3. Filename helpers (module-private, unit-tested):
   `fn content_fingerprint(entry: &CacheEntry) -> String` — blake3 128-bit hex
   of `entry.bytes` followed by the one-byte `u8` discriminant of
   `entry.content_type` obtained from an exhaustive `match` over the closed
   `ContentType` enum {Bytes, Text, Json, Xml} (domain separation);
   `fn blob_filename(key: &str, death_epoch: u64, entry: &CacheEntry) -> String`
   → format `{blake3-128hex(key)}.{death_epoch}.{content_fingerprint}.blob`;
   `fn parse_death_epoch(file_name: &str) -> Option<u64>` — second dot-separated
   component parsed as u64, `None` otherwise.
4. `set(key, entry, ttl)` — compute `effective_ttl = ttl.unwrap_or(payload_max_ttl)`
   and `effective_expires_at = clock() + effective_ttl` (checked, saturating).
   Blob filename death epoch = `effective_expires_at + stale_retention +
   sweep_interval` as unix seconds (checked, saturating). Write the blob FIRST:
   create `dir` if missing; unique tmp name per attempt
   (`{dest}.{blake3-32hex(key || clock_nanos || attempt_counter)}.tmp`), open
   with `OpenOptions::new().write(true).create_new(true)`, on `AlreadyExists`
   retry with a fresh nonce (bounded, e.g. 8 attempts), write bytes,
   `sync_all()`, then `tokio::fs::rename(tmp, dest)`, then best-effort
   parent-dir fsync (open dir read-only + `sync_all`, ignore errors, warn-once).
   THEN store the index entry: strip `entry.bytes = Vec::new()`, set
   `entry.payload_path = Some(dest_file_name)`, and call
   `inner.set(key, entry, Some(effective_ttl))` — the ttl MUST be `Some`:
   all three inners overwrite `entry.expires_at` from `ttl`
   (redis cache_repo.rs:106, redb redb.rs:423, memory memory.rs:123), and
   passing `None` would wipe expiry. The inner recomputes `expires_at` from
   its own clock; the sub-second skew is absorbed by the death-epoch grace.
   On blob-write error (create/write/fsync/rename): WARN, then inline
   fallback — `inner.set(key, original_entry_unstripped, ttl)` — and return
   that result (the decorator never converts its own file-write failure into
   a new `Err`). Inner `Err` propagates unchanged.
5. `get`/`peek_stale`: call `inner.get`/`inner.peek_stale`; on `Some(entry)`:
   if `entry.payload_path` is `None` → return as-is (legacy/inline rows);
   else sanitize: the path must be a single file name (no `/`, no `\`, no
   `..`, not absolute, non-empty) — anything else is a corrupt row → WARN +
   `Ok(None)`. Join to `dir`, read the file: on `NotFound`/`NotADirectory`
   → WARN + `Ok(None)`; on `PermissionDenied` and other non-NotFound error
   kinds → `Err(CamelError::Io(format!(
   "cache payload blob read '{}': {e}", path.display())))` per Contract C1;
   on success →
   `entry.bytes = loaded`, `entry.payload_path = None`, return `Some(entry)`.
6. Add a small in-file tracing-capture helper for tests: a
   `tracing_subscriber` layer installed via `tracing::subscriber::with_default`
   that records WARN event messages into a `Arc<Mutex<Vec<String>>>`
   (`struct CaptureLayer`; the camel-core dev-deps already include
   tracing/tracing-subscriber — verify and add if absent).
7. Unit tests in `mod tests` inside `disk_offload.rs` using
   `MemoryCacheRepository` as `inner` + `tempfile::tempdir()` per Tests block.

**Tests:** (executable spec; fixed clock via `with_clock` where epoch math matters)
- `set_stores_blob_and_bytes_empty_index_entry`: tempdir D, inner=memory;
  action = `set("k", entry(50KiB, Bytes, None), Some(1h))`; assert = exactly
  one file in D matching `{blake3hex(k)}.{death}.{fp}.blob` with the payload
  bytes; `get("k")` returns the original bytes + content_type; and a second
  `DiskOffloadRepository` sharing D + same inner reads the same entry
  (round-trip through the decorated face is the required assertion; the raw
  inner row assertion is covered by `no_ttl` and fallback tests).
- `death_epoch_formula_uses_ttl_retention_and_grace`: fixed clock C,
  retention 168h, sweep 1h, `set("k", e, Some(2h))`; assert = filename's
  parsed epoch == C+2h+168h+1h as unix seconds.
- `no_ttl_fabricates_max_ttl_expiry`: max_ttl 24h, `set("k", e, None)`;
  assert = blob filename epoch == C+24h+retention+sweep AND the inner row's
  `expires_at` is `Some` within 2s of real-now+24h (inner recomputes from its
  own clock — tolerance assert, not exact).
- `blob_write_failure_falls_back_inline`: point `dir` at a path occupied by a
  regular file (so dir creation/first write fails); action =
  `set("k", e, Some(1h))`; assert = returns `Ok(())`, `get("k")` returns the
  original bytes, and a WARN was captured by the tracing helper.
- `file_dead_read_is_miss_with_warn_never_err`: set ok, delete the blob file;
  assert = `get` and `peek_stale` both `Ok(None)` and a WARN captured.
- `existing_blob_read_failure_surfaces_err`: set ok, `chmod 0o000` the blob
  file (`#[cfg(unix)]`); assert = `get("k")` returns `Err`.
- `traversal_payload_path_rejected_as_miss`: two inner rows injected directly
  (`inner.set("a", CacheEntry{ payload_path: Some("../../etc/passwd".into()), ..})`,
  same for `"b"` with `Some("/etc/passwd".into())`); assert = both
  `get` → `Ok(None)` with WARN, no file outside D opened.
- `legacy_row_without_payload_path_passes_through`: inner.set with
  `payload_path: None` + bytes; assert = decorator `get` returns those bytes.
- `peek_stale_reinjects_past_expiry`: set with ttl 10ms, sleep 20ms; assert =
  `peek_stale("k")` returns entry with original bytes AND `get("k")` is
  `Ok(None)` (memory inner enforces in-band expiry).
- `concurrent_same_key_different_payload_no_cross_pair`: fixed clock (same
  epoch second), `set("k", payloadA/Bytes, Some(1h))` then
  `set("k", payloadJ/Json, Some(1h))`; assert = D holds two blob files;
  `get("k")` returns payloadJ AND content_type Json (same writer).
- `filename_fingerprint_domain_separated`: same bytes under different
  content types via two keys → two distinct fingerprints (two files, both
  retrievable under their keys).
- Command: `cargo test -p camel-core --lib` — expected: fail before the task,
  pass after.

**Acceptance:**
- `cargo test -p camel-core --lib` passes.
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.
- No `unwrap()` in non-test code (`cargo xtask lint-unwrap` clean).

- [x] 3.1

### Task 3.2: DiskOffloadRepository — delete paths (clear unlink, delegation) + stats/name tests

**Files:**
- `crates/camel-core/src/cache/disk_offload.rs` (modified)

**Steps:**
1. Replace 3.1's plain `clear` delegation with: best-effort unlink every entry
   of `dir` (`tokio::fs::read_dir` + `remove_file`; per-file `NotFound` = ok;
   other errors WARN and continue), then `inner.clear()`. `clear` never
   returns `Err` for its own unlink failures.
2. Keep `invalidate`/`invalidate_prefix` as delegations with a doc comment:
   the returned count is index-scoped; blobs are reclaimed asynchronously at
   their filename-encoded death epoch.
3. Unit tests per the Tests block.

**Tests:**
- `invalidate_delegates_and_blob_survives_until_epoch`: set, `invalidate("k")`;
  assert = `get("k")` → `Ok(None)`; blob file still present in D.
- `invalidate_prefix_delegates_count_is_index_scoped`: two keys "ns:a"/"ns:b"
  + "other:c" set via decorator; `invalidate_prefix("ns:")` returns 2 and all
  three blob files remain on disk until their epochs.
- `clear_unlinks_dir_and_delegates`: two sets; `clear()`; assert = inner
  `stats().entries == 0` and D has no blob files.
- `clear_swallows_unlink_failures` (cfg(unix)): two sets; `chmod 0o555` D;
  `clear()`; assert = returns `Ok(())`, inner cleared (entries 0), WARN
  captured; restore `chmod 0o755` for tempdir cleanup.
- `stats_and_name_delegate`: inner = memory repo; assert = decorator
  `name()` == inner `name()`; after set+get, `stats().hits >= 1` flows from
  inner; `stats().bytes` equals inner's value unchanged.
- Command: `cargo test -p camel-core --lib` — pass after this task.

**Acceptance:**
- `cargo test -p camel-core --lib` passes (3.1 tests still green).
- `cargo clippy -p camel-core -- -D warnings` exits 0.

- [x] 3.2

### Task 3.3: payload sweeper task (death-epoch unlink, tmp GC, shutdown, Drop-abort)

**Files:**
- `crates/camel-core/src/cache/disk_offload.rs` (modified)

**Steps:**
1. Add module-private `async fn sweep_payload_dir(dir: &Path, now: SystemTime,
   sweep_interval: Duration) -> (u64, u64)` — scans `dir` once: unlinks files
   matching `*.blob` whose `parse_death_epoch` < `now` (unix secs), and files
   matching `*.tmp` whose modified time is older than `now - sweep_interval`;
   per-file `NotFound` (ENOENT race) counts as success; other errors WARN and
   continue. Returns (blobs_unlinked, tmps_unlinked). Also extract the
   per-file decision into `fn unlink_payload_file(path: &Path, now: SystemTime,
   sweep_interval: Duration) -> std::io::Result<bool>` so the ENOENT path is
   unit-testable in isolation.
2. Add `fn spawn_sweeper(dir: PathBuf, sweep_interval: Duration,
   shutdown_token: CancellationToken) -> tokio::task::JoinHandle<()>`
   mirroring the redb sweep loop (redb.rs:149-183): tokio task with
   `tokio::time::interval(sweep_interval)`, `tokio::select!` on
   `shutdown_token.cancelled()`, calls `sweep_payload_dir` with
   `SystemTime::now()`. Call it from both constructors (`new` and
   `with_clock`) — pass the real clock to the sweeper regardless of the
   injected decorator clock — store the handle in `sweep_handle`, and
   implement `Drop for DiskOffloadRepository` aborting the handle (mirror
   redb.rs:689-692). Remove the interim comment from 3.1.
3. Unit tests per the Tests block (synthetic `now`; mtime backdating via the
   `filetime` dev-dep added in 3.1).

**Tests:**
- `sweep_unlinks_dead_and_keeps_live_blobs`: D with `x.{past_epoch}.fp.blob`
  and `y.{future_epoch}.fp.blob` (crafted by writing plain files with those
  names); action = `sweep_payload_dir(D, now, 1h)`; assert = x gone, y
  present, returned count (1, 0).
- `unlink_payload_file_enoent_is_success`: call `unlink_payload_file` on a
  nonexistent `*.blob` path with a past epoch encoded in its name; assert =
  `Ok(false)` (vanished between listing and unlink — no error).
- `sweep_gcs_stale_tmp_by_age`: D with `a.tmp` (mtime backdated 2h via
  `filetime::set_file_mtime`) and `b.tmp` (fresh); action =
  `sweep_payload_dir(D, SystemTime::now(), 1h)`; assert = a.tmp gone, b.tmp
  present.
- `sweeper_task_stops_on_shutdown`: `spawn_sweeper` with 10ms interval +
  CancellationToken; action = `token.cancel()`; assert = handle completes
  within 2s (await with timeout — no hang).
- `blob_reclaimed_after_death`: decorator with fixed clock and NON-ZERO
  intervals (ttl 100ms, retention 0, sweep_interval 100ms — `tokio::time::interval`
  panics on zero, so zero is never a valid constructor value); set, then
  parse the blob filename's death epoch and call `sweep_payload_dir(D,
  epoch + 1s, 1h)` directly; assert = blob gone (orphan reclaim path for
  invalidate/evict in sweep form).
- Command: `cargo test -p camel-core --lib` — pass after this task.

**Acceptance:**
- `cargo test -p camel-core --lib` passes (3.1/3.2 tests still green).
- `cargo clippy -p camel-core -- -D warnings` exits 0.

- [x] 3.3

## camel-config (wiring)

### Task 4.1: context wiring — wrap backends, startup WARN, no sweeper inline

**Files:**
- `crates/camel-config/src/context_ext.rs` (modified)
- `crates/camel-config/src/config.rs` (modified — add
  `impl CacheRepoConfig { pub(crate) fn payload_durations(&self)
  -> (Duration, Duration) }` returning humantime-parsed
  `(payload_sweep_interval or 1h, payload_max_ttl or 720h)` unconditionally
  — defaults apply for unset fields; only the disk wiring arm consumes it)
- `crates/camel-config/tests/cache_repo_config.rs` (modified — add wiring tests)

**Steps:**
1. In the cache_repo registration arms (`context_ext.rs:468-485`), after
   building the redb or redis repo and BEFORE `register_cache_repository`,
   when the validated config has `payload = "disk"`:
   `let (sweep_interval, max_ttl) = ccfg.payload_durations();` then
   `let repo = DiskOffloadRepository::new(Arc::new(repo),
   payload_dir.clone().into(), stale_retention, sweep_interval, max_ttl,
   ctx.shutdown_token().clone())` and register the decorator under the SAME
   name the bare backend used ("persistent"/"redis"). The memory arm never
   wraps.
2. Emit one startup WARN naming the resolved `payload_dir` and stating that
   offloaded entries are unreadable by consumers that do not share the
   directory (tracing WARN; verified by lint-log-levels gate).
3. Wiring tests per the Tests block (redb-based — fully local, no infra),
   including a tracing-capture assert for the startup WARN (same
   CaptureLayer technique as 3.1, local copy in the test file).

**Tests:**
- `redb_disk_payload_wraps_repository_and_round_trips`: CamelConfig with
  `backend = "redb"`, tempdir `path`, valid `cache_size`, `payload = "disk"`,
  `payload_dir` = tempdir D; action = build context, resolve `"persistent"`
  repo, `set`/`get` round-trip; assert = exactly one `*.blob` file in D,
  `get` returns the original bytes, and the startup WARN naming D was
  captured.
- `inline_payload_builds_no_sweeper_and_no_dir`: config with
  `backend = "redb"` (tempdir), payload unset; action = build context, set
  via `"persistent"`; assert = no `*.blob` files appear in the would-be D
  (bytes stored inline) — D is never created.
- `disk_defaults_apply_when_intervals_unset`: config with payload=disk,
  intervals unset; action = build context (redb, tempdirs); assert = build
  succeeds and a set/round-trip works (defaults 1h/720h exercised through
  behavior; exact default values are covered by Task 2.1 validation tests).
- Command: `cargo test -p camel-config --test cache_repo_config` — pass after
  this task.

**Acceptance:**
- `cargo test -p camel-config` passes.
- `cargo clippy -p camel-config -- -D warnings` exits 0.
- `cargo xtask lint-log-levels` exits 0.
- `cargo xtask lint-component-deps` exits 0 (camel-core dep direction intact).

- [x] 4.1

## camel-test + docs

### Task 5.1: live redis offload integration suite

**Files:**
- `crates/camel-test/tests/cache_payload_offload_test.rs` (new)
- `.github/workflows/ci.yml` (modified — add a CI step running this suite
  with `--features integration-tests`, mirroring the existing
  redis_repositories step at ci.yml:104-114)

**Steps:**
1. Live integration test file mirroring
   `crates/camel-test/tests/redis_repositories_test.rs` conventions:
   `#![cfg(feature = "integration-tests")]` gate at the top (house pattern,
   redis_repositories_test.rs:30), testcontainers `redis:7-alpine`, per-test
   container, NO `#[ignore]` per ADR-0054.
2. Build the context CONFIG-DRIVEN (not manual wrap): write a
   `[default.cache_repo]` TOML with `backend = "redis"` (url mode, container
   port), `payload = "disk"`, `payload_dir` = tempdir D — extend the existing
   suite's `cache_repo_toml`/`CamelConfig::from_file` → `configure_context`
   helper pattern (redis_repositories_test.rs:73-99) — then resolve the
   `"redis"` repository and exercise it. This exercises the real
   `context_ext.rs` redis wiring arm, not a bypass.
3. Add the CI step per the Files entry.
4. The file reuses the dependencies the existing suites already declare
   (testcontainers behind the existing `integration-tests` optional feature,
   tempfile, tokio-util CancellationToken) — no `Cargo.toml` changes for
   `camel-test`. All sweeper-timing tests use config-expressible values
   (`stale_retention`, `payload_sweep_interval`, `payload_max_ttl` are all
   `[default.cache_repo]` fields), so every test goes through the
   config-driven context; no manual wrapping anywhere.

**Tests:**
- `redis_disk_offload_round_trip`: container redis; context built from
  config TOML (`backend = "redis"`, `payload = "disk"`, `payload_dir` = D);
  set 50KiB entry via the registered `"redis"` repository; assert = one blob
  in D; `get` returns identical bytes; the startup portability WARN naming D
  was captured; and a raw redis GET of the namespaced key (second
  connection, same pattern as the existing suite) returns an entry JSON with
  empty `bytes` array and `payload_path` set.
- `redis_disk_offload_early_sweep_is_miss`: set; delete blob file; assert =
  `get` and `peek_stale` → `Ok(None)`.
- `redis_disk_offload_sweeper_reclaims_orphan`: set with ttl 100ms +
  stale_retention 1ms + sweep 100ms (constructor values); sleep ~500ms;
  assert = blob gone from D (sweeper task reclaimed it).
- `redis_disk_offload_no_ttl_capped`: set with ttl None, payload_max_ttl
  200ms; sleep ~500ms; assert = `get` via inner → `Ok(None)` (redis EXAT
  expired) and blob swept from D.
- Command: `cargo test -p camel-test --features integration-tests --test
  cache_payload_offload` (requires Docker) — expected: pass after
  implementation (WITHOUT the feature flag the file cfgs out to zero tests —
  a vacuous pass; always run with the flag).

**Acceptance:**
- `cargo test -p camel-test --features integration-tests --test
  cache_payload_offload` passes locally with Docker up (if Docker is
  unavailable in the run environment, record
  `integration-verification-deferred-to-CI` — do not delete the suite).
- ci.yml carries the new step with `--features integration-tests`.
- `cargo fmt --check --all` exits 0.

- [x] 5.1

### Task 5.2: docs — ADR 0065, schema rows, CONTEXT-MAP entry

**Files:**
- `docs/adr/0065-cache-payload-offload.md` (new — next free number after 0064)
- `docs/src/configuration/schema.md` (modified — payload rows in the
  cache_repo tables)
- `CONTEXT-MAP.md` (modified — ADR index entry)

**Steps:**
1. Write ADR 0065 following the house ADR format (see docs/adr/0063 for
   structure): context (tile-proxy resilience cache, replicas>1, redb RWO
   file-lock vs redis full-dataset RAM); decision (decorator +
   index-in-backend/blob-on-disk; file-first tmp+fsync+rename with unique
   per-attempt tmp; self-die filenames `{blake3-128(key)}.{death_epoch}.{blake3-128(payload||content_type-discriminant)}.blob`;
   inline fallback on blob-write failure; MISS+WARN for file-dead vs Err for
   existing-blob I/O failures per Contract C1; sweeper with ENOENT=success
   and tmp GC; fail-closed config matrix incl. memory+disk rejection);
   consequences (rollback requires cache clear — old binaries serve empty
   bytes on offloaded rows; NFS caveats — local volume preferred, fsync
   durability is mount-dependent, `.tmp` leftovers reclaimed by the sweeper;
   stats report index-side accounting only — redb sums emptied `bytes`
   contributing 0, redis reports None; portability — offloaded entries are
   unreadable by consumers that do not share payload_dir).
2. Add the four field rows to the cache_repo tables in
   `docs/src/configuration/schema.md` (name, type, default, applicable
   backends, notes) matching the existing row style.
3. Register ADR 0065 in `CONTEXT-MAP.md` following the existing entry style
   (one line in the ADR index + domain-language additions if the house style
   requires them).

**Tests:**
- `docs_consistency` (non-Rust): action = review that schema.md rows match
  the four field names/types/defaults in `CacheRepoConfig` and that
  CONTEXT-MAP lists 0065; assert = no drift between docs and config struct
  (manual check recorded in the task result).
- Command: `cargo xtask lint-context-citations` — expected: exit 0 (ADR
  referenced correctly).

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- Docs written in English; ADR number 0065; schema.md carries the four rows.
- `cargo fmt --check --all` exits 0.

- [x] 5.2
