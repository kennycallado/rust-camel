# Tasks: flavor-markers

## camel-cli feature surface

### Task 1: Flavor marker features + default flip

**Files:**
- `crates/camel-cli/Cargo.toml` (modified)

**Steps:**
1. In the `[features]` section, directly above the `slim-http` line, add
   a comment block: flavor markers are the single selection surface for
   profiles; priority when several are enabled is full > regular >
   slim; builds with no marker report `custom`; bodies in this change
   are aliases only (content curation is the flavor-matrix change).
2. Add `flavor-slim = ["slim-http"]`.
3. Add `flavor-regular = ["full"]`.
4. Add `flavor-full = ["full", "kafka"]`.
5. Change `default = ["full"]` to `default = ["flavor-regular"]`.
6. Touch nothing else in the section (`full`, `slim-http`, `kafka`,
   `dynamic-linking` lines byte-identical).

**Tests:** (shell, from workspace root, before/after the edits; every
invocation uses `--prefix none` — default tree glyphs would make field
extraction compare box-drawing characters, not packages)
- `marker-closure-equivalence`: after the edits,
  `cargo tree -p camel-cli --no-default-features --features flavor-full -e no-dev --prefix none --locked`
  and
  `cargo tree -p camel-cli --no-default-features --features full,kafka -e no-dev --prefix none --locked`
  print identical package sets (compare sorted `awk '{print $1}' |
  sort -u` outputs; diff must be empty). Same check for
  `--no-default-features --features flavor-regular` vs
  `--no-default-features --features full`, and
  `--no-default-features --features flavor-slim` vs
  `--no-default-features --features slim-http`.
- `default-tree-stable`: `cargo tree -p camel-cli --locked -e no-dev --prefix none`
  output is identical before and after the default flip (capture
  before the edit, diff after).

**Acceptance:**
- Both shell checks hold as stated
- `cargo tree -p camel-cli --features flavor-full --locked` exits 0
- The `[features]` diff shows exactly: one modified line (`default`),
  three added `flavor-*` lines, and the new comment block — no other
  line changed

- [x] 1.1

## version wiring

### Task 2: FLAVOR const + version suffix on the interactive CLI

**Files:**
- `crates/camel-cli/src/main.rs` (modified)

**Steps:**
1. Add to `main.rs`:
   ```rust
   /// Compile-time flavor from the marker features. Priority:
   /// full > regular > slim; no marker enabled means a raw
   /// composition, reported as `custom`.
   pub const FLAVOR: &str = if cfg!(feature = "flavor-full") {
       "full"
   } else if cfg!(feature = "flavor-regular") {
       "regular"
   } else if cfg!(feature = "flavor-slim") {
       "slim"
   } else {
       "custom"
   };

   fn version_line() -> String {
       format!("{} ({})", env!("CARGO_PKG_VERSION"), FLAVOR)
   }
   ```
2. Change the clap derive's `version` attribute on the `Cli` struct
   (currently `version,` at approximately line 15, which pulls the bare
   crate version) to `version = version_line(),`.
3. Do NOT touch `crates/camel-cli/src/compile/` (the compiled-artifact
   runtime prints bare `RUNTIME_VERSION` from
   `compile/manifest.rs::RUNTIME_VERSION`; the manifest must stay
   semver-only — spec scenario pins it).

**Tests:**
- `version_line_matches_contract` (Rust unit test in a
  `#[cfg(test)] mod tests` at the end of `main.rs`):
  - setup: task 1 landed (markers exist).
  - action: assert `version_line() == format!("{} ({})", env!("CARGO_PKG_VERSION"), FLAVOR)`
    and that `version_line()` ends with `" (" ` joined by the flavor
    word and `")"`, i.e. matches the pattern `<semver> (<word>)` where
    `<word>` is one of `full|regular|slim|custom` (use a manual
    string check: `version_line().ends_with(&format!(" ({FLAVOR})"))`
    plus `version_line().starts_with(env!("CARGO_PKG_VERSION"))`).
  - command: `cargo test -p camel-cli --bin camel version`
  - expected: green after task 2; fails to compile before (FLAVOR
    undefined).
- `version_polarity_local` (shell, one-time, after tasks 1+2; the
  `custom` polarity needs `--no-default-features --features slim-http`
  so no marker is on; slim needs the marker):
  - `cargo run -p camel-cli --quiet -- --version` prints
    `<semver> (regular)`;
  - `cargo run -p camel-cli --quiet --no-default-features --features flavor-slim -- --version` prints `<semver> (slim)`;
  - `cargo run -p camel-cli --quiet --no-default-features --features slim-http -- --version` prints `<semver> (custom)`.
  - The `(full)` polarity is not built locally (rdkafka cost); it is
    owned by the release-leg version probe (task 4) and the closure
    equivalence test (task 3).
