# Tasks: bridgeforward

## camel-bundles

### Task 1.1: Per-bridge feature split and cfg-gate renames

**Files:**
- `crates/camel-bundles/Cargo.toml` (modified)
- `crates/camel-bundles/src/lib.rs` (modified)

**Steps:**
1. In `crates/camel-bundles/Cargo.toml` `[features]`: replace the
   carrier `http-static` line (the one whose body lists the eight
   bridge `dep:` entries) with `http-static = []`; add eight keys with exactly these bodies:
   `jms = ["dep:camel-component-jms"]`, `sql = ["dep:camel-component-sql"]`,
   `redis = ["dep:camel-component-redis"]`,
   `opensearch = ["dep:camel-component-opensearch"]`,
   `ws = ["dep:camel-component-ws"]`, `cxf = ["dep:camel-component-cxf"]`,
   `xj = ["dep:camel-xj"]`, `xslt = ["dep:camel-xslt"]`; extend `default`
   to `["grpc", "wasm", "http-static", "llm", "surrealdb", "mqtt", "mcp",
   "jms", "sql", "redis", "opensearch", "ws", "cxf", "xj", "xslt"]`;
   replace the transitional-carrier comment block with: http-static gates
   the HttpStaticBundle registration only (its pre-115 meaning); the eight
   bridges are per-bridge gates forwarded by camel-cli (mission 121,
   rc-9720m).
