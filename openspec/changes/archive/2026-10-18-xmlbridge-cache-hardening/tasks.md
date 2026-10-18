# Tasks: xmlbridge-cache-hardening

## Phase 1: Bounded pin-aware XML schema cache (rc-mac0j, Java)

Goal: the compiled-schema cache in `bridges/xml` is bounded by a configurable entry limit,
registered schema-set digests are pinned against eviction, inline entries evict
oldest-inserted-first, and evictions are logged and counted. Matches design.md Phase 1
exit-criteria. No proto/IDL change; Java-only.

### bridges/xml — SchemaCache core

#### Task 1.1: Bounded pin-aware SchemaCache with eviction counter

**Files:**
- `bridges/xml/src/main/java/org/rustcamel/xmlbridge/SchemaCache.java` (modified)
- `bridges/xml/src/test/java/org/rustcamel/xmlbridge/SchemaCacheBoundTest.java` (new)
- `bridges/xml/src/test/java/org/rustcamel/xmlbridge/XsdValidationIntegrationTest.java` (modified — one-line compile fix only)

**Steps:**
1. Add a Quarkus-configured constructor to `SchemaCache`: `@Inject SchemaCache(@ConfigProperty(name = "xml-bridge.schema-cache.max-entries", defaultValue = "256") int maxEntries)`. Keep the field `private final int maxEntries`. In the constructor, throw `IllegalArgumentException` with a message naming the property — `"xml-bridge.schema-cache.max-entries must be positive, got " + maxEntries` — when `maxEntries <= 0` (ArC surfaces this as a startup failure — fail-loud, no silent unbounded fallback).
2. Update the existing test at `XsdValidationIntegrationTest.java:855` from `new SchemaCache()` to `new SchemaCache(256)` — the new int constructor becomes the only constructor, so this one-line fix keeps the existing suite compiling.
3. Add cache bookkeeping fields, all mutated only under the `SchemaCache` instance monitor: `private final ArrayDeque<String> unpinnedOrder = new ArrayDeque<>();`, `private final Set<String> pinned = ConcurrentHashMap.newKeySet();`, `private long evictions = 0;`. Keep `byDigest` (`ConcurrentHashMap<String, Schema>`) as-is.
4. Add `public synchronized void pin(String digest)` — adds `digest` to `pinned` and removes it from `unpinnedOrder` if present. Add `public synchronized void unpin(String digest)` — removes from `pinned` and, if `byDigest` still contains the digest, appends it to the tail of `unpinnedOrder` (oldest-inserted-first eviction order is preserved because unpin is rare and appends at the tail).
5. Change the body of `getOrCompile` (already `synchronized`): after `byDigest.put(digest, compiled)`, if `pinned` contains `digest` do NOT add to `unpinnedOrder`, else append `digest` to `unpinnedOrder`; then call the new private `evictUntilBounded(String justInserted)`.
6. Route the existing `public void put(String digest, Schema schema)` through the same bookkeeping: make it `synchronized`; if `byDigest` already contains the key, overwrite the value without enqueueing or evicting; otherwise enqueue per the pin rule and call `evictUntilBounded(digest)`.
7. Add `private void evictUntilBounded(String justInserted)`: `while (byDigest.size() > maxEntries)` poll `unpinnedOrder`; when the polled head equals `justInserted`, break immediately (it is the newest entry — no older candidate exists, and polling past it would lose it from the queue); also skip polled digests that are no longer in `byDigest` or are now in `pinned`; when a real victim is found, `byDigest.remove(victim)`, `evictions++`, and emit one log line at info level naming the evicted digest and the new size, using `org.jboss.logging.Logger` (Quarkus standard; note the bridge main sources have no existing logger convention — `BenignParserWarningLogger` extends Saxon's `StandardLogger`, so introduce the JBoss Logger import here). If the queue empties or only `justInserted` remains, break (transient pinned overshoot — documented in design.md).
8. Add `public synchronized long evictions()` returning the counter. Keep `size()`, `get()`, `remove()` semantics; make `remove` also drop the digest from `unpinnedOrder` when present.
9. Create `SchemaCacheBoundTest` (plain JUnit, no Quarkus) with the tests listed below. Compile tiny real schemas by passing a minimal in-memory XSD string to `SchemaSetCompiler.compileLegacy(byte[] schemaBytes)` (or fall back to `javax.xml.validation.SchemaFactory`) so cached values are genuine `Schema` instances.

**Tests:**
- `inlineInsertsBeyondBoundStayBounded`: setup `new SchemaCache(4)` and a `SchemaCompiler` lambda that compiles a distinct minimal XSD per digest → `getOrCompile(digest, compiler)` for 6 distinct digests → assert `size() <= 4` and `evictions() >= 2`.
- `pinnedDigestSurvivesEvictionPressure`: setup `new SchemaCache(2)`, `pin("A")`, `getOrCompile("A", compiler)` → `getOrCompile` for 3 further distinct digests under pressure → assert `get("A") != null` after all inserts and `size() <= 2`.
- `unpinnedAfterUnpinBecomesEvictable`: setup bound 2, `pin("A")`, compile A plus one other → `unpin("A")` → `getOrCompile` for 2 more distinct digests → assert `get("A") == null` and `size() <= 2`.
- `inlineInsertDoesNotDowngradePin`: setup bound 2, `pin("A")`, call `getOrCompile("A", compiler)` twice (second call simulates the inline path inserting the same digest) → `getOrCompile` for 2 more distinct digests → assert `get("A") != null`.
- `nonPositiveBoundRejected` (proxy for the ArC startup failure: with `@ConfigProperty` constructor injection, the thrown `IllegalArgumentException` during bean creation fails context startup, which this direct-construction test exercises without booting Quarkus): `assertThrows(IllegalArgumentException.class, () -> new SchemaCache(0))` and the same for `-1`; assert the exception message contains `xml-bridge.schema-cache.max-entries` (spec scenario: the startup failure names the invalid setting).
- `evictionCounterIncreasesMonotonically`: setup bound 1 → compile digest X, record `evictions()` → compile digest Y → assert `evictions()` increased and `size() == 1`.
- `justInsertedHeadBreakKeepsQueueIntact`: setup bound 1, `pin("A")`, `getOrCompile("A", compiler)`, then `getOrCompile` for B and C → `unpin("A")` → `getOrCompile` for D → assert `size() == 1`, `get("B") == null`, `get("A") == null` (guards against the polled-`justInserted` entry being dropped from the eviction queue, which would strand unpinned entries above the bound permanently).
- `removeDropsDigestFromEvictionQueue`: setup bound 4 → compile X → `remove(X)` → compile 3 others → assert `size() == 3` (no stale queue entry distorts eviction).
- Command (run from the worktree `/home/shared/rust-camel-worktrees/367-xmlbridge-cache`, never the main checkout): `cd bridges/xml && GRADLE_USER_HOME=/home/shared/tmp/367/gradle-home ./gradlew --no-daemon test --tests 'org.rustcamel.xmlbridge.SchemaCacheBoundTest'`
- Expected before implementation exists: test compilation fails (`pin`/`unpin`/`evictions`/test ctor missing) — red. After implementation: all 8 pass — green.

**Acceptance:**
- The 8 new tests pass under the command above.
- Full existing suite still passes: from the worktree root, `cd bridges/xml && GRADLE_USER_HOME=/home/shared/tmp/367/gradle-home ./gradlew --no-daemon test` exits 0.
- The eviction log line from `evictUntilBounded` is visible in the Gradle test stdout when `inlineInsertsBeyondBoundStayBounded` runs (manual verification in the task report).
- No `byDigest` read path changed for `get()` (hot path: plain `ConcurrentHashMap.get`, no lock).
- No placeholder text of any kind; no open work items left in comments.

- [x] 1.1

### bridges/xml — service wiring + integration proof

#### Task 1.2: Pin registered digests in XsdValidatorService + bounded inline path

**Files:**
- `bridges/xml/src/main/java/org/rustcamel/xmlbridge/XsdValidatorService.java` (modified)
- `bridges/xml/src/main/resources/application.yml` (modified)
- `bridges/xml/src/test/java/org/rustcamel/xmlbridge/XsdValidationIntegrationTest.java` (modified)

**Steps:**
1. In `registerSchemaSet`, in BOTH branches (legacy single-blob and decoded set), inside the `synchronized (registryLock)` block after `schemaHashById.put(schemaId, digest)`, call `schemaCache.pin(digest)`. Lock order stays `registryLock` → `SchemaCache` monitor (never the inverse) — matches design.md.
2. In `evictIfUnreferenced(previous, current)`: keep the `!schemaHashById.containsValue(previous)` guard; when true call `schemaCache.unpin(previous)` followed by the existing `schemaCache.remove(previous)` (eager removal of an unreferenced digest is today's behaviour and stays).
3. In `unregisterSchema`, in the `!schemaHashById.containsValue(digest)` branch, call `schemaCache.unpin(digest)` before the existing `schemaCache.remove(digest)`.
4. Verify the inline path (`getOrCompileSchema`) needs NO pin call: it inserts unpinned via `getOrCompile`/`put`, exactly the bounded path from task 1.1. Do not add pinning there.
5. In `application.yml`, add the explicit config key with a comment: under a new top-level `xml-bridge:` key, `schema-cache: max-entries: 256` (comment: bound on compiled schemas; registered schema-set entries are pinned; non-positive fails startup). In the existing `"%test"` profile block, override to `xml-bridge: schema-cache: max-entries: 8` so integration tests exercise eviction with small inserts.
6. Extend `XsdValidationIntegrationTest` with the tests below. Inject the cache bean directly with `@Inject SchemaCache schemaCache;` (same bean the service uses) to assert sizes.

**Tests:**
- `inlineValidateBeyondBoundKeepsCacheBounded`: setup `@QuarkusTest` context with `%test` bound 8. The cache bean and `schemaHashById` are singletons shared by every test in the file and earlier tests may have registered (and thus pinned) digests, so first capture baselines: `int sizeBase = schemaCache.size(); long evBase = schemaCache.evictions();` → build 10 `ValidateRequest` messages, each carrying a distinct minimal inline XSD (unique element name) plus a matching document, and call `xsd.validate(request).await().atMost(Duration.ofSeconds(10))` for each via the gRPC stub → assert each response `getValid()` is true (no validation broke), `schemaCache.size() <= sizeBase + 8`, and `schemaCache.evictions() >= evBase + 2`.
- `registeredSchemaSetNotEvictedByInlinePressure`: setup bound 8, capture `sizeBase` → `registerSchema` a valid set under id `pinned-set-<unique>` → run 10 distinct inline validates → assert `xsd.validateWith(ValidateWithRequest with that schema_id)` returns `getValid()` true for a matching document and `schemaCache.size() <= sizeBase + 1 + 8` (pre-existing entries + the pinned set + at most 8 unpinned).
- `unregisterAllowsBoundRecovery`: setup bound 8, capture `sizeBase`, register `temp-set-<unique>` → `unregisterSchema` it → run 10 distinct inline validates → assert `schemaCache.size() <= sizeBase + 8` and a `validateWith` for the unregistered id returns `RESOURCE_NOT_FOUND`.
- Command (from the worktree root): `cd bridges/xml && GRADLE_USER_HOME=/home/shared/tmp/367/gradle-home ./gradlew --no-daemon test --tests 'org.rustcamel.xmlbridge.XsdValidationIntegrationTest'`
- Expected before this task's wiring exists: the three new tests fail (no bound in `%test`, no pinning — cache exceeds bound / pinned entry evicted) — red. After: all pass and the pre-existing tests in the file still pass — green.

**Acceptance:**
- The 3 new tests pass and `XsdValidationIntegrationTest` as a whole passes under the command above.
- Full suite green: from the worktree root, `cd bridges/xml && GRADLE_USER_HOME=/home/shared/tmp/367/gradle-home ./gradlew --no-daemon test` exits 0.
- `registryLock` → cache-monitor lock order preserved; no code path takes the cache monitor and then `registryLock`.
- No proto/IDL file changed (`git status` shows no `*.proto` modifications).

- [x] 1.2

## Phase 2: Actionable unwritable-cache-dir error (rc-u68ap, Rust)

Goal: shared bridge binary resolution in `camel_bridge::download` fails with an actionable
`BridgeError::CacheUnwritable` when the cache dir cannot be written (probe before any
network work), while verified cache hits and writable-dir misses behave exactly as before.
Matches design.md Phase 2 exit-criteria. Offline-testable; no Docker.

### camel-bridge — error variant + writability probe

#### Task 2.1: BridgeError::CacheUnwritable variant and ensure_cache_writable helper

**Files:**
- `crates/services/camel-bridge/src/process.rs` (modified)
- `crates/services/camel-bridge/src/download.rs` (modified)

**Steps:**
1. In `process.rs`, add one additive variant to `pub enum BridgeError` (thiserror style, matching neighbours): `#[error("bridge cache directory {path} is not writable: {source}; the directory may be owned by another user (e.g. left by a Docker run as root) or on a read-only filesystem — repair its ownership/permissions, or bypass the cache via CAMEL_<BRIDGE>_BINARY_PATH or the component's bridge_cache_dir")] CacheUnwritable { path: std::path::PathBuf, source: std::io::Error }`. Do not touch existing variants.
2. In `download.rs`, add `pub(crate) fn ensure_cache_writable(cache_dir: &Path, version: &str) -> Result<(), BridgeError>`: run `std::fs::create_dir_all(cache_dir.join(version))`, then create a probe file named `format!(".write-probe-{}", std::process::id())` inside `cache_dir.join(version)`, write one byte, delete it. On ANY `std::io::Error` from any of these steps return `Err(BridgeError::CacheUnwritable { path: cache_dir.to_path_buf(), source })` (covers EACCES, EROFS, ENOSPC per blessed design). Delete the probe file even on the success path (best-effort; an orphan probe file is harmless).
3. Add the unit tests below to the existing `#[cfg(test)] mod tests` in `download.rs`, using the `tempfile` dependency (already in `Cargo.toml`). Guard each permission test: first verify a write into the directory actually fails; if it unexpectedly succeeds (running as root), return early — the test then asserts nothing about permissions.
4. Run `pgrep -c compiled_artifa` before and after the test battery; must be 0 both times.

**Tests:**
- `ensure_cache_writable_succeeds_on_writable_dir`: setup `tempfile::TempDir` (respects `TMPDIR`) → call `ensure_cache_writable(dir.path(), "0.1.0")` → assert `Ok(())` and no probe file remains in `dir.path().join("0.1.0")`.
- `ensure_cache_writable_maps_denied_dir_to_cache_unwritable`: setup `TempDir`, create subdir, `set_permissions` mode `0o555` via `std::os::unix::fs::PermissionsExt` → call `ensure_cache_writable(subdir, "0.1.0")` → assert `Err(BridgeError::CacheUnwritable { path, .. })` with `path == subdir` and the Display string contains the path, the phrase `not writable`, and the remedy token `repair`. Restore `0o755` after the assertion so `TempDir` cleanup works.
- Command: `systemd-run --user --scope --collect --unit=fleet-367-1 -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp/367 CARGO_BUILD_JOBS=6 cargo test -p camel-bridge -j4 ensure_cache_writable`
- Expected before implementation exists: compile error (variant/helper missing) — red. After: both pass — green.

**Acceptance:**
- Both tests pass under the command above (run from the worktree root, never the main checkout).
- `cargo fmt --check -p camel-bridge` exits 0.
- Command: `systemd-run --user --scope --collect --unit=fleet-367-1 -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp/367 CARGO_BUILD_JOBS=6 cargo clippy -p camel-bridge --all-targets -j4 -- -D warnings` (containment wrapper, run from the worktree root; `-j4` must sit BEFORE `--` — post-`--` args go to clippy-driver, which rejects `-j`) exits 0.
- Existing `BridgeError` variants and their tests unchanged (`git diff` shows only the additive variant).

- [x] 2.1

### camel-bridge — wire probe into resolution + regression proof

#### Task 2.2: Probe cache dir on miss in ensure_binary_for_spec

**Files:**
- `crates/services/camel-bridge/src/download.rs` (modified)

**Steps:**
1. In `ensure_binary_for_spec`, after the verified-cache-hit early return (the `bin_path.exists() && hash_path.exists()` block) and BEFORE the `let base_url = release_base_url(spec, version)?;` line, insert the probe: `ensure_cache_writable(cache_dir, version)?;`. This is the only insertion — no reordering of the env override, workspace-build, or cache-hit checks. Also DELETE the now-stale `#[allow(dead_code)]` attribute on `ensure_cache_writable` (added transitionally in task 2.1 because the helper was unused until this wiring).
2. Confirm by reading the final function order that: env override → workspace build → verified hit → probe → URL/checksum fetch → `create_dir_all`/unpack. The later `create_dir_all` at unpack time stays (it is now known-writable; mapping there can remain `BridgeError::Io`).
3. Add the regression tests below to `mod tests` in `download.rs`. For the cache-hit test, populate a valid hit by hand: write a dummy binary file at the path built from `tarball_filename(&XML_BRIDGE, version)` (strip `.tar.gz`), `join("bin").join("xml-bridge")`, and write `sha256_hex(&bin_bytes)` to `cache_dir.join(version).join(".binary.sha256")` (both helpers are in this module's scope).
4. For ALL THREE tests (hit test included — `ensure_binary_for_spec` checks the env override and the workspace native build BEFORE the cache hit, at `download.rs:71-108`), neutralize ambient short-circuits in setup: `unsafe { std::env::remove_var("CAMEL_XML_BRIDGE_BINARY_PATH") }` (workspace edition is 2024, so env mutation is an `unsafe` fn — wrap per repo convention, e.g. `camel-bench/src/lib.rs:107`; same-value concurrent writes make flake risk negligible, no serial harness needed), and guard on the workspace build — if `<workspace>/bridges/xml/build/native/xml-bridge` exists (find the workspace root the same way `find_workspace_root` does), skip the test's assertions (return early) since resolution would return before reaching the code under test. Then set, also inside `unsafe`, the release-URL override env var `CAMEL_XML_BRIDGE_RELEASE_URL=http://127.0.0.1:9` (miss tests only; the hit test never reaches the URL) so no real endpoint is contacted: `resolve_release_base_url` allowlists only https/github.com URLs and rejects the http loopback override with `BridgeError::UrlNotAllowed` before any network I/O. The miss-test assertions tolerate ANY non-`CacheUnwritable` error kind from the fetch, so concurrent env mutation by other tests cannot flake the assertion.
5. Run `pgrep -c compiled_artifa` before and after the battery; must be 0 both times.

**Tests:**
- `unwritable_cache_dir_yields_cache_unwritable_before_download`: setup `TempDir`, subdir mode `0o555`, empty cache (no binary), same root-skip guard as task 2.1 → call `ensure_binary_for_spec(&XML_BRIDGE, "0.0.0-test", subdir).await` → assert the error is `BridgeError::CacheUnwritable` whose `path` is the subdir, i.e. failure occurs at the probe before any fetch. (Use `#[tokio::test]`, matching existing async tests in this crate.)
- `verified_cache_hit_succeeds_without_write_probe`: setup `TempDir` with a valid populated hit as in step 3, then set the cache dir itself to mode `0o555` → call `ensure_binary_for_spec(&XML_BRIDGE, "0.0.0-test", dir).await` → assert `Ok(bin_path)` equal to the pre-populated path (proves the hit path returns before the probe and successful behaviour is unchanged). Restore `0o755` for `TempDir` cleanup.
- `writable_cache_miss_does_not_report_cache_unwritable`: setup empty writable `TempDir`, env release override to `http://127.0.0.1:9` → call `ensure_binary_for_spec(&XML_BRIDGE, "0.0.0-test", dir).await` → assert the call errors (fetch refused) and the error does NOT match `BridgeError::CacheUnwritable { .. }`.
- Command (unit battery with the three new tests): `systemd-run --user --scope --collect --unit=fleet-367-2 -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp/367 CARGO_BUILD_JOBS=6 cargo test -p camel-bridge -j4` followed by the integration test: `systemd-run --user --scope --collect --unit=fleet-367-3 -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp/367 CARGO_BUILD_JOBS=6 cargo test -p camel-bridge -j4 --test subprocess_startup_health_pin_regression`.
- Expected before this task's wiring exists: `unwritable_cache_dir_yields_cache_unwritable_before_download` fails (with the release-URL override in place the pre-wiring error is `BridgeError::UrlNotAllowed`, not `CacheUnwritable`) — red. After: all three pass — green.

**Acceptance:**
- The 3 new tests pass; full `cargo test -p camel-bridge -j4` battery passes (both unit `mod tests` and `tests/subprocess_startup_health_pin_regression.rs` — enumerate BOTH, no `--lib` shortcut).
- Verified order in `ensure_binary_for_spec`: env → workspace → hit → probe → fetch (cite the line numbers in the task report).
- `cargo fmt --check -p camel-bridge` exits 0 and `systemd-run --user --scope --collect --unit=fleet-367-4 -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp/367 CARGO_BUILD_JOBS=6 cargo clippy -p camel-bridge --all-targets -j4 -- -D warnings` exits 0 (`-j4` before `--`, per the clippy-driver constraint noted in task 2.1).
- No network contacted outside loopback in tests (release URL override points at `127.0.0.1:9`).

- [x] 2.2