- `artifact-version-untouched` (shell): `git diff --name-only` for
  this task's commit contains no path under
  `crates/camel-cli/src/compile/`;
  `cargo test -p camel-cli --test compiled_artifact_test artifact_help_and_version_exit_zero`
  exits 0 (the executable pin for the bare artifact `--version`) and
  `cargo test -p camel-cli --test compiled_artifact_test trailer_free_binary_keeps_normal_cli`
  exits 0.

**Acceptance:**
- `cargo test -p camel-cli --bin camel version` passes
- All three `version_polarity_local` lines print the stated suffixes
- No diff under `crates/camel-cli/src/compile/`
- `cargo fmt --check --all` and
  `cargo clippy -p camel-cli --all-targets -- -D warnings` exit 0

- [x] 2.1

## feature-graph tests

### Task 3: Marker table and closure-equivalence tests

**Files:**
- `crates/camel-cli/tests/feature_profiles.rs` (modified)

**Steps:**
1. Add test `flavor_marker_table`: read `crates/camel-cli/Cargo.toml`;
   assert the raw text contains the exact lines
   `flavor-slim = ["slim-http"]`,
   `flavor-regular = ["full"]`,
   `flavor-full = ["full", "kafka"]`, and
   `default = ["flavor-regular"]`.
2. Add test `flavor_full_closure_equals_full_plus_kafka`:
   `let a = tree_lines(&["--no-default-features", "--features", "flavor-full"]);`
   `let b = tree_lines(&["--no-default-features", "--features", "full,kafka"]);`
   `assert_eq!(a, b);` (tree_lines already normalizes, sorts, dedups).
3. Add test `flavor_regular_closure_equals_full`:
   same pattern, `flavor-regular` vs `full`.
4. Add test `flavor_slim_closure_equals_slim_http`:
   same pattern, `--no-default-features --features flavor-slim` vs
   `--no-default-features --features slim-http`.
5. Do NOT modify `default_closure_matches_golden`, the kafka
   exact-set test, or the golden fixture.

**Tests:**
- `flavor_marker_table`: setup task 1 landed; action: exact-line
  assertions over the manifest text; command:
  `cargo test -p camel-cli --test feature_profiles flavor_marker_table`;
  expected: fails before task 1, passes after.
- `flavor_full_closure_equals_full_plus_kafka`: setup task 1;
  action/assert: package-set equality via tree_lines Vec equality;
  command:
  `cargo test -p camel-cli --test feature_profiles flavor_full_closure_equals_full_plus_kafka`;
  expected: passes after task 1 (equivalence is the alias invariant).
- `flavor_regular_closure_equals_full` and
  `flavor_slim_closure_equals_slim_http`: same shape; commands
  analogous; expected: pass after task 1.
- `default_closure_matches_golden` (EXISTING, unmodified): command
  `cargo test -p camel-cli --test feature_profiles default_closure_matches_golden`;
  expected: green before and after (fixture NOT regenerated).

**Acceptance:**
- `cargo test -p camel-cli --test feature_profiles` passes (existing
  8 + new 4)
- Golden fixture file untouched (`git diff` shows no fixture change)
- `cargo clippy -p camel-cli --all-targets -- -D warnings` and
  `cargo fmt --check --all` exit 0

- [x] 3.1

## release workflow

### Task 4: Single-feature legs + version flavor probe

(setup: tasks 1 and 2 landed — the legs build with marker features
and the `--version` suffix exists)

**Files:**
- `.github/workflows/release.yml` (modified)

**Steps:**
1. Rename the matrix key `kafka-features` to `flavor` on the five
   kafka legs with value `"flavor-full"` (x86_64-gnu, aarch64-gnu,
   x86_64-apple-darwin, aarch64-apple-darwin, x86_64-pc-windows-msvc).
2. Add `flavor: "flavor-regular"` to the two musl legs
   (x86_64-musl, aarch64-musl); their `alloc-features: "jemalloc"`
   keys stay.
