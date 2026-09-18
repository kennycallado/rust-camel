# Tasks: slimblockers

## Phase 1: camel-bundles bridge optionality (rc-9720m)

### Task 1.1: camel-bundles manifest — eight bridges optional behind http-static

**Files:**
- `crates/camel-bundles/Cargo.toml` (modified)

**Steps:**
1. In `[dependencies]`, mark each of the eight bridge deps optional, preserving
   the existing line order and `.workspace = true` inheritance:
   `camel-component-cxf`, `camel-component-jms`, `camel-component-opensearch`,
   `camel-component-redis`, `camel-component-sql`, `camel-component-ws`,
   `camel-xj`, `camel-xslt` become `{ workspace = true, optional = true }`.
2. Replace `http-static = []` with:
   `http-static = ["dep:camel-component-cxf", "dep:camel-component-jms", "dep:camel-component-opensearch", "dep:camel-component-redis", "dep:camel-component-sql", "dep:camel-component-ws", "dep:camel-xj", "dep:camel-xslt"]`
   prefixed by a comment block: `# Transitional legacy-bridge carrier (rc-9720m, mission 115): the eight bridge deps ride the existing http-static gate because lint-gate-forwarding Rule 2 requires boot consumers to forward every [features] key and camel-cli's dependency wiring is out of zone (design-time exclusion; the resume grant covers only the slim-benchmarks rename). Remove and split into per-bridge features when the next camel-cli mission adds the consumer forwards (it also renames the src cfg keys).`
3. Leave `default` EXACTLY as-is (`["grpc", "wasm", "http-static", "llm", "surrealdb", "mqtt", "mcp"]`) and add NO other feature key.
4. From the worktree root run and record outputs:
   - `cargo tree -p camel-bundles --no-default-features -e no-dev --prefix none --locked | grep -cE '^(camel-component-(jms|sql|redis|opensearch|ws|cxf)|camel-xj|camel-xslt) v'` → prints `1` (only the camel-config→camel-redis-repo redis path survives; verify with `-i camel-component-redis` that its sole dependent chain is camel-redis-repo ← camel-config).
   - `cargo tree -p camel-bundles --no-default-features --features http-static -e no-dev --prefix none --locked | grep -E '^(camel-component-(jms|sql|redis|opensearch|ws|cxf)|camel-xj|camel-xslt) v' | grep -v '(\*)$' | wc -l` → prints `8` (the trailing grep drops cargo-tree dedup marker lines ending in `(*)`; redis and xslt are dual-path crates and emit them).
   - `git diff --stat Cargo.lock` → empty (zero lockfile drift).

**Tests:** (resolve-level proofs — the compile-level tests land in 1.2)
- `slim_resolve_drops_bridges`: manifest edited per steps → `cargo tree -p camel-bundles --no-default-features` → seven of eight bridge crate names absent (redis present only via camel-redis-repo); command: the grep invocations in step 4; expected: pass after step 2.
- `http_static_restores_bridges`: `--features http-static` added → all eight present; command: step 4 second invocation; expected: pass after step 2.

**Acceptance:**
- The three step-4 commands print `1`, `8`, and empty respectively.
- `grep -cE '^(camel-component-(cxf|jms|opensearch|redis|sql|ws)|camel-xj|camel-xslt) = \{ workspace = true, optional = true \}$' crates/camel-bundles/Cargo.toml` prints `8`.
- `git diff Cargo.lock` empty.

- [x] 1.1

### Task 1.2: cfg-gated registration, BridgeCleanup, BootHandle + tests

**Files:**
- `crates/camel-bundles/src/lib.rs` (modified)

**Steps:**
1. Gate the bridge registrations inside `boot()` with `#[cfg(feature = "http-static")]`, one gate per site, each preceded by `// Transitional gate: rides http-static until per-bridge features land (rc-9720m).`:
   the `camel_xslt::XsltComponent` + `camel_xj::XjComponent` registration block and their `bridge_runtime()` captures; `register_bundle::<camel_component_ws::WsBundle>`; the `jms_pool` and `cxf_pool` `register_bundle_with` calls; `register_bundle::<camel_master::MasterBundle>` stays ungated (camel-master remains unconditional); `register_bundle::<camel_component_opensearch::OpenSearchBundle>`; `register_bundle::<camel_component_redis::RedisBundle>`; the `camel_component_sql::SqlBundle` match block.