2. In `crates/camel-bundles/src/lib.rs`, rename every transitional
   `#[cfg(feature = "http-static")]` gate to its per-bridge feature, and
   split the combined pool blocks per feature:
   - `BridgeCleanup.xslt` field, its `stop()` drain line, and the
     `xslt_runtime` block in `boot()`: `#[cfg(feature = "xslt")]`
   - `BridgeCleanup.xj` field, its drain, and `xj_runtime`:
     `#[cfg(feature = "xj")]`
   - `BootHandle.jms_pool` field: `#[cfg(feature = "jms")]`;
     `cxf_pool` field: `#[cfg(feature = "cxf")]`
   - shutdown step 1: split the single two-pool block into
     `#[cfg(feature = "jms")] self.jms_pool.begin_shutdown();` and
     `#[cfg(feature = "cxf")] self.cxf_pool.begin_shutdown();`
   - shutdown step 3: split the two timeout matches into one
     `#[cfg(feature = "jms")]` block (JMS match) and one
     `#[cfg(feature = "cxf")]` block (CXF match)
   - `WsBundle` registration: `#[cfg(feature = "ws")]` (keep the note
     that this is the ws gate, no longer a carrier rider)
   - jms pool `register_bundle_with`: `#[cfg(feature = "jms")]`; cxf
     pool: `#[cfg(feature = "cxf")]`
   - OpenSearch bundle: `#[cfg(feature = "opensearch")]`; Redis bundle:
     `#[cfg(feature = "redis")]`; the Sql bundle block:
     `#[cfg(feature = "sql")]`
   - HttpStaticBundle gate: UNCHANGED (`http-static`).
   Delete every "Transitional gate: rides http-static" comment; update
   the `BootHandle` struct doc ("present only under the http-static
   bridge carrier" → jms/cxf per-bridge gates) and the
   `shutdown_with_deadline` doc ("Pool steps 1 and 3 are
   cfg-conditional on the `http-static` feature" → on the `jms`/`cxf`
   features, one block per pool).
3. Update the inline `mod tests` cfg polarity: gate
   `boot_registers_all_bundles_from_fixture_config` with
   `#[cfg(all(feature = "jms", feature = "sql", feature = "redis",
   feature = "opensearch", feature = "ws", feature = "cxf",
   feature = "xj", feature = "xslt"))]`; gate
   `boot_slim_registers_core_without_bridges` with
   `#[cfg(not(feature = "jms"))]` and update its doc comment (bridges
   off = per-bridge features off; assert text "without http-static" →
   "without the jms feature").
4. Add test `http_static_registers_without_bridges` gated
   `#[cfg(all(feature = "http-static", not(feature = "jms"),
   not(feature = "sql"), not(feature = "redis"),
   not(feature = "opensearch"), not(feature = "ws"),
   not(feature = "cxf"), not(feature = "xj"), not(feature = "xslt")))]`:
   boot the `no-http/Camel.toml` fixture, then assert
   `ctx.registry().get("http-static").is_some()` (verify the exact
   scheme string HttpStaticBundle registers by reading
   `crates/components/camel-http` source; use that string) and
   `ctx.registry().get("jms").is_none()`.
5. Add test `boot_subset_sql_jms_registers_only_selected` gated
   `#[cfg(all(feature = "sql", feature = "jms", not(feature = "cxf"),
   not(feature = "opensearch"), not(feature = "redis"),
   not(feature = "ws"), not(feature = "xj"), not(feature = "xslt")))]`:
   boot the `bundles-present/Camel.toml` fixture, assert
   `registry().get("sql").is_some()`, `get("jms").is_some()`, and each
   of cxf/opensearch/redis/ws/xj/xslt `.is_none()`, then
   `handle.shutdown(&mut ctx).await` returns Ok (jms pool drains, no
   cxf pool compiled).

**Tests:** (executable)
- `boot_registers_all_bundles_from_fixture_config`: default features →
  boot bundles-present fixture → all 13 schemes resolve. Command:
  `RUSTC_WRAPPER= cargo test -p camel-bundles --lib boot_registers_all`.
  Expected: pass after steps 1-3.
- `boot_slim_registers_core_without_bridges`: no-default features →
  boot no-http fixture → core schemes resolve, jms absent, shutdown
  drains. Command:
  `RUSTC_WRAPPER= cargo test -p camel-bundles --lib --no-default-features boot_slim`.
  Expected: pass after steps 1-3.
- `http_static_registers_without_bridges`:
  `--no-default-features --features http-static` → http-static scheme
  resolves, jms does not. Command:
  `RUSTC_WRAPPER= cargo test -p camel-bundles --lib --no-default-features --features http-static http_static_registers`.
  Expected: fails before step 4 (test absent), passes after.
- `boot_subset_sql_jms_registers_only_selected`:
  `--no-default-features --features sql,jms` → sql+jms resolve, the
  six unselected bridges absent, shutdown drains. Command:
  `RUSTC_WRAPPER= cargo test -p camel-bundles --lib --no-default-features --features sql,jms boot_subset`.
  Expected: fails before step 5, passes after.

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-bundles --lib` green under each
  of: default; `--no-default-features`;
  `--no-default-features --features http-static`;
  `--no-default-features --features sql,jms`
- `RUSTC_WRAPPER= cargo clippy -p camel-bundles --all-targets -- -D warnings` green (default)
- `RUSTC_WRAPPER= cargo fmt --check` green
- `grep -c 'feature = "http-static"' crates/camel-bundles/src/lib.rs`
  returns exactly 2 (the HttpStaticBundle registration gate + the new
  `http_static_registers_without_bridges` test gate; no pool/cleanup/
  bundle site remains)

- [x] 1.1

### Task 1.2: camel-bundles CONTEXT.md feature notes

**Files:**
- `crates/camel-bundles/CONTEXT.md` (modified)
- `crates/camel-bundles/README.md` (modified — r_glm review fold-in:
  the same stale carrier sentence lived there)

**Steps:**
1. Update the feature-gating paragraph (currently around lines 52-63):
   the gate list becomes `grpc`, `wasm`, `http-static` (HttpStaticBundle
   only, no longer a carrier), `llm`, `surrealdb`, `mqtt`, `mcp`
   default-on plus the eight per-bridge gates `jms`, `sql`, `redis`,
   `opensearch`, `ws`, `cxf`, `xj`, `xslt` (default-on, forwarded
   same-named from camel-cli, mission 121); remove the "transitional
   carrier" wording; keep the redis slim-survivor note (retention now
   attributed to camel-config → camel-redis-repo, plus camel-cli's own
   now-optional edge dropped); note BootHandle jms/cxf pools gate on
   their own features.

**Tests:**
- doc-only task; verification is the acceptance greps.

**Acceptance:**
- `grep -i 'carrier' crates/camel-bundles/CONTEXT.md` returns no
  transitional-carrier statement (historical archive references excepted
  if any quote the 115 design; if so rephrase to past tense)
- `RUSTC_WRAPPER= cargo xtask lint-context-citations` exit 0

- [x] 1.2

## camel-cli

### Task 2.1: Own-dep optionalization and per-bridge forwarding features

DEPENDS ON Task 1.1 (the forwards reference camel-bundles keys that must
exist).

**Files:**
- `crates/camel-cli/Cargo.toml` (modified)
- `crates/camel-integration-test/Cargo.toml` (modified)

**Steps:**
1. Make the eight own bridge deps optional (keep workspace inheritance):
   `camel-component-jms`, `camel-component-sql`,
   `camel-component-redis`, `camel-component-opensearch`,
   `camel-component-ws`, `camel-component-cxf`, `camel-xj`,
   `camel-xslt` → `{ workspace = true, optional = true }`.
2. Add eight forwarding features next to the existing capability
   features, each carrying BOTH halves (own dep + bundles gate —
   lint-gate-forwarding Rules 1 and 2):
   `jms = ["dep:camel-component-jms", "camel-bundles/jms"]`,
   `sql = ["dep:camel-component-sql", "camel-bundles/sql"]`,
   `redis = ["dep:camel-component-redis", "camel-bundles/redis"]`,
   `opensearch = ["dep:camel-component-opensearch", "camel-bundles/opensearch"]`,
   `ws = ["dep:camel-component-ws", "camel-bundles/ws"]`,
   `cxf = ["dep:camel-component-cxf", "camel-bundles/cxf"]`,
   `xj = ["dep:camel-xj", "camel-bundles/xj"]`,
   `xslt = ["dep:camel-xslt", "camel-bundles/xslt"]`,
   with one comment block: same-named per camel-bundles' gates (Rule 1),
   forwarded so Rule 2 holds; slim drops the bridges by not selecting
   them; redis stays linked in slim via camel-config → camel-redis-repo
   (out of zone); camel-xj pulls camel-xslt transitively (documented).
3. Extend `full` with `"jms"`, `"sql"`, `"redis"`, `"opensearch"`,
   `"ws"`, `"cxf"`, `"xj"`, `"xslt"` (default closure package set
   unchanged — golden test proves it).
4. Change `redis-tls = ["camel-component-redis/tls"]` to
   `redis-tls = ["redis", "camel-component-redis/tls"]` (TLS is a
   refinement of the redis capability, which now owns activation).
5. Update the `http-static` feature comment: it forwards camel-bundles'
   HttpStaticBundle gate (the static-file scheme); since mission 121 it
   carries no bridge deps.
6. In `crates/camel-integration-test/Cargo.toml`, change
   `sql = ["dep:sqlx"]` to `sql = ["dep:sqlx", "camel-bundles/sql"]`
   (mirror of the existing `security` forward). Reason: the harness is
   a camel-bundles consumer, so lint Rule 1 fires the moment
   camel-bundles gains a `sql` gate — the shadow feature must forward
   it. Closure-safe: camel-cli default already activates
   camel-component-sql via `full → sql`; the forward is
   forwarding-seeded (renders nothing in the golden fixture), and
   harness full-boots gain coherent SqlBundle registration under the
   camel-bundles `sql` gate.

**Tests:** (executable — metadata-only, no compile of gated code)
- `default_closure_matches_golden`: after steps 1-3 the default closure
  still matches the committed fixture WITHOUT regeneration (dep:-activated
  edges render identically to unconditional edges; forwarding renders
  nothing). Command:
  `RUSTC_WRAPPER= cargo test -p camel-cli --test feature_profiles default_closure_matches_golden`.
  Expected: pass after steps 1-3 as a set — between step 1
  (optionalize) and step 3 (extend `full`) the closure transiently
  lacks the bridges, so the golden test is transiently red; NEVER
  regenerate the fixture to silence a transient state (a persistent
  post-step-3 failure means the rendering assumption is broken — STOP
  and report instead).
- gate-forwarding completeness:
  `RUSTC_WRAPPER= cargo xtask lint-gate-forwarding` exits 0 with the 17
  gates (9 existing + 8 new) across BOTH consumers (camel-cli Rule 2;
  camel-integration-test Rule 1 on `sql` via step 6). Expected: fails
  between steps 1 and 2 (shadow names without forwards / missing
  forwards), passes after steps 2 and 6.

**Acceptance:**
- `RUSTC_WRAPPER= cargo xtask lint-gate-forwarding` exit 0
- `RUSTC_WRAPPER= cargo test -p camel-cli --test feature_profiles` green (default features — whole file)
- `RUSTC_WRAPPER= cargo check -p camel-cli` green (default)
- `RUSTC_WRAPPER= cargo fmt --check` green

- [x] 2.1

### Task 2.2: Lint-catalog cfg gates in camel-cli src

DEPENDS ON Task 2.1 (features must exist for the cfgs).

**Files:**
- `crates/camel-cli/src/lib.rs` (modified)
- `crates/camel-cli/tests/lint_catalog_gates.rs` (new)

**Steps:**
1. In `register_builtin_components_for_lint`
   (crates/camel-cli/src/lib.rs): gate each bridge registration with its
   feature —
   `#[cfg(feature = "xslt")]` on the `camel_xslt::XsltComponent`
   registration; `#[cfg(feature = "xj")]` on `camel_xj::XjComponent`;
   `#[cfg(feature = "ws")]` on the WsBundle line;
   `#[cfg(feature = "opensearch")]` on OpenSearchBundle;
   `#[cfg(feature = "redis")]` on RedisBundle;
   `#[cfg(feature = "jms")]` on the JmsBundle line (merge with its
   comment); `#[cfg(feature = "cxf")]` on CxfBundle;
   `#[cfg(feature = "sql")]` on the
   `register_datasource_bundle_empty!(ctx, camel_component_sql::SqlBundle,
   datasource_catalog)` line. Because `sql` and `surrealdb` become the
   only consumers of the local binding, gate the fn-body
   `use camel_api::datasource::DatasourceCatalog;` /
   `use camel_core::datasource::RuntimeDatasourceCatalog;` statements
   and the `let datasource_catalog: Arc<dyn DatasourceCatalog>` binding
   with `#[cfg(any(feature = "sql", feature = "surrealdb"))]` so slim
   compiles warning-free (the workspace lint set elevates warnings).
   Keep section order; update the function's
   doc comment: bridge schemes are feature-gated like wasm/exec; slim
   builds surface them as `unverified-scheme` lint notes (accepted
   graceful degradation).
2. New test file `crates/camel-cli/tests/lint_catalog_gates.rs` with
   `#![cfg(not(feature = "jms"))]` at the top and two tests:
   - `slim_lint_catalog_omits_gated_bridges`: build a fresh
     `CamelContext` (mirror the construction idiom used in
     `crates/camel-cli/tests/lint_corpus.rs`; read it first), call
     `register_builtin_components_for_lint(&mut ctx)`, then assert
     `ctx.registry().get("jms").is_none()`, `get("xslt").is_none()`,
     `get("sql").is_none()`, while `get("http").is_some()` and
     `get("timer").is_some()`.
   - `slim_lint_flags_gated_bridge_scheme`: diagnostic-level coverage —
     lint a route document whose endpoint URI is `jms:queue` through the
     production lint engine (reuse the `production_engine` / lint-corpus
     harness idiom from `tests/lint_corpus.rs`), and assert the output
     contains exactly one `unverified-scheme` diagnostic naming the
     jms endpoint (the same Info class wasm/exec produce).

**Tests:** (executable)
- `slim_lint_catalog_omits_gated_bridges` and
  `slim_lint_flags_gated_bridge_scheme`: slim-compiled test binary →
  register builtins → gated bridge schemes absent, core schemes
  present; and a `jms:` URI lints to exactly one `unverified-scheme`
  diagnostic. Command:
  `RUSTC_WRAPPER= cargo test -p camel-cli --no-default-features --features slim-benchmarks --test lint_catalog_gates`.
  Expected: both fail before step 1 (ungated registrations make jms
  resolve), pass after.
- Default catalog unchanged (regression):
  `RUSTC_WRAPPER= cargo test -p camel-cli --test lint_corpus` green.

**Acceptance:**
- `RUSTC_WRAPPER= cargo check -p camel-cli --no-default-features` green
- `RUSTC_WRAPPER= cargo check -p camel-cli --no-default-features --features sql` green
- `RUSTC_WRAPPER= cargo clippy -p camel-cli --all-targets -- -D warnings` green (default)
- `RUSTC_WRAPPER= cargo test -p camel-cli --test lint_corpus` green
- `RUSTC_WRAPPER= cargo fmt --check` green

- [x] 2.2

### Task 2.3: Slim exclusion set and bridge composition tests

DEPENDS ON Task 2.1.

**Files:**
- `crates/camel-cli/tests/feature_profiles.rs` (modified)

**Steps:**
1. Extend `SLIM_FORBIDDEN_PREFIXES` with seven bridge prefixes:
   `"camel-component-jms v"`, `"camel-component-sql v"`,
   `"camel-component-opensearch v"`, `"camel-component-ws v"`,
   `"camel-component-cxf v"`, `"camel-xj v"`, `"camel-xslt v"`.
   Extend the const's doc comment: redis is excepted (unconditional
   camel-config → camel-redis-repo path, out of zone); camel-xj pulls
   camel-xslt transitively so slim+`xj` links both (default/full enable
   all eight via `full`).
