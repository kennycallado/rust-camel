# Tasks: three-flavor-matrix

Single-phase change. Task order is load-bearing: bodies before tests, tests
before pipeline, pipeline lockstep (docker/release) after artifacts exist.
Run all cargo commands from the repo root of the working tree.
NOTE: after Task 1 lands alone, several closure tests are temporarily red
(marker-table and slim-body expectations); Task 2 turns them green. That is
acknowledged mid-phase breakage — do not "fix" Task 1 to avoid it.

## Task 1: Chained flavor bodies + Tier-2 gates in Cargo.toml

Files:
- `crates/camel-cli/Cargo.toml` (modified)
- `crates/camel-bundles/Cargo.toml` (modified — one feature)
- `crates/camel-bundles/src/lib.rs` (modified — one cfg gate)

Steps:
1. Replace the `flavor-slim` body `["slim-http"]` with the chained edge pack:
   `["mqtt", "mqtt-tls", "http-static", "sql", "lang-jsonpath", "lang-rhai"]`.
2. Replace the `flavor-regular` body `["full"]` with:
   `["flavor-slim", "otel", "grpc", "wasm", "llm", "mcp", "security", "redis", "redis-tls", "jms", "cxf", "xj", "xslt", "opensearch", "ws", "lang-xpath", "lang-js", "lang-minijinja", "lsp", "kubernetes", "integration-http", "integration-sql"]`.
3. Replace the `flavor-full` body `["full", "kafka"]` with:
   `["flavor-regular", "exec", "kafka", "surrealdb", "containers"]`.
4. New Tier-2 feature `containers` (one feature for the coupled pair,
   function⇒container per ADR-0005):
   - `camel-cli/Cargo.toml`: `camel-function` dependency gains
     `optional = true`; add feature
     `containers = ["dep:camel-function", "camel-bundles/containers"]`.
   - `camel-bundles/Cargo.toml`: `camel-component-container` dependency
     gains `optional = true`; add feature
     `containers = ["dep:camel-component-container"]`.
   - `camel-bundles/src/lib.rs` line ~344: gate the
     `register_bundle::<camel_component_container::ContainerBundle>` call
     with `#[cfg(feature = "containers")]` (keep the always-on Http, File,
     Template, Master registrations untouched).
   - `crates/camel-cli/src/` (the two concrete wiring sites — locate with
     `grep -rn 'camel_function::' crates/camel-cli/src/`, expect
     `run.rs:253` and `job/mod.rs:1022`): gate each
     `camel_function::FunctionRuntimeService::with_default_container_provider`
     call behind `#[cfg(feature = "containers")]` so a build without the
     feature constructs no function runtime and the `function:` /
     `container:` paths fail closed with an explicit error at those
     call sites (verify the existing error path fires; if no explicit
     rejection exists for the featureless path, STOP and report
     `test-design-gap: function step without containers feature has no
     explicit rejection`).
5. New feature `kubernetes`: in `crates/camel-cli/Cargo.toml`, remove the
   hardcoded `"kubernetes"` from the `camel-config` dependency's
   `features = ["otel", "kubernetes"]` (KEEP `"otel"` — base telemetry
   wiring), and add feature
   `kubernetes = ["camel-config/kubernetes"]`.
6. Delete the `slim-http` and `slim-benchmarks` alias feature entries and
   their comment block (aliases expire at 0.50; this change lands ≥0.50).
7. Update the comment above the flavor markers: chained bodies, the four
   principled exclusions from regular (kafka C-dep, surrealdb BUSL,
   exec ADR-0037, containers daemon-client), and that CI legs pass only
   `flavor-*` markers.

Tests (run after edit; verification commands, not new test files):
- name: all three markers resolve
  action: for each of `flavor-slim` (with `--no-default-features`),
  `flavor-regular`, `flavor-full`:
  `CARGO_TERM_COLOR=never cargo tree -p camel-cli [--no-default-features] --features <marker> -e features,no-dev > /dev/null`
  assert: exit 0 for all three
- name: slim excludes the principled set
  action: `cargo tree -p camel-cli --no-default-features --features flavor-slim -e no-dev --prefix none | grep -ci 'surrealdb\|camel-function\|camel-component-container'`
  assert: 0 (kafka/exec absent by feature absence — verify via the closure
  test in Task 2)