2. Restructure `BridgeCleanup`: cfg-gate the `xslt` and `xj` fields (`#[cfg(feature = "http-static")] xslt: ...`), keep `validator` ungated; gate the matching cleanup arms in its `Lifecycle::stop`/cleanup impl identically so the struct compiles in both configurations; construct it conditionally at the `ctx.add_lifecycle` site (when both bridges are off, still register the lifecycle carrying `validator`).
3. Gate `BootHandle`'s `jms_pool` and `cxf_pool` fields and the corresponding `begin_shutdown` calls (teardown step 1) and JMS/CXF deadline-drain blocks (teardown step 3) inside `shutdown_with_deadline`; `ctx.stop()` (step 2) and `datasource_catalog.close_all()` (step 4) stay unconditional. Keep `datasource_catalog` field, `datasource_catalog()`, `shutdown()`, `shutdown_with_deadline()` signatures unchanged in both configurations. Keep the numbered teardown-order doc comment accurate by adding one sentence: pool steps 1 and 3 are cfg-conditional on http-static.
4. Gate the existing fixture tests that assert bridge registration (`boot_registers_all_bundles_from_fixture_config` and any test touching sql/jms/redis/opensearch/ws/cxf/xslt/xj assertions) with `#[cfg(feature = "http-static")]`, leaving the bridge-free tests (`boot_handle_exposes_datasource_catalog`, no-http fallback tests) ungated. Extend `boot_registers_all_bundles_from_fixture_config`'s scheme assertions to cover all eight bridges — it currently asserts only `ws` and `jms`; add `"xslt"`, `"xj"`, `"cxf"`, `"sql"`, `"redis"`, `"opensearch"` to the asserted scheme set (schemes confirmed in the component sources) so every gate has a runtime regression probe.
5. Add test `boot_slim_registers_core_without_bridges` gated `#[cfg(not(feature = "http-static"))]`: boots `tests/fixtures/no-http/Camel.toml` via `booted_context`-equivalent plumbing (inline a private helper if `booted_context` is itself gated), asserts `ctx` registers timer/log/direct/seda/mock/controlbus components (existing registry probing API used by the gated tests), asserts at least one bridge scheme is absent (`ctx` registry lookup for `"jms"` returns none), and that `boot_handle.shutdown(&mut ctx)` completes.

**Tests:**
- `boot_slim_registers_core_without_bridges`: camel-bundles built `--no-default-features` → boot the no-http fixture → core components present, shutdown drains Ok; command: `cargo test -p camel-bundles --no-default-features boot_slim_registers_core_without_bridges`; expected: pass after implementation (the `#[cfg(not(...))]` test cannot compile before gating lands).
- `boot_registers_all_bundles_from_fixture_config` (existing, now gated + extended): default features → bundles-present fixture → all eight bridges register (`ws`, `jms`, `xslt`, `xj`, `cxf`, `sql`, `redis`, `opensearch` schemes); command: `cargo test -p camel-bundles boot_registers_all_bundles_from_fixture_config`; expected: pass with the extended scheme set.

**Acceptance:**
- `cargo check -p camel-bundles` and `cargo check -p camel-bundles --no-default-features` exit 0.
- `cargo test -p camel-bundles` and `cargo test -p camel-bundles --no-default-features` exit 0.
- `cargo fmt --check` and `cargo clippy -p camel-bundles --all-targets -- -D warnings` and `cargo clippy -p camel-bundles --all-targets --no-default-features -- -D warnings` exit 0.

- [x] 1.2

### Task 1.3: Phase-1 gate run — golden pin, lint, compile matrix

**Files:**
- (no source changes; verification task producing recorded evidence in the commit message and park inputs)

**Steps:**
1. Run the feature_profiles suite three times:
   `for i in 1 2 3; do cargo test -p camel-cli --test feature_profiles; done` → all runs green, `default_closure_matches_golden` passes WITHOUT touching `tests/fixtures/default-deptree.txt` (verify `git status --short crates/camel-cli` stays empty).
2. `cargo xtask lint-gate-forwarding` → exit 0.
3. Compile matrix: `cargo check -p camel-bundles`, `cargo check -p camel-bundles --no-default-features`, `cargo check -p camel-bundles --all-features`, `cargo check -p camel-cli`, `cargo check -p camel-cli --no-default-features --features slim-http`, `cargo check -p camel-cli --all-features` → all exit 0 (camel-cli compiles untouched: http-static is on in default/full; slim never names the gated fields).
4. `cargo test -p camel-cli` (full suite) → green.