2. Fix the count-dependent comments and filters:
   `slim_plus_grpc_resolves_grpc_only` filters "the other thirteen" →
   the other twenty; `dynamic_linking_closure_resolves_kafka`'s
   "remaining controllable set" filter needs no name change but recount
   in any comment; `slim_alias_resolves_identically` unchanged.
3. Add `slim_plus_sql_resolves_sql_only`: tree with
   `["--no-default-features", "--features", "slim-benchmarks,sql"]`;
   assert some line starts with `"camel-component-sql v"`; build the
   forbidden list minus the sql prefix and `assert_absent` it.
4. Add a package-targeting tree helper: `tree_lines` pins
   `-p camel-cli` inside `TREE_BASE_ARGS`, so a second `-p` would union
   roots instead of retargeting — add a separate
   `tree_lines_for(package, extra_args)` (or equivalent) that REPLACES
   the pinned package (and uses the same `-e no-dev` edge filter the
   bundles probes need) while reusing the normalization pipeline.
5. Add `bundles_slim_drops_bridges`: tree of camel-bundles
   `--no-default-features -e no-dev`; assert absent the eight bridge
   package prefixes EXCEPT `camel-component-redis v` (camel-config →
   camel-redis-repo path); the doc comment cites the out-of-zone
   deferral.