3. In the `Build binary` step, replace the sed composition block with:
   ```yaml
   env:
     FLAVOR: ${{ matrix.flavor }}
     ALLOC_FEATURES: ${{ matrix.alloc-features }}
   run: |
     # Single flavor marker per leg (single source of truth, no
     # composed feature lists); allocator joins with a plain
     # conditional expansion — empty ALLOC_FEATURES must not leave
     # a leading comma (rc-vnm8 class).
     FEATURES="${FLAVOR}${ALLOC_FEATURES:+,$ALLOC_FEATURES}"
     echo "Building ${{ matrix.target }} with: --features $FEATURES"
     if [[ "${{ matrix.use-cross }}" == "true" ]]; then
       cross build -p camel-cli --release --target "${{ matrix.target }}" --features "$FEATURES"
     else
       cargo build -p camel-cli --release --target "${{ matrix.target }}" --features "$FEATURES"
     fi
   ```
   (the root manifest is a virtual workspace — `-p camel-cli` is
   mandatory and MUST survive the rewrite). Only the FEATURES
   computation and the matrix key change; every other flag of the
   current build command (`-p camel-cli`, `--release`, `--target`)
   is preserved verbatim.
4. Add a step `Assert version flavor` immediately after the kafka
   probe step, gated on natively-executable legs
   (`if: matrix.use-cross != true`):
   ```yaml
   - name: Assert version flavor
     if: matrix.use-cross != true
     shell: bash
     run: |
       set -euo pipefail
       BIN="target/${{ matrix.target }}/release/camel${{ matrix.bin-suffix }}"
       EXPECTED="${{ matrix.flavor }}"
       EXPECTED="${EXPECTED#flavor-}"
       OUT="$("$BIN" --version)"
       printf '%s\n' "$OUT"
       case "$OUT" in
         *" ($EXPECTED)") ;;
         *) echo "FAIL: expected flavor suffix ($EXPECTED)"; exit 1 ;;
       esac
   ```
5. Leave `kafka-probe`, `install-librdkafka`, `install-musl-tools`,
   `use-cross`, jemalloc assert, docker publish keys untouched.

**Tests:** (static, from workspace root)
- `sed-composition-gone`:
  `grep -c 's/,,/,/g' .github/workflows/release.yml` prints `0`.
- `flavor-full-legs`:
  `grep -c 'flavor: "flavor-full"' .github/workflows/release.yml`
  prints `5`.
- `flavor-regular-musl`:
  `grep -c 'flavor: "flavor-regular"' .github/workflows/release.yml`
  prints `2`.
- `kafka-features-key-gone`:
  `grep -c 'kafka-features' .github/workflows/release.yml` prints `0`.
- `version-probe-gate`:
  `grep -c 'if: matrix.use-cross != true' .github/workflows/release.yml`
  prints `1` (the Assert version flavor step; the kafka probe keeps
  its own `matrix.kafka-probe == true` gate —
  `grep -c 'if: matrix.kafka-probe == true'` stays `1`).
- `workflow-yaml-parses`:
  `npx js-yaml .github/workflows/release.yml > /dev/null && echo OK`
  prints `OK` (or python3 yaml.safe_load on a pip-capable host).
- `case-glob-bash-n`: extract the `Assert version flavor` run block
  and `bash -n` it (syntax check only).

**Acceptance:**
- All static checks hold as stated
- `kafka-probe` gate count unchanged (`1`), docker publish keys
  untouched (`git diff` shows no lines under the docker publish job)
- The build command still carries any `--no-default-features` or
  profile flags the current block has (only FEATURES computation and
  the matrix key change)

- [x] 4.1

## docs

### Task 5: CONTEXT.md flavor-marker documentation

**Files:**
- `crates/camel-cli/CONTEXT.md` (modified)

**Steps:**
1. In `## Build profiles` / the `**Feature profiles.**` paragraph:
   amend the opening clause `default = ["full"]` to
   `default = ["flavor-regular"]` (closure-identical to the
   historical `["full"]`).
2. After the sentence introducing `slim-http`, add: the flavor markers
   `flavor-slim`, `flavor-regular`, `flavor-full` are the single
   selection surface (aliases today; content curation is the
   flavor-matrix change); overlapping markers report by priority
   full > regular > slim; a raw composition with no marker reports
   `custom`; `camel --version` prints the flavor as a suffix
   (`camel 0.48.0 (regular)`) while the compiled-artifact manifest
   stays semver-only.
3. English prose only.

**Tests:**
- `context-citations-gate`: `cargo xtask lint-context-citations`
  exits 0.
- `docs-mention-markers`:
  `grep -c 'flavor-regular' crates/camel-cli/CONTEXT.md` prints `>= 1`.

**Acceptance:**
- Both checks hold
- No other doc site claims to define the flavor markers (spot-check
  CONTEXT-MAP.md and GLOSSARY.md mention nothing conflicting:
  `rg -n 'flavor-slim|flavor-regular|flavor-full' CONTEXT-MAP.md docs/ 2>/dev/null | wc -l`
  — pre-existing hits allowed, new conflicting definitions not;
  record the count in the task report)

- [x] 5.1