**Tests:**
- `feature_profiles_x3`: post-1.2 tree → three consecutive suite runs → 3× green including `default_closure_matches_golden` and `slim_closure_excludes_controllable_set`; command: step 1 loop; expected: pass.
- `gate_forwarding_clean`: step 2 command; expected: exit 0.
- `camel_cli_full_suite`: `cargo test -p camel-cli` → exit 0; expected: pass (camel-cli untouched by this phase — any failure means the gating leaked).

**Acceptance:**
- All commands in steps 1-4 exit 0; `git status --short crates/camel-cli Cargo.lock` empty.
- Commit phase 1 with subject `feat(bundles): optional bridge set behind http-static` — ONE atomic commit covering tasks 1.1+1.2 (1.1 alone leaves ungated bridge references that break featureless camel-bundles and camel-integration-test compiles).

- [x] 1.3

## Phase 2: camel-template minijinja optionality (rc-wcs3v)

### Task 2.1: camel-template optional engine + module gating

**Files:**
- `crates/components/camel-template/Cargo.toml` (modified)
- `crates/components/camel-template/src/lib.rs` (modified)
- `crates/components/camel-template/tests/engine_free.rs` (new)

**Steps:**
1. In `[dependencies]`: `camel-language-minijinja` and `minijinja` become `{ workspace = true, optional = true }`.
2. Replace the `[features]` insertion point (before `[lints]`) with exactly:
   ```
   [features]
   # All-or-nothing engine gate (rc-wcs3v, mission 115): dep: entries keep
   # default builds byte-identical (dep: activation renders no cargo-tree
   # feature lines) but set no named feature, so engine modules gate on
   # cfg(feature = "default"). The named lang-minijinja feature, the cfg-key
   # rename, and the in-workspace default-features flip ride the next
   # camel-cli mission (it owns the consumer manifests and the golden regen).
   default = ["dep:camel-language-minijinja", "dep:minijinja"]
   ```
   and NO other feature key.
3. In `src/lib.rs`: wrap the engine-coupled module declarations (`pub mod bundle`, `mod closure`, `pub(crate) mod component`, `pub(crate) mod endpoint`, `pub(crate) mod lifecycle`, `pub(crate) mod producer`, `pub(crate) mod reload`, `pub(crate) mod template_set`) in `#[cfg(feature = "default")]`; gate the matching `pub use` re-exports (`TemplateBundle`, `TemplateBundleConfig`, `TemplateComponent`); leave `pub mod config`, `pub mod error`, `pub(crate) mod path_util`, `pub(crate) mod uri` ungated (verified engine-free); extend the crate-level `//!` doc with one sentence stating the all-or-nothing engine gate (default features = full component; `--no-default-features` = engine-free crate); gate engine-coupled `#[cfg(test)]` module internals inside the gated modules as needed so unit tests compile only under default features. Fix any ungated-module reference to a gated item (e.g. the anticipated doc-link in `path_util.rs` around line 170) by moving the reference inside the gated surface or adding the same cfg — no other files are edited.
4. Create `tests/engine_free.rs` with test `engine_free_public_surface_resolves` that imports the root re-exports `ExternalTemplateLimitsConfig`, `ResolvedExternalTemplateLimits`, and `TemplateReloadError` and type-checks each (binding to `_` at type position / referencing associated types) without constructing engine state.

**Tests:**
- `engine_absent_build_compiles`: manifest+gates in place → `cargo check -p camel-template --no-default-features` → exit 0 and `cargo tree -p camel-template --no-default-features -e no-dev --prefix none | grep -cE '^(minijinja|camel-language-minijinja) v'` prints `0`; command as stated; expected: pass after implementation.
- `engine_free_public_surface_resolves`: new integration test → `cargo test -p camel-template --no-default-features --test engine_free` → exit 0 (public engine-free re-exports usable without the feature).
- `engine_on_tests_unchanged`: `cargo test -p camel-template` → all pre-existing tests green with zero assertion edits; command as stated; expected: pass.

**Acceptance:**
- `cargo check -p camel-template` and `cargo check -p camel-template --no-default-features` exit 0.
- `cargo test -p camel-template` green; `cargo fmt --check` and `cargo clippy -p camel-template --all-targets -- -D warnings` exit 0.
- `git diff Cargo.lock` still empty.

- [x] 2.1

### Task 2.2: Phase-2 cross-crate verification

**Files:**
- (no source changes; verification task)

