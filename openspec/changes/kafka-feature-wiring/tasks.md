# Tasks: kafka-feature-wiring

## camel-cli feature surface

### Task 1: Collapse the kafka feature surface to `kafka` + `dynamic-linking`

**Files:**
- `crates/camel-cli/Cargo.toml` (modified)

**Steps:**
1. Replace the four-line comment block above the `kafka` feature (it
   starts with "kafka activates the kafka component code only" and ends
   with the `kafka-static` sentence) with:
   "kafka is the capability feature: activates the kafka component and
   its registration in the lint registry and the boot cascade.
   librdkafka builds from source via cmake (the rdkafka-sys default).
   dynamic-linking selects a system librdkafka instead and implies
   kafka."
2. Keep `kafka = ["dep:camel-component-kafka", "camel-bundles/kafka"]`
   unchanged.
3. Replace `dynamic-linking = ["camel-component-kafka/dynamic-linking"]`
   with `dynamic-linking = ["kafka", "camel-component-kafka/dynamic-linking"]`.
4. Delete the `cmake-build = ["camel-component-kafka/cmake-build"]` line.
5. Delete the `kafka-static = ["dep:camel-component-kafka", "cmake-build"]`
   line.

**Tests:** (shell-executable from the workspace root)
- `dynamic-linking builds stay resolvable`: after the edits,
  `cargo tree -p camel-cli --no-default-features --features dynamic-linking -e features,no-dev --locked`
  exits 0 and its output contains `camel-component-kafka`. (This also
  holds at base — the slash syntax already activates the optional dep —
  so it is a sanity check, not the change detector.)
- `removed names rejected` (THE change detector): after the edits,
  `cargo tree -p camel-cli --features cmake-build --locked` exits
  non-zero with stderr naming `cmake-build`, and
  `cargo tree -p camel-cli --features kafka-static --locked` exits
  non-zero with stderr naming `kafka-static`. (Both exit 0 at base.)

**Acceptance:**
- The two shell checks above hold as stated
- `cargo tree -p camel-cli --features kafka --locked` still exits 0
- `cargo xtask lint-gate-forwarding` exits 0
- No diff under `crates/components/camel-kafka/` (component crate
  untouched)

- [x] 1.1

## camel-cli tests

### Task 2: Feature-table and closure tests for the collapsed kafka surface

**Files:**
- `crates/camel-cli/tests/feature_profiles.rs` (modified)

**Steps:**
1. Add helper `fn tree_fails_with(extra_args: &[&str], fragment: &str)`
   that spawns the exact same `cargo tree -p camel-cli -e
   features,no-dev --prefix none --locked <extra_args>` command as
   `tree_lines` (same `CARGO_TERM_COLOR=never` env,
   `current_dir(workspace_root())`), asserts `!output.status.success()`,
   and asserts the stderr (via `String::from_utf8_lossy`) contains
   `fragment`; on either failed expectation it panics with the status
   and stderr captured.
2. Add test `kafka_feature_table_implies_capability`: read
   `crates/camel-cli/Cargo.toml` to a String (relative to
   `workspace_root()`), then assert the raw text contains the exact
   line `dynamic-linking = ["kafka", "camel-component-kafka/dynamic-linking"]`,
   and assert no line of the `[features]` section starts with
   `cmake-build =` or `kafka-static =`. (The implication is asserted at
   the feature-table level because feature-forwarding edges never
   render in cargo tree — documented at feature_profiles.rs:183-186 —
   a closure-based implication test is vacuous.)
3. Add test `dynamic_linking_closure_resolves_kafka`:
   `let lines = tree_lines(&["--no-default-features", "--features", "dynamic-linking"]);`
   then assert some line contains `camel-component-kafka`, and — using
   the existing `assert_absent` helper with the constant list
   `SLIM_FORBIDDEN_PREFIXES` minus its `camel-component-kafka` entry —
   that the remaining controllable set (including tower-lsp and the
   excluded language-runtime crates) is absent.
4. Add test `removed_kafka_feature_names_rejected`: calls
   `tree_fails_with(&["--features", "cmake-build"], "cmake-build")` and
   `tree_fails_with(&["--features", "kafka-static"], "kafka-static")`.
5. Do NOT regenerate or edit the golden fixture; do NOT touch
   `default_closure_matches_golden`.

**Tests:**
- `kafka_feature_table_implies_capability`: task 1 landed → read the
  camel-cli Cargo.toml feature table → `dynamic-linking` list includes
  `kafka`; `cmake-build`/`kafka-static` keys absent;
  command: `cargo test -p camel-cli --test feature_profiles kafka_feature_table_implies_capability`;
  expected: fails before task 1 (table lacks the implication), passes
  after.