- name: default (regular) excludes exactly the four
  action: `cargo tree -p camel-cli --features flavor-regular -e no-dev --prefix none | grep -ci 'surrealdb\|rdkafka\|camel-component-exec\|camel-function\|camel-component-container'`
  assert: 0
- name: default includes the new regulars
  action: `cargo tree -p camel-cli --features flavor-regular -e no-dev --prefix none | grep -c 'kube\|boa_engine\|rhai\|tower-lsp'`
  assert: ≥ 4 (kubernetes client, boa, rhai, lsp all in regular)
- name: removed aliases rejected
  action: `cargo tree -p camel-cli --no-default-features --features slim-http -e features,no-dev > /dev/null 2>&1`
  assert: exit non-zero, stderr contains `none of the selected packages contains these features: slim-http`

Acceptance:
- all five verification commands hold their asserted outcomes
- `grep -c 'slim-http\|slim-benchmarks' crates/camel-cli/Cargo.toml` returns 0
- `grep -n 'features = \["otel"\]' crates/camel-cli/Cargo.toml` matches the
  camel-config dependency line (kubernetes de-hardcoded)
- `grep -c 'cfg(feature = "containers")' crates/camel-bundles/src/lib.rs` ≥ 1
- `cargo fmt --check` and `cargo clippy -p camel-cli -p camel-bundles -- -D warnings` exit 0

- [x] 1

## Task 2: Closure contract tests and golden fixture

Files:
- `crates/camel-cli/tests/feature_profiles.rs` (modified)
- `crates/camel-bundles/src/lib.rs` (modified — one cfg-gated test)
- golden fixture file read by `golden_fixture_lines()`
  (`crates/camel-cli/tests/` directory; path comes from the constant inside
  `golden_fixture_lines`, do not relocate it) (modified)

Steps:
1. Extract shared prefix consts: today `SLIM_FORBIDDEN_PREFIXES`
   (lines ~340-362) holds ~21 entries, mostly INLINE string literals
   (mqtt, wasm, llm, mcp, jms, opensearch, ws, cxf, camel-xj, camel-xslt,
   lsp/tower-lsp, language crates) plus the const `GRPC_PREFIX` and
   `SQL_PREFIX`. Extract the literals that remain relevant in the final
   model into named consts (`KAFKA_PREFIX`, `EXEC_PREFIX`,
   `LANG_JS_PREFIX`, `LANG_XPATH_PREFIX`, `SURREALDB_PREFIX`
   (`camel-component-surrealdb v`), `FUNCTION_PREFIX` (`camel-function v`),
   `CONTAINER_PREFIX` (`camel-component-container v`), and `SECURITY_PREFIX`
   — the actual shared security crate observed via
   `cargo tree -p camel-cli --features security -e features,no-dev`, do not
   guess). Entries that become irrelevant (mqtt, sql, jsonpath, rhai, and
   the other regular capabilities) are NOT extracted — they are deleted in
   step 2. Do NOT duplicate string literals across arrays.
2. REPLACE the entire `SLIM_FORBIDDEN_PREFIXES` array (it is ~21 entries
   today, including `SQL_PREFIX`, wasm, llm, mcp, jms, opensearch, ws, cxf,
   xj, xslt, lsp, `camel-language-rhai`, `camel-language-jsonpath`,
   minijinja — most of which are IN slim or regular now) with the 8-entry
   final set: `KAFKA_PREFIX`, `EXEC_PREFIX`, `LANG_JS_PREFIX`,
   `LANG_XPATH_PREFIX`, `SURREALDB_PREFIX`, `FUNCTION_PREFIX`,
   `CONTAINER_PREFIX`, `SECURITY_PREFIX`. Explicitly DROP `SQL_PREFIX` and
   `camel-language-jsonpath v` (both IN slim now). Put any rationale
   comment ABOVE the const declaration, never between the array brackets
   (the acceptance grep scopes to the bracket range).
3. Add `REGULAR_FORBIDDEN_PREFIXES: &[&str]` = `KAFKA_PREFIX`,
   `EXEC_PREFIX`, `SURREALDB_PREFIX`, `FUNCTION_PREFIX`,
   `CONTAINER_PREFIX` (the four principled exclusions; lang-js/rhai/xpath
   are IN regular). Add `FULL_REQUIRED_PREFIXES: &[&str]` =
   `&[KAFKA_PREFIX, SURREALDB_PREFIX, FUNCTION_PREFIX, CONTAINER_PREFIX,
   EXEC_PREFIX]`.