**Steps:**
1. Golden re-pin after phase 2: `cargo test -p camel-cli --test feature_profiles` → green, fixture untouched.
2. Consumer compile matrix: `cargo check -p camel-bundles`, `cargo check -p camel-config`, `cargo check -p camel-integration-test`, `cargo check -p camel-cli`, `cargo check -p camel-cli --no-default-features --features slim-http` → all exit 0 (every in-tree consumer takes camel-template defaults, so `TemplateBundle` stays present).
3. `cargo tree -p camel-cli -e features,no-dev --prefix none --locked | sed -E 's| \(/[^)]*\)||g; s| \(\*\)||g; s| \[\*\]||g' | LC_ALL=C sort -u | diff - <(LC_ALL=C sort -u crates/camel-cli/tests/fixtures/default-deptree.txt)` → empty.

**Tests:**
- `golden_zero_diff_post_template`: step 3 pipeline → empty diff; expected: pass.
- `consumers_compile`: step 2 commands; expected: all exit 0.

**Acceptance:**
- All commands exit 0; `git status --short Cargo.lock crates/camel-cli` empty.
- Commit phase 2 with subject `feat(template): optional minijinja engine gate`.

- [x] 2.2

## Phase 3: evidence, measurement, docs

### Task 3.1: slim-http before/after binary size evidence

**Files:**
- `openspec/changes/slimblockers/evidence/slim-size.md` (new)

**Steps:**
1. Build the slim-http profile on the completed tree:
   `cargo build --release --locked -p camel-cli --no-default-features --features slim-http`.
2. Record `stat -c %s target/release/camel` as AFTER; the BEFORE figure is 55,712,576 bytes, measured from the baseline rebuild at the rebased base 37ec0cb6 (`/tmp/slim-size-before.txt`, binary at /tmp/slim-camel-before); the 0.48-base figure 55,685,632 bytes is historical context only — the table carries the 37ec0cb6 pair.
3. Write `evidence/slim-size.md`: before/after table, the exact build command and the base commit for each side (BEFORE built at 37ec0cb6, AFTER at the final tree), host profile note (release, thin LTO, strip, codegen-units 1), and the honest interpretation — expected ~0 delta because camel-cli's own unconditional deps keep all eight bridges + template engine linked in slim; the size win materializes when the next camel-cli mission optionalizes camel-cli's own edges; cite the cargo-tree inversion proving camel-cli's direct edges are the remaining pullers.

**Tests:**
- `after_build_size_recorded`: evidence file exists, contains both numbers and `stat -c %s target/release/camel` output matches the AFTER entry; expected: pass.

**Acceptance:**
- `evidence/slim-size.md` committed with both sizes and the interpretation; numbers are direct `stat` output, no rounding in the table.

- [x] 3.1

### Task 3.2: CONTEXT docs alignment

**Files:**
- `crates/camel-bundles/CONTEXT.md` (modified)

**Steps:**
1. In camel-bundles CONTEXT.md, extend the feature/gating section (the one mirroring ADR-0069 §10 gates) with: the eight bridges are optional deps carried by the http-static gate (transitional, per-bridge split deferred to the next camel-cli mission), BootHandle pool fields cfg-gated, and the redis-via-camel-redis-repo slim-survivor note.
2. Verify (no edits): `grep -c 'all-or-nothing' crates/components/camel-template/src/lib.rs` ≥ 1 — the sentence was written by Task 2.1 step 3; if the grep fails, STOP and report rather than editing camel-template here.

**Tests:**
- `context_docs_current`: grep checks — `grep -c 'http-static' crates/camel-bundles/CONTEXT.md` ≥ 1 and `grep -c 'all-or-nothing' crates/components/camel-template/src/lib.rs` ≥ 1; expected: pass.

**Acceptance:**
- Both greps ≥ 1; `cargo xtask lint-context-citations` exit 0; docs prose in English.

- [x] 3.2

### Task 3.3: slim-benchmarks rename with one-release alias

**Files:**
- `crates/camel-cli/Cargo.toml` (modified)
- `crates/camel-cli/tests/feature_profiles.rs` (modified)
- `crates/camel-cli/CONTEXT.md` (modified)

**Steps:**
1. In camel-cli `[features]`: replace the old marker comment block plus the `slim-http = []` line (the comment saying "Named marker for the `--no-default-features` http-only baseline" goes with it) with:
   ```
   # slim-benchmarks is the renamed slim-http (owner ruling 2026-09-17:
   # benchmark-oriented baseline); the alias below keeps existing legs
   # building for one release.
   slim-benchmarks = []
   # One-release alias (drop at 0.50).
   slim-http = ["slim-benchmarks"]
   ```
   No other camel-cli change.