- `dynamic_linking_closure_resolves_kafka`: task 1 landed →
  tree_lines over `--no-default-features --features dynamic-linking` →
  camel-component-kafka present, remaining controllable set absent;
  command: `cargo test -p camel-cli --test feature_profiles dynamic_linking_closure_resolves_kafka`;
  expected: green before and after (sanity, not change detector).
- `removed_kafka_feature_names_rejected`: task 1 landed → cargo tree
  with each removed feature name → non-zero exit naming the feature;
  command: `cargo test -p camel-cli --test feature_profiles removed_kafka_feature_names_rejected`;
  expected: fails before task 1 (names still resolve), passes after.
- `default_closure_matches_golden` (EXISTING, unmodified): default
  closure unchanged, no fixture regen;
  command: `cargo test -p camel-cli --test feature_profiles default_closure_matches_golden`;
  expected: green before and after.
- camel-bundles gating tests (EXISTING, unmodified; own the
  kafka-less-legs scenario). Both polarity invocations are required —
  the positive test is cfg-gated behind the feature and compiles only
  when enabled (documented at camel-bundles/src/lib.rs:469-472):
  `cargo test -p camel-bundles --lib boot_feature_gating` (negative
  polarity) and
  `cargo test -p camel-bundles --features kafka --lib boot_feature_gating`
  (positive polarity); expected: both green before and after.

**Acceptance:**
- `cargo test -p camel-cli --test feature_profiles` passes (all
  functions, existing and new)
- `cargo test -p camel-bundles --lib boot_feature_gating` passes AND
  `cargo test -p camel-bundles --features kafka --lib boot_feature_gating`
  passes (each runs its polarity's tests; "0 tests" output for either
  invocation is a failure of this criterion)
- `cargo clippy -p camel-cli --all-targets -- -D warnings` exits 0
- `cargo fmt --check --all` exits 0

- [x] 2.1

## release workflow

### Task 3: Kafka legs build with the capability feature + native probe

**Files:**
- `.github/workflows/release.yml` (modified)

**Steps:**
1. In the build matrix, change `kafka-features: "cmake-build"` to
   `kafka-features: "kafka"` on exactly five legs:
   x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu,
   x86_64-apple-darwin, aarch64-apple-darwin, x86_64-pc-windows-msvc.
   The two musl legs are unchanged.
2. Add matrix key `kafka-probe: true` to exactly the four
   native-executable kafka legs (x86_64-unknown-linux-gnu, both macOS
   legs, x86_64-pc-windows-msvc). Do NOT add it to
   aarch64-unknown-linux-gnu (cross-built; the probe cannot execute
   there) or to any musl leg.
3. Insert a step `Assert kafka capability (release probe)` immediately
   after the `Build binary` step, gated on the matrix key. Write the
   fixture with `printf '%s\n'` lines — NOT a heredoc (a quoted
   heredoc terminator must sit flush-left, and YAML block-indentation
   stripping removes that, breaking the script at release time). The
   fixture endpoint carries `brokers=` so a registered component lints
   clean. The normative gate is two-part and both parts are
   load-bearing: exit code 0 AND no `unverified-scheme` diagnostic in
   the lint output (the matcher keys on the diagnostic CODE, not
   word co-occurrence: diagnostics never name the scheme on the
   header line, and ANSI escapes split the echoed scheme token —
   the fixture's only capability-gated scheme is kafka, so an
   `unverified-scheme` hit names kafka by construction):
   ```yaml
   - name: Assert kafka capability (release probe)
     if: matrix.kafka-probe == true
     shell: bash
     run: |
       set -euo pipefail
       FIXTURE="$RUNNER_TEMP/kafka-probe-route.yaml"
       printf '%s\n' \
         'routes:' \
         '  - id: "kafka-capability-probe"' \
         '    from: "kafka:orders?brokers=localhost:9092"' \
         '    steps:' \
         '      - to: "log:out"' \
         > "$FIXTURE"
       BIN="target/${{ matrix.target }}/release/camel${{ matrix.bin-suffix }}"
       set +e
       OUT="$("$BIN" lint "$FIXTURE" 2>&1)"
       RC=$?
       set -e
       if [ "$RC" -ne 0 ]; then
         echo "FAIL: kafka capability probe exited $RC"
         printf '%s\n' "$OUT"
         exit 1
       fi
       if printf '%s\n' "$OUT" | grep -iq 'unverified-scheme'; then
         echo "FAIL: unverified-scheme diagnostic in a kafka-capability build"
         printf '%s\n' "$OUT"
         exit 1
       fi
       echo "OK: kafka capability verified on ${{ matrix.target }}"
   ```