6. Add `bundles_per_bridge_composes`: tree of camel-bundles
   `--no-default-features --features sql`; assert
   `"camel-component-sql v"` present and the other six bridges absent
   (`camel-component-redis v` excepted — the camel-config path keeps it
   in camel-bundles' own tree under any feature set).
7. Add `redis_tls_implies_redis`: manifest assertion in the
   `kafka_feature_table_implies_capability` idiom — read
   `crates/camel-cli/Cargo.toml`, extract the `[features]` section,
   assert the exact line
   `redis-tls = ["redis", "camel-component-redis/tls"]` (the implication
   is feature-table-level; forwarding edges do not render in cargo
   tree).

**Tests:** (executable)
- `slim_closure_excludes_controllable_set`: slim closure → 21 forbidden
  prefixes absent. Command:
  `RUSTC_WRAPPER= cargo test -p camel-cli --test feature_profiles slim_closure_excludes`.
  Expected: fails before step 1 (bridges present), passes after.
- `slim_plus_sql_resolves_sql_only`: command:
  `RUSTC_WRAPPER= cargo test -p camel-cli --test feature_profiles slim_plus_sql`.
  Expected: pass after step 3.
- `bundles_slim_drops_bridges`: command:
  `RUSTC_WRAPPER= cargo test -p camel-cli --test feature_profiles bundles_slim_drops`.
  Expected: pass after step 5.
- `bundles_per_bridge_composes`: command:
  `RUSTC_WRAPPER= cargo test -p camel-cli --test feature_profiles bundles_per_bridge`.
  Expected: pass after step 6.
- `redis_tls_implies_redis`: command:
  `RUSTC_WRAPPER= cargo test -p camel-cli --test feature_profiles redis_tls`.
  Expected: fails before step 7 (line still the old body), passes after.

**Acceptance:**
- `RUSTC_WRAPPER= cargo test -p camel-cli --test feature_profiles` green (default features, whole file)
- No golden fixture file modified:
  `git diff --stat crates/camel-cli/tests/fixtures/` empty
- `RUSTC_WRAPPER= cargo fmt --check` green

- [x] 2.3

### Task 2.4: camel-cli CONTEXT.md profile notes

**Files:**
- `crates/camel-cli/CONTEXT.md` (modified)

**Steps:**
1. In the "Build profiles" section, "Feature profiles" paragraph:
   replace the trailing deferral sentences — the ones stating the eight
   bridges stay linked in every camel-cli profile through camel-cli's
   OWN unconditional dependencies, with the diet deferred to the
   camel-cli bridge-forward mission — with: the eight bridges are
   optional via same-named camel-cli features (`jms`, `sql`, `redis`,
   `opensearch`, `ws`, `cxf`, `xj`,
   `xslt`), each activating the own dependency and forwarding the
   camel-bundles gate; `full` enables all eight; slim drops seven of
   them, redis remaining via the unconditional camel-config →
   camel-redis-repo path (out of zone); `redis-tls` implies `redis`;
   camel-cli's `http-static` feature forwards camel-bundles'
   HttpStaticBundle gate only.
2. Same paragraph, minijinja sentence: replace "the in-workspace flip
   rides the camel-cli bridge-forward mission" with a follow-up
   deferral: the flip needs the named-feature shape that forces golden
   regeneration; tracked as bd rc-gcs5d (discovered from rc-9720m).

**Tests:**
- doc-only task; verification is the acceptance greps.

**Acceptance:**
- `grep -c 'bridge-forward' crates/camel-cli/CONTEXT.md` returns 0
- `grep -c 'OWN unconditional' crates/camel-cli/CONTEXT.md` returns 0
- `RUSTC_WRAPPER= cargo xtask lint-context-citations` exit 0

- [x] 2.4

## evidence

### Task 3.1: AFTER size evidence

DEPENDS ON Tasks 1.1, 2.1, 2.2, 2.3.

**Files:**
- `openspec/changes/bridgeforward/evidence/size.md` (new)
- `openspec/changes/bridgeforward/evidence/slim-smoke.routes.yaml` (new)

**Steps:**
1. Build the slim binary AFTER:
   `RUSTC_WRAPPER= cargo build --release -p camel-cli --no-default-features --features slim-benchmarks`;
   record exact bytes of `target/release/camel`.
2. Capture proof the chains left:
   `RUSTC_WRAPPER= cargo tree -p camel-cli --no-default-features -e no-dev --prefix none`
   piped through `grep -E 'camel-component-(jms|sql|opensearch|ws|cxf)|camel-xj|camel-xslt|sqlx'` —
   save the (empty or redis-only) result.
3. Boot smoke against the built binary (first change to alter the slim
   linked set; the release artifact itself must prove it boots): write
   `evidence/slim-smoke.routes.yaml` with two routes — an http consumer
   on `http://127.0.0.1:18087/hz` forwarding to a `log:` step, and a
   one-shot `timer` route (single-fire, short delay) forwarding to
   `log:slim-smoke-marker`. Run
   `timeout --signal=TERM 20 target/release/camel run openspec/changes/bridgeforward/evidence/slim-smoke.routes.yaml`
   capturing combined output. Expected: exit code 0 (first SIGTERM →
   graceful shutdown per the signal contract), transcript contains a
   `Route started` line for EACH route (camel-http's explicit
   readiness guarantees the listener bound before that line —
   `crates/components/camel-http/src/lib.rs` readiness path), and the
   one-shot `slim-smoke-marker` log emission between them. Do NOT
   grep for a bind-ack format or race a concurrent curl; `Route
   started` after http readiness is the stable assertion. Record the
   captured transcript in size.md.
4. Write `evidence/size.md`: BEFORE 55,652,544 bytes (base f1a70e5f,
   recorded 2026-09-18, same command), AFTER bytes, delta absolute and
   percent, the tree proof, the boot-smoke transcript, and the
   redis-retention explanation.
5. Mission rule: if the delta is on the order of KBs, STOP and report —
   the load lives elsewhere (do not park a green-but-hollow change).

**Tests:**
- evidence capture; verification is the acceptance checks.

**Acceptance:**
- `evidence/size.md` exists and contains both byte numbers, the
  delta arithmetic (AFTER − BEFORE, percent), and the boot-smoke
  marker/bind evidence
- delta magnitude ≥ 1,000,000 bytes (MBs); smaller → task FAILS by
  mission rule and the conductor stops for the human

- [x] 3.1