2. In tests/feature_profiles.rs: replace the three `slim-http` literals (the
   `--features slim-http,grpc` invocation and the two assertion messages) with
   `slim-benchmarks` forms.
3. Add test `slim_alias_resolves_identically`: `tree_lines(&["--no-default-features", "--features", "slim-http"])` equals `tree_lines(&["--no-default-features", "--features", "slim-benchmarks"])` (assert set equality) and `slim_closure_excludes_controllable_set`'s forbidden prefixes are absent from the alias closure too.
4. In `crates/camel-cli/CONTEXT.md`, move the two `slim-http` mentions (the marker-name sentence around line 237 and the compose-additivity sentence around line 251) to the canonical `slim-benchmarks` name, appending one sentence noting the `slim-http` alias lives for one release (drop at 0.50).
5. Run `grep -rn 'slim-http' crates/camel-cli/ --include='*.rs' --include='*.toml' --include='*.md'` and require the output to equal EXACTLY this closed set: the single Cargo.toml comment line containing `slim-http`, the `slim-http = ["slim-benchmarks"]` line, the invocation lines inside `slim_alias_resolves_identically`, and the one CONTEXT.md alias-note sentence. Any other hit is a stray → fix it, do not widen the filter.

**Tests:**
- `slim_alias_resolves_identically`: new test per step 3 → `cargo test -p camel-cli --test feature_profiles slim_alias` green; expected: pass after implementation.
- `slim_plus_grpc_resolves_grpc_only` (renamed literals): `cargo test -p camel-cli --test feature_profiles` full suite green ×1 (the ×3 run happens in the final gate pass).

**Acceptance:**
- `cargo test -p camel-cli --test feature_profiles` green (fixture untouched — `git status --short crates/camel-cli/tests/fixtures` empty).
- `cargo check -p camel-cli --no-default-features --features slim-http` and `--features slim-benchmarks` both exit 0.
- `cargo fmt --check` and `cargo clippy -p camel-cli --all-targets -- -D warnings` exit 0.
- Commit with subject `feat(cli): rename slim-http to slim-benchmarks`.

- [x] 3.3

### Task 3.4: Phase-3 closure — hard pins, e_glm stage-4, bd ledger

**Files:**
- `openspec/changes/slimblockers/evidence/bd-rc-9720m.md` (new)
- `openspec/changes/slimblockers/evidence/bd-rc-wcs3v.md` (new)

**Steps:**
1. Run the final hard-pin proof (conductor, from worktree root):
   `git diff --exit-code 37ec0cb6 -- Cargo.lock` → exit 0 (zero lock drift);
   `git diff --name-only 37ec0cb6 -- crates/camel-cli` prints exactly `crates/camel-cli/Cargo.toml`, `crates/camel-cli/tests/feature_profiles.rs`, and `crates/camel-cli/CONTEXT.md` (which mechanically covers the golden fixture: any fixture touch adds a fourth line and fails).
2. Obtain the MANDATORY stage-4 `e_glm` review of the complete diff (mission owner ruling 2026-09-17); record the verdict in the change dir (`.review.json` + the park report).
3. Write `evidence/bd-rc-9720m.md`: implementation commits, verification results (golden ×3 no-regen, lint, compile matrix, tests), remaining camel-cli-side direct-edge deferral, review verdicts.
4. Write `evidence/bd-rc-wcs3v.md`: same shape — commits, verification, the deferred in-workspace flip (workspace-inheritance rule + forbidden consumers), review verdicts.
5. Post both to bd from the MAIN repo root (never the worktree), using absolute worktree paths for the evidence files:
   `bd comments add rc-9720m -f /home/shared/rust-camel-worktrees/slimblockers/openspec/changes/slimblockers/evidence/bd-rc-9720m.md` and
   `bd comments add rc-wcs3v -f /home/shared/rust-camel-worktrees/slimblockers/openspec/changes/slimblockers/evidence/bd-rc-wcs3v.md`.
6. Verify both comments landed: `bd show rc-9720m --json` and `bd show rc-wcs3v --json` — comment_count incremented by exactly one on each.

**Tests:**
- `final_hard_pins_hold`: step 1 commands → Cargo.lock zero diff; camel-cli diff limited to the rename files; expected: pass.
- `stage4_expert_verdict_recorded`: `.review.json` in the change dir contains the e_glm verdict; expected: present before parking.

**Acceptance:**
- Step 1 checks pass; both bd comments posted AND verified (`bd show <id> --json` comment_count +1 on each); e_glm verdict recorded.

- [x] 3.4