4. Add `REGULAR_REQUIRED_PREFIXES: &[&str]` = the capabilities regular must
   resolve — use REAL crate names observed from
   `cargo tree -p camel-cli --features flavor-regular -e features,no-dev`:
   mqtt, redis, `SQL_PREFIX`, jms, otel instrumentation crate,
   `LANG_JSONPATH` (`camel-language-jsonpath v`), rhai (`rhai v`),
   boa (`boa_engine v`), `LANG_XPATH` (`camel-language-xpath v`),
   `LANG_MINIJINJA` (`camel-language-minijinja v`), lsp (`tower-lsp v`),
   kubernetes client (the `kube` crate or actual name observed), wasm
   (`wasmtime v`), `SECURITY_PREFIX`. NOTE on http: `camel-component-http`
   is a NON-OPTIONAL dep (present in every closure including slim — a tree
   assert on it is vacuous, do not add it), and `http-static` is a
   registration-level gate with no tree-level crate (already covered by
   camel-bundles' `http_static_registers_without_bridges` test) — neither
   gets a REGULAR_REQUIRED entry; http capability is proven by the existing
   registration test.
5. Add test `regular_closure_satisfies_contract`:
   action `tree_lines(&["--features", "flavor-regular"])`; assert every
   `REGULAR_REQUIRED_PREFIXES` entry present and every
   `REGULAR_FORBIDDEN_PREFIXES` entry absent (reuse `assert_absent`).
6. Add test `full_closure_satisfies_contract`:
   action `tree_lines(&["--features", "flavor-full"])`; assert every
   `FULL_REQUIRED_PREFIXES` entry present (kafka, surrealdb, function,
   container, exec all reachable in full).
7. Add test `slim_closure_satisfies_contract`:
   action `tree_lines(&["--no-default-features", "--features", "flavor-slim"])`;
   assert mqtt, rhai, jsonpath and the sql stack present, and every
   `SLIM_FORBIDDEN_PREFIXES` entry absent.
8. Add test `full_covers_universe` (the anti-omission net): parse the
   `[features]` table of `crates/camel-cli/Cargo.toml` (the test file
   already reads files for the golden fixture — follow that pattern);
   compute the closure of `flavor-full` (existing tree helpers) as a
   feature-name set by resolving `flavor-slim`/`flavor-regular` chains;
   assert every feature name in `[features]` EXCEPT the non-flavor axes
   (`jemalloc`, `dynamic-linking`, `itest-e2e`, `slim-benchmarks`-style
   aliases if any remain, and the `flavor-*` markers themselves) appears
   transitively in the flavor-full body. A new feature placed in no flavor
   fails this test with a message naming it.
9. Update `flavor_regular_closure_equals_full` (line ~556): regular no
   longer relates to `full` that way. REPLACE with
   `flavor_chain_is_structural`: assert the closure of `flavor-slim` ⊆
   closure of `flavor-regular` ⊆ closure of `flavor-full` (set inclusion on
   normalized tree lines — the chained bodies make this structural; the
   test guards against future body edits that break the chain).
    ALSO delete `flavor_full_closure_equals_full_plus_kafka` (line ~550) —
    its premise (flavor-full == legacy `full` + kafka) is false under the
    new bodies (flavor-full adds containers/kubernetes the legacy `full`
    lacks); it is superseded by `full_closure_satisfies_contract` +
    `flavor_chain_is_structural`.
10. Update `flavor_slim_closure_equals_slim_http` (line ~563): DELETE it —
    replaced by `slim_closure_satisfies_contract` (step 7).
11. Delete `slim_alias_resolves_identically` (line ~426) — its subject alias
    is deleted in Task 1.
12. Retarget `slim_plus_grpc_resolves_grpc_only` (line ~376) and
    `slim_plus_sql_resolves_sql_only` (line ~400): replace the deleted
    `slim-benchmarks` feature with `flavor-slim` in their feature
    selections (e.g. `--no-default-features --features flavor-slim,grpc`).
    NOTE: `slim_plus_sql` becomes partially vacuous (sql is IN slim now) —
    rewrite it as `slim_plus_grpc` style: assert the added feature resolves
    exactly (sql asserts stay on the slim closure itself via step 7).
    Their per-feature resolution assertions stay unchanged.
13. Update `flavor_marker_table` (lines ~515-545) to the three new bodies.
    Mind the line-collector: format each body single-line per marker in
    Cargo.toml (the whole feature list stays on ONE line) so the table test
    keeps parsing, or rework its parser for multi-line — prefer single-line
    Cargo.toml formatting.
14. Update `default_closure_matches_golden` package-presence loop
    (lines ~259-271): default = flavor-regular now — keep
    `camel-language-js/rhai/xpath/jsonpath/minijinja v` ALL asserted
    present (regular includes them); ADD asserted-ABSENT lines for the
    four principled exclusions (`camel-component-exec`,
    `camel-component-surrealdb`, `camel-function`,
    `camel-component-container`) via `assert_absent`.
15. Regenerate the golden fixture using the procedure documented in the
    file header comment at `feature_profiles.rs:1-16` — run
    `CARGO_TERM_COLOR=never cargo tree -p camel-cli -e features,no-dev` from
    the workspace root and save the normalized output into the fixture file
    exactly as the header describes — so the fixture matches the new default
    (flavor-regular) closure.
16. Gate the container-scheme asserts that regressed with the Tier-2 gate
    (containers is default-off in camel-bundles now):
    - `crates/camel-bundles/tests/parity_test.rs:72`
      (`two_boots_register_identical_sets` asserts `schemes.contains("container")`
      with no cfg gate): cfg-gate the container entries behind
      `#[cfg(feature = "containers")]` (split assert or gated scheme list),
      so default `cargo test -p camel-bundles` is green.
    - `crates/camel-bundles/src/lib.rs` boot-fixture test (~:465-476,
      `boot_registers_all_bundles_from_fixture_config`): same gating for its
      container scheme assertion.
17. In `crates/camel-bundles/src/lib.rs`: first VERIFY (read the security
    boot path) that a configuration requiring the security guard under
    `not(feature = "security")` is rejected with a descriptive error at the
    same entry point the kafka rejection test uses (that test lives at
    `camel-bundles/src/lib.rs:620`; :441-463 is its module header/helpers).
    If production does NOT fail closed there, STOP and report
    `test-design-gap: security omission does not fail closed in camel-bundles`
    AND file a bd issue for the production gap
    (`bd create "<title>" -t bug --deps discovered-from:rc-5t5fo.5` from
    the repo root) — do not fix production behavior in this task. If
    verified, add test `security_omission_fails_closed` gated
    `#[cfg(not(feature = "security"))]` following the kafka-test pattern:
    action — feed the security-requiring configuration to the boot/registry
    entry point; assert an explicit descriptive error variant (the real
    existing one — do not invent); assert it does NOT return Ok with an
    insecure default.

Tests:
- name: regular_closure_satisfies_contract
  setup: Task 1 bodies landed
  command: `cargo test -p camel-cli --test feature_profiles regular_closure_satisfies_contract`
  assert: passes
- name: full_closure_satisfies_contract
  command: `cargo test -p camel-cli --test feature_profiles full_closure_satisfies_contract`
  assert: passes (kafka, surrealdb, function, container, exec reachable in full)
- name: slim_closure_satisfies_contract
  command: `cargo test -p camel-cli --test feature_profiles slim_closure_satisfies_contract`
  assert: passes (mqtt, rhai, jsonpath, sql present; forbidden set absent)
- name: full_covers_universe
  command: `cargo test -p camel-cli --test feature_profiles full_covers_universe`
  assert: passes (every non-axis feature reachable from flavor-full)
- name: default_closure_matches_golden (regenerated)
  command: `cargo test -p camel-cli --test feature_profiles default_closure_matches_golden`
  assert: passes with regenerated fixture; all five language crates asserted
  present, the four principled exclusions asserted absent
- name: security_omission_fails_closed
  command: `cargo test -p camel-bundles --lib security_omission_fails_closed 2>&1 | tee /dev/stderr | grep -c '1 passed'`
  assert: output contains `1 passed` (guards against a vacuous 0-matched
  pass — cargo exits 0 when no test matches the filter); compiled WITHOUT
  the security feature (camel-bundles default already excludes security —
  plain invocation suffices; if the default changes, use
  `--no-default-features`)
- name: whole suite green
  command: `cargo test -p camel-cli --test feature_profiles`
  assert: 0 failures (alias test deleted, marker table + slim_plus_* retargeted)

Acceptance:
- `cargo test -p camel-cli --test feature_profiles` passes in full
- `cargo test -p camel-bundles --lib` passes with default features
- `cargo fmt --check` and `cargo clippy -p camel-cli -p camel-bundles -- -D warnings` exit 0
- `grep -c 'slim_alias_resolves_identically' crates/camel-cli/tests/feature_profiles.rs` returns 0
- `sed -n '/^const SLIM_FORBIDDEN_PREFIXES/,/^\]/p' crates/camel-cli/tests/feature_profiles.rs | grep -ci 'mqtt\|rhai'` returns 0 (anchored to the declaration; a rationale comment above the const is outside the range; mqtt/rhai may appear elsewhere in the file)

- [x] 2

## Task 3: 14-leg build matrix, artifact names, dev-profile input

> POSTSCRIPT (owner ruling post-rc.2, 2026-09-19): this task was EXECUTED
> as written (14 legs, validated green in CI run 35452443811). The owner
> then ruled desktop platforms ship full-only: the matrix is now 12
> entries / 11 uploading (see design.md Decision 3 and the spec deltas
> for the normative topology). The block below is the historical record.

Files:
- `.github/workflows/release-matrix.yml` (modified)
- `.github/workflows/release-dev.yml` (modified)

Steps:
1. In `release-matrix.yml` `on: workflow_call: inputs:`, add
   `dev-profile: {description: "Trim the build matrix to the 3-leg dev subset", type: boolean, default: false}`.
2. Restructure the `build` job `strategy.matrix.include` to 14 legs. Each
   entry carries: `target`, `flavor` (one of `flavor-slim`, `flavor-regular`,
   `flavor-full`), `artifact-name` (explicit string per leg), and
   `in-dev-profile` (boolean). AUXILIARY KEYS RULE: each new leg inherits the
   auxiliary    keys (`os`, `install-librdkafka`, `kafka-probe`, `alloc-features`,
   `bin-suffix`, `use-cross`, `install-musl-tools`) from
   the existing same-target leg, EXCEPT: `kafka-probe` and
   `install-librdkafka` appear on ALL flavor-full LINUX legs — x86_64-gnu
   AND the new ARM-native aarch64-gnu leg (the apt step "Install build deps
   (Linux native)" is what provides librdkafka deps on the ARM runner; the
   old cross image shipped them) — and NEVER on regular/slim legs (a kafka
   probe on regular/slim legs is red — they lack kafka by design); jemalloc
   `alloc-features` stays on ALL musl legs including the new slim legs
   (allocator selection, not closure selection).
   - slim (in-dev-profile: false): `x86_64-unknown-linux-musl`,
     `aarch64-unknown-linux-musl` → artifact-name `camel-slim-<target>`.
   - regular (artifact-name `camel-<target>`, the clean name): all 7
     targets; `in-dev-profile: true` ONLY for `x86_64-unknown-linux-musl`
     and `x86_64-apple-darwin`.
   - full (artifact-name `camel-full-<target>`): `x86_64-unknown-linux-gnu`
     (in-dev-profile: true), `aarch64-unknown-linux-gnu` with
     `os: ubuntu-24.04-arm` (native ARM, replaces cross for this leg),
     `x86_64-apple-darwin`, `aarch64-apple-darwin`,
     `x86_64-pc-windows-msvc` (all in-dev-profile: false).
3. Feature selection: the build step's feature argument becomes the single
   marker. The composed assignment at line ~106 becomes
   `FEATURES="${{ matrix.flavor }}${ALLOC_FEATURES:+,$ALLOC_FEATURES}"` —
   the base selection (previously composed `full`/empty sets per leg) is
   replaced by the marker; the jemalloc append on musl legs STAYS (allocator
   selection, not closure selection). Verify no leg composes closure
   features beyond marker + allocator.
4. Staged FILENAME rename: the binary staging step (line ~178) copies
   `target/<target>/release/camel${bin-suffix}` — rename the staged file to
   `${{ matrix.artifact-name }}${{ matrix.bin-suffix }}` (bin-suffix is empty
   except `.exe` on windows) so the file INSIDE the artifact carries the
   flavor prefix. `Upload artifact` step: `name: ${{ matrix.artifact-name }}`
   (was `camel-${{ matrix.target }}`). Without the staged rename, slim and
   regular musl artifacts collide on merge-multiple download.
5. Add job-level gate on `build`:
   `if: ${{ !inputs.dev-profile || matrix.in-dev-profile }}`.
6. VERIFY (do not rewrite) the existing `--version` flavor-suffix probe
   (lines ~145-157): it must derive the expected suffix from
   `matrix.flavor` minus the `flavor-` prefix for all 14 legs; if it
   hardcodes today's two-flavor mapping, extend it to three flavors.
7. Add a NEGATIVE kafka probe as a SEPARATE matrix key
   `kafka-absence-probe: true` (mirrors the positive kafka-probe pattern at
   lines ~115-143): the step lints a minimal route referencing
   `kafka:test-topic` and EXPECTS failure (exit non-zero with an
   unknown/unregistered-scheme error; VERIFIED ACTUAL SEMANTICS at
   implementation: camel-lint emits an Info `unverified-scheme` note with
   exit 0 — the probe passes iff RC==0 AND the note is present, so a
   broken probe can never read as absence) — proving kafka absence in
   slim/regular artifacts. Put the key on ALL runnable slim/regular legs:
   slim x86_64-musl, regular x86_64-musl, regular x86_64-unknown-linux-gnu,
   regular x86_64-apple-darwin, regular aarch64-apple-darwin,
   regular x86_64-pc-windows-msvc. EXEMPT the aarch64-musl cross legs
   (cross-compiled, not runnable on the runner) — the release-job filename
   assert (Task 4 step 5) is the backstop for those.
8. Extend `closure-check` (lines ~429-430): after the camel-cli
   feature-closure invocation, add
   `cargo test -p camel-bundles --lib security_omission_fails_closed`
   so every dev run re-validates the slim fail-closed security rejection
   (canonical spec R3 letter: the dev suite proves the rejection).
9. In `release-dev.yml`, add `dev-profile: true` to the reusable workflow
   call's `with:` block.
10. Verify no other workflow file references the `camel-<target>` artifact
    names (rg over `.github/workflows/` for `download-artifact` and
    `camel-`): the three bridge-release workflows release jars, not camel
    binaries — confirm and leave untouched.

Tests (structural, executed locally — GitHub validates syntax at run time):
- name: yaml parses
  action: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release-matrix.yml')); yaml.safe_load(open('.github/workflows/release-dev.yml'))"`
  assert: exit 0
- name: 14 legs with flavor + artifact names
  action: python3 script walking `jobs.build.strategy.matrix.include`
  assert: exactly 14 entries; each has `flavor` in
  {flavor-slim, flavor-regular, flavor-full}; `artifact-name` matches
  `^camel(-slim|-full)?-(x86_64|aarch64)(-[a-z0-9_.]+){2,3}$`; counts
  per flavor = 2/7/5; exactly 3 entries with `in-dev-profile: true`
  (musl-regular x86_64, darwin-regular x86_64, gnu-full x86_64); the
  aarch64-gnu full leg has `os: ubuntu-24.04-arm`; the matrix key
  `kafka-probe` appears on exactly the 5 full legs; the matrix key
  `kafka-absence-probe` appears on exactly the 6 runnable slim/regular
  legs listed in step 7 (exact key match — substring matches collide)
- name: negative kafka probe present
  action: python3 walking the build matrix include list
  assert: the `kafka-absence-probe` key is true on exactly the 6 runnable
  slim/regular legs named in Task 3 step 7, false/absent elsewhere; and
  `grep -c 'kafka:test-topic' .github/workflows/release-matrix.yml` ≥ 1
- name: closure-check runs the security rejection test
  action: `grep -c 'cargo test -p camel-bundles --lib security_omission_fails_closed' .github/workflows/release-matrix.yml`
  assert: exactly 1 (the camel-bundles security test wired into
  closure-check)
- name: dev wrapper passes dev-profile
  action: `grep -A5 'release-matrix.yml' .github/workflows/release-dev.yml | grep 'dev-profile: true'`
  assert: exit 0

Acceptance:
- all five structural tests above pass
- `grep -c 'camel-\${{ matrix.target }}' .github/workflows/release-matrix.yml` returns 0
- release-dev.yml still carries the permissions trio ceiling (rc-myx4r
  guard — do not touch the permissions block)

- [x] 3

## Task 4: Docker lockstep and release-job flavor asserts

Files:
- `.github/workflows/release-matrix.yml` (modified — `docker` and `release` jobs)

Steps:
1. Docker matrix (`docker:` job) — artifact keys:
   - production variant: `amd64-artifact: camel-x86_64-unknown-linux-musl`,
     `arm64-artifact: camel-aarch64-unknown-linux-musl` — UNCHANGED (regular
     keeps the clean name).
   - alpine variant: `amd64-artifact: camel-slim-x86_64-unknown-linux-musl`,
     `arm64-artifact: camel-slim-aarch64-unknown-linux-musl`.
   - gnu variant: `amd64-artifact: camel-full-x86_64-unknown-linux-gnu`,
     `arm64-artifact: camel-full-aarch64-unknown-linux-gnu`.
2. Docker dev-mode gating (prevents red dev runs — only 3 artifacts exist
   with `dev-profile: true`): add per-variant `in-dev` key to the docker
   matrix (gnu: `true`; production, alpine: `false`), job-level
   `if: ${{ !inputs.dev-profile || matrix.in-dev }}` on the docker job, and
   in dev mode (`inputs.dev-profile == true`) skip the arm64 download step
   and the arm64 `cp` in `Prepare build context` (there is no separate
   arm64 dev image build — `Build dev image` is already amd64-only; gate
   each consumer of the arm64 binary with the dev-profile condition).
3. Semantic docker tags — the `Docker metadata` (id: meta) output is DEAD
   (nothing references `steps.meta`); the real tag lists are the explicit
   `tags:` rows in the per-arch push steps and the
   `for TAG in "${VERSION}${suffix}" "latest${suffix}"` loop in
   `Create and push multi-arch manifest`. Semantic tags go to the MANIFEST
   LOOP ONLY (per-arch push `tags:` rows stay exactly as they are):
   - production loop: `for TAG in "${VERSION}" "latest" "regular"`.
   - alpine loop: `for TAG in "${VERSION}-alpine" "latest-alpine" "slim"`.
   - gnu loop: `for TAG in "${VERSION}-gnu" "latest-gnu" "full"`.
   Semantic tags are UNSUFFIXED; imagetools create re-tags the combined
   multi-arch list, so per-arch pushes keep their versioned + latest-arch
   tags untouched.
4. Dev smoke assert (`Smoke dev image (assert flavor suffix)`): update the
   expected-flavor case mapping: `production) EXPECTED="regular"`,
   `alpine) EXPECTED="slim"`, `gnu) EXPECTED="full"`.
5. `release` job: between `Download artifacts` and `Create release`, add a
   step `Assert artifact flavor suffixes` — a bash step over `dist/*`
   (downloaded with pattern `camel-*`, merge-multiple) using DISJOINT
   classification (each file matches exactly one class; enumerate the 7
   regular target names explicitly, including
   `camel-x86_64-pc-windows-msvc.exe` with its `.exe`):
   (a) exactly 14 files; (b) 2 matching `camel-slim-<target>`, 5 matching
   `camel-full-<target>`, 7 matching the enumerated regular names;
   (c) no file matches any other shape. Filename-based only —
   cross-compiled binaries are not runnable on the ubuntu runner; runtime
   smoke already happened as the per-build-leg `--version` probe from
   Task 3. Fail the job on mismatch.
6. Leave `files: dist/camel-*` in softprops unchanged (prefix pattern still
   correct).

Tests (structural):
- name: docker artifact keys lockstep
  action: python3 walking `jobs.docker.strategy.matrix.include`
  assert: alpine keys reference `camel-slim-`, gnu keys reference
  `camel-full-`, production keys reference bare `camel-` musl names (no
  slim/full prefix); `in-dev` true on gnu only
- name: semantic docker tags in the manifest loops
  action: grep the three `for TAG` loop lines in
  `Create and push multi-arch manifest`
  assert: production loop contains `"regular"`, alpine loop contains
  `"slim"`, gnu loop contains `"full"`; the per-arch push `tags:` blocks
  are UNCHANGED from their versioned + latest-arch shape
- name: docker dev gating present
  action: `grep -c 'dev-profile' .github/workflows/release-matrix.yml`
  assert: ≥ 3 (docker job if + arm64 skips + build job gate from Task 3)
- name: smoke case mapping updated
  action: `grep -A10 'Smoke dev image' .github/workflows/release-matrix.yml`
  assert: `slim` expected for alpine, `full` for gnu
- name: release asserts 14 artifacts
  action: `grep -B2 -A14 'Assert artifact' .github/workflows/release-matrix.yml`
  assert: step exists between Download artifacts and Create release

Acceptance:
- all five structural tests pass
- `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release-matrix.yml'))"` exits 0
- docker job still carries `if:` publish-gating on every push/login/metadata
  step (unchanged — publish-input gating is canonical spec)

- [x] 4

## Task 5: Prerelease tag guard on the crates.io publish job

Files:
- `.github/workflows/release.yml` (modified)

Steps:
1. On the `publish` job (the crates.io job homed in this file per ADR-0082),
   add `if: ${{ !contains(github.ref_name, '-rc.') }}` alongside its
   existing `needs: call-release-matrix` (do not move the job, do not add
   any inputs.publish conditional — canonical spec forbids it; a tag-name
   guard is the only allowed gate).
2. Add a two-line comment above the `if:` explaining: prerelease smoke tags
   (`v*-rc.*`) exercise matrix/assets/docker but must not publish crates.

Tests (structural):
- name: guard present on publish job only
  action: `grep -n "contains(github.ref_name, '-rc.')" .github/workflows/release.yml`
  assert: exactly 1 hit, inside the `publish:` job block
- name: job stays homed
  action: `grep -c 'environment: crates-io' .github/workflows/release.yml`
  assert: 1 (job still in release.yml — job_workflow_ref constraint)

Acceptance:
- both structural tests pass
- `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml'))"` exits 0
- no `inputs.publish` conditional appears anywhere in release.yml

- [x] 5

## Task 6: Distribution documentation

Files:
- `docs/src/operations/distribution-flavors.md` (new)
- `crates/camel-cli/CONTEXT.md` (modified)

Steps:
1. New `distribution-flavors.md`: table of the three flavors (contents,
  targets, artifact names), the Docker tag map (`latest`/`:regular`,
  `-alpine`/`:slim`, `-gnu`/`:full`), install guidance per channel
  (release download, docker, `cargo install camel-cli` = regular source
  build, `--features flavor-full` for everything, composition beyond
  presets), a BREAKING-CHANGE section (`camel-<target>` re-aliases from
  the historical full-ish closure to regular at 0.50 — one-time; this
  section is the canonical callout text the 0.50 release notes will
  reference), the ITERATION POLICY (flavors are presets, not walls;
  additions flow down freely in minors — full→regular→slim is
  non-breaking; removals only at majors), and the HOW-TO-MOVE-A-FEATURE
  RECIPE: (1) edit the ONE flavor list in camel-cli/Cargo.toml where the
  feature should start appearing (bodies are chained), (2) update the
  contract prefix sets in feature_profiles.rs if a principle boundary is
  crossed, (3) regenerate the golden fixture with the documented one-line
  command, (4) update the flavor table in this doc; CI matrix legs never
  change (legs are target×flavor).
2. Update `crates/camel-cli/CONTEXT.md` flavor-markers section: the three
  bodies as landed (slim = edge pack
  mqtt/mqtt-tls/http-static/sql/lang-jsonpath/lang-rhai; regular = slim + all
  capabilities except the four principled exclusions; full = regular +
  exec/kafka/surrealdb/containers), alias removal
  note (slim-http/slim-benchmarks dropped at 0.50 per rc-n6iop).
3. Cross-link from `docs/src/operations/oidc-publish-fallback.md` ONLY if it
   mentions artifact names (check; it should not — do not add unrelated
   links).

Tests:
- name: context citations lint
  command: `cargo xtask lint-context-citations`
  assert: exit 0
- name: docs name all three artifacts
  action: `grep -c 'camel-slim-\|camel-full-' docs/src/operations/distribution-flavors.md`
  assert: ≥ 4 (naming table + breaking-change section)

Acceptance:
- both tests pass
- the doc is listed in the operations index/summary if one exists (check
  `docs/src/operations/` neighbors for an index file; add entry if present)

- [x] 6