4. Update the comment block above the feature-composition `run:` block
   (the one explaining empty/absent matrix keys) so it describes
   `kafka-features` as carrying the capability feature, removing any
   cmake-build wording that remains.
5. Confirm no `cmake-build` or `kafka-static` string remains anywhere
   in the file.

**Tests:** (static, executable from the workspace root)
- `workflow-refs-clean`: `grep -cE 'cmake-build|kafka-static' .github/workflows/release.yml`
  prints `0`; expected: 5+ hits before, 0 after.
- `workflow-kafka-legs`: `grep -c 'kafka-features: "kafka"' .github/workflows/release.yml`
  prints `5`.
- `workflow-probe-gate`: `grep -c 'if: matrix.kafka-probe == true' .github/workflows/release.yml`
  prints `1`; `grep -c 'brokers=localhost:9092' .github/workflows/release.yml`
  prints `1`; `grep -A28 'Assert kafka capability' .github/workflows/release.yml`
  shows the printf fixture lines, the `$BIN lint` invocation, and both
  failure branches (non-zero exit check and the kafka/unverified
  output check).
- `workflow-cross-leg-excluded`: `grep -A8 'target: aarch64-unknown-linux-gnu' .github/workflows/release.yml`
  output contains `use-cross: true` and does NOT contain
  `kafka-probe: true`.
- `workflow-yaml-parses`:
  `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml'))"`
  exits 0.
- `probe-polarity-local` (pre-merge, one-time, run in the worktree
  with `RUNNER_TEMP` set to any temp dir — the script runs under
  `set -u`): build both polarities and run the exact probe script
  against each:
  `cargo build -p camel-cli --release --no-default-features --features slim-http`
  then `cargo build -p camel-cli --release --no-default-features --features kafka`
  (source build; needs cmake + libssl-dev + libsasl2-dev + libcurl4-openssl-dev
  on the host). The kafka-less binary MUST trip the unverified branch,
  the kafka binary MUST pass both gate parts. This validates the probe
  logic itself before it ever runs on a release tag.

**Acceptance:**
- All five static checks above hold
- `probe-polarity-local` demonstrates both polarities behave as
  specified (fail-open is impossible, healthy binary passes)
- The docker publish matrix keys (`amd64-artifact`, `arm64-artifact`)
  are untouched by this change

- [x] 3.1

## docs

### Task 4: CONTEXT.md build-profiles note + repo-wide residue check

**Files:**
- `crates/camel-cli/CONTEXT.md` (modified)

**Steps:**
1. In `## Build profiles` → the `**Feature profiles.**` paragraph, after
   the sentence introducing the controllable optional set (which lists
   kafka), append: "The kafka surface is exactly two features: `kafka`
   (capability — component activation plus registration in the lint
   registry and the boot cascade; librdkafka source build) and
   `dynamic-linking` (capability plus system-librdkafka linking; it
   implies `kafka`). The historical `cmake-build` and `kafka-static`
   names were removed (bd rc-5t5fo.1): they activated the dependency
   without enabling registration."
2. Run the residue check and fix any straggler outside the sanctioned
   sites (adjudicated during implementation: the mandated CONTEXT.md
   migration note and the task-2 rejection test necessarily name the
   removed features — the check asserts the hit list equals the
   sanctioned set exactly). The check must use `--hidden` (rg skips
   dot-dirs like `.github` by default) and exclude all of
   `openspec/**` (post-archive, the synced canonical spec legitimately
   quotes the removed names).

**Tests:**
- `no-unsanctioned-kafka-static-residue`:
  `rg -l --hidden 'kafka-static' --glob '!docs/audits' --glob '!openspec/**' --glob '!.git'`
  from the workspace root → exactly the sanctioned sites:
  `crates/camel-cli/tests/feature_profiles.rs` (removal-rejection
  test) and `crates/camel-cli/CONTEXT.md` (the mandated migration
  note).
- `cmake-build-only-sanctioned`:
  `rg -l --hidden 'cmake-build' --glob '!docs/audits' --glob '!openspec/**' --glob '!.git'`
  → exactly the two sanctioned sites above plus
  `crates/components/camel-kafka/Cargo.toml` (the component's own
  rdkafka forwarding features).
- `context-citations-gate`: `cargo xtask lint-context-citations` exits 0.

**Acceptance:**
- Both residue greps hold exactly as stated
- `cargo xtask lint-context-citations` exits 0
- The edited CONTEXT.md section is in English
- The change's final squash-commit body documents the removed
  `cmake-build`/`kafka-static` feature names as the breaking note the
  risk budget promises (the changelog is commit-generated)

- [x] 4.1
