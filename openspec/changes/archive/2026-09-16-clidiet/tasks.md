# Tasks: clidiet

## Phase 1: Allocator dedup

### Task 1.1: Profile test scaffolding — golden snapshot + allocator-graph tests

**Files:**
- `crates/camel-cli/tests/feature_profiles.rs` (new)
- `crates/camel-cli/tests/fixtures/default-deptree.txt` (new)

**Steps:**
1. Generate the golden fixture from the CURRENT tree (pre-dedup, default features) with exactly: `cargo tree -p camel-cli -e features,no-dev --prefix none --locked | sed -E 's| \(/[^)]*\)||g; s| \(\*\)||g; s| \[\*\]||g' | sort -u > crates/camel-cli/tests/fixtures/default-deptree.txt` (run from the workspace root; the worktree must still be at the spec-bless commit with zero code diffs — assert `git status --porcelain -- crates Cargo.lock` is empty first, so the current tree IS the base tree).
2. Create `crates/camel-cli/tests/feature_profiles.rs` with a helper `fn tree_lines(extra_args: &[&str]) -> Vec<String>` that runs `cargo tree -p camel-cli -e features,no-dev --prefix none --locked <extra_args>` with `current_dir` set to the workspace root (two levels above `env!("CARGO_MANIFEST_DIR")`), `CARGO` resolved from the `CARGO` env var falling back to `cargo`, applies the SAME sed normalization in Rust (strip ` (/...)` paren groups anywhere in the line, strip trailing ` (*)` and ` [*]`), sorts and dedups. Cargo-tree failure (non-zero exit) must panic with the captured stderr.
3. Write test `default_closure_matches_golden`: tree_lines(&[]) equals the lines of `tests/fixtures/default-deptree.txt` (both sorted; compare as sets with a diff-pretty error listing first 20 differing lines).
4. Write test `mimalloc_stack_absent_under_all_features`: tree_lines(&["--all-features"]) contains no line starting with `mimalloc v` and no line starting with `libmimalloc-sys v` (lines starting with `tikv-jemallocator` are EXPECTED and ignored — jemalloc is the surviving override).
5. Write test `default_build_has_no_allocator_crate`: tree_lines(&[]) contains no line starting with `mimalloc v`, `libmimalloc-sys v`, `tikv-jemallocator v`, or `tikv-jemalloc-sys v`.

**Tests:** (these ARE the task's content)
- `default_closure_matches_golden`: golden fixture generated in step 1 → run test → PASSES on the unmodified tree (guards Phase 2).
- `mimalloc_stack_absent_under_all_features`: pre-dedup tree → run test → FAILS (mimalloc v0.1.x present under --all-features) — this is the red test Task 1.2 turns green.
- `default_build_has_no_allocator_crate`: unmodified tree → PASSES (audit fact: zero allocator crates under default features).

**Acceptance:**
- `cargo test -p camel-cli --test feature_profiles` runs: 2 pass (`default_closure_matches_golden`, `default_build_has_no_allocator_crate`), 1 fail (`mimalloc_stack_absent_under_all_features`) — the exact red/green split documented here.
- `cargo fmt --check` and `cargo clippy -p camel-cli --all-targets -- -D warnings` exit 0.

- [x] 1.1

### Task 1.2: Remove the mimalloc feature, dependency, and cfg block; regenerate Cargo.lock

**Files:**
- `crates/camel-cli/Cargo.toml` (modified)
- `crates/camel-cli/src/main.rs` (modified)
- `Cargo.lock` (modified, regenerated)

**Steps:**
1. In `crates/camel-cli/Cargo.toml`: delete the `mimalloc = { version = "0.1", optional = true }` dependency line and the `mimalloc = ["dep:mimalloc"]` feature line. Update the feature-table comment above the `jemalloc` line: the override set is now single-path (jemalloc only); keep the existing "if both are enabled, jemalloc wins" historical note only if reworded to past tense, or drop it — there is no second feature anymore.
2. In `crates/camel-cli/src/main.rs`: delete the `#[cfg(all(feature = "mimalloc", not(feature = "jemalloc")))]` block (the `#[global_allocator] static ALLOC: mimalloc::MiMalloc` item) and any comment line that names mimalloc. The jemalloc `#[global_allocator]` block stays byte-identical; its header comment was reworded during execution (r_glm t1.2 review, disclosed deviation): the stale "if both features are enabled, jemalloc wins" sentence became "jemalloc is the single override path; default builds use the system allocator".
3. Prune the lockfile from the workspace root: run `cargo check -p camel-cli --all-features` (this rewrites Cargo.lock), then verify with `git diff Cargo.lock` that the diff removes EXACTLY camel-cli's `"mimalloc",` dependency-list edge and nothing else — the `[[package]] mimalloc`/`libmimalloc-sys` entries STAY (the benchmarks fixture `rust-camel-lib` keeps its own independent optional mimalloc dep; untouchable zone).
4. Run the Task 1.1 test suite; `mimalloc_stack_absent_under_all_features` now passes.

**Tests:**
- `mimalloc_stack_absent_under_all_features`: after removal → `cargo test -p camel-cli --test feature_profiles` exits 0 with all three tests passing.
- `git diff Cargo.lock` shows only camel-cli's `"mimalloc",` edge removed (worker reports the diffstat; conductor verifies).
- `cargo tree -p camel-cli --all-features -i mimalloc` errors with `did not match any packages` (no camel-cli path reaches mimalloc anymore).

**Acceptance:**
- `cargo test -p camel-cli --test feature_profiles` — 3/3 pass.
- `rg -n "mimalloc" crates/camel-cli/Cargo.toml crates/camel-cli/src/` returns zero hits (the tests/ fixture legitimately keeps the crate-name string).
- `cargo build -p camel-cli --release` exits 0 (default features unchanged).
- `cargo fmt --check` and `cargo clippy -p camel-cli --all-targets -- -D warnings` exit 0.

- [x] 1.2

## Phase 2: Slim feature profile

### Task 2.1: Slim exclusion profile tests (red first)

**Files:**
- `crates/camel-cli/tests/feature_profiles.rs` (modified)

**Steps:**
1. Add test `slim_closure_excludes_controllable_set`: `tree_lines(&["--no-default-features"])` contains NO line starting with: `camel-component-kafka v`, `camel-component-grpc v`, `camel-component-wasm v`, `camel-component-llm v`, `camel-component-mcp v`, `camel-component-mqtt v`, `camel-component-surrealdb v`, `camel-component-exec v`, `camel-lsp v`, `tower-lsp v`, `camel-language-js v`, `camel-language-rhai v`, `camel-language-jsonpath v`, `camel-language-xpath v`. (AMENDED during 2.2 execution: `camel-language-minijinja v` dropped from the forbidden set — camel-template hard-depends on it via non-optional out-of-lease paths; deferred family. The `lang-minijinja` forward feature stays for full parity.) `ariadne v` MAY appear (camel lint keeps it non-optional). Assert each forbidden prefix individually so a failure names the offending crate.
2. Add test `slim_plus_grpc_resolves_grpc_only`: `tree_lines(&["--no-default-features", "--features", "slim-http,grpc"])` DOES contain `camel-component-grpc v` and `tonic v`, and still contains NONE of the other thirteen forbidden prefixes from step 1 (the amended set without minijinja).
3. Run the suite: `slim_closure_excludes_controllable_set` FAILS on the current tree naming `camel-lsp v` and `camel-language-* v` (non-optional today); `slim_plus_grpc_resolves_grpc_only` fails earlier with cargo's `unknown feature \"slim-http\"` error (the feature does not exist yet) — both are the expected red state for Task 2.2.

**Tests:**
- `slim_closure_excludes_controllable_set`: current tree → FAILS naming `camel-lsp v` / `camel-language-* v` (red state for Task 2.2).
- `slim_plus_grpc_resolves_grpc_only`: current tree → FAILS (same cause).

**Acceptance:**
- `cargo test -p camel-cli --test feature_profiles` shows exactly 3 pass (Phase 1 set) + 2 fail (the new pair) — the documented red split.
- `cargo fmt --check`, `cargo clippy -p camel-cli --all-targets -- -D warnings` exit 0.

- [x] 2.1

### Task 2.2: Feature rework — lsp/lang optional, full + slim-http meta-features

**Files:**
- `crates/camel-cli/Cargo.toml` (modified)
- `crates/camel-cli/src/main.rs` (modified)
- `crates/camel-cli/src/commands/mod.rs` (modified)
- `crates/camel-cli/src/commands/lsp.rs` (gated via the module decl; no content edit expected — listed so the worker confirms it compiles untouched)

**Steps:**
1. In `crates/camel-cli/Cargo.toml` dependencies: change `camel-lsp.workspace = true` → `camel-lsp = { workspace = true, optional = true }`; `tower-lsp.workspace = true` → `tower-lsp = { workspace = true, optional = true }`; ariadne stays non-optional. Change `camel-core = { workspace = true, features = ["lang-js", "lang-rhai", "lang-jsonpath", "lang-xpath", "lang-minijinja"] }` → `camel-core.workspace = true` (camel-core's own default `export-internal-adapters` still applies — closure preserved).
2. In `[features]`: add `lsp = ["dep:camel-lsp", "dep:tower-lsp"]`, `lang-js = ["camel-core/lang-js"]`, `lang-rhai = ["camel-core/lang-rhai"]`, `lang-jsonpath = ["camel-core/lang-jsonpath"]`, `lang-xpath = ["camel-core/lang-xpath"]`, `lang-minijinja = ["camel-core/lang-minijinja"]`, `slim-http = []` (with a comment: named marker for the `--no-default-features` http-only baseline), and `full = ["otel", "grpc", "wasm", "http-static", "llm", "surrealdb", "exec", "mqtt", "mcp", "integration-http", "integration-sql", "security", "redis-tls", "lsp", "lang-js", "lang-rhai", "lang-jsonpath", "lang-xpath", "lang-minijinja"]`; change the existing `default` list (currently the 13 features `otel, grpc, wasm, http-static, llm, surrealdb, exec, mqtt, mcp, integration-http, integration-sql, security, redis-tls` in `crates/camel-cli/Cargo.toml`) to `default = ["full"]` (the `full` list is exactly today's default set plus the five lang features plus lsp — lsp and langs were previously unconditional, so the union reproduces today's closure).
3. Gate the LSP surface: `src/commands/mod.rs` → `#[cfg(feature = "lsp")] pub mod lsp;`; `src/main.rs` → gate the `Lsp` clap variant and its dispatch arm with `#[cfg(feature = "lsp")]` (clap derive supports cfg'd enum variants; the match arm gets the same gate). If `src/commands/lsp.rs` has no non-gated references left, no edit inside it is needed beyond compiling.
4. Search for compile fallout: `rg -n "camel_lsp|tower_lsp|Commands::Lsp" crates/camel-cli/src` — every hit must sit under a `#[cfg(feature = "lsp")]` gate or be removed. Same for `rg -n "camel_language_" crates/camel-cli/src` (expected zero hits — langs are camel-core-internal).
5. Iterate `cargo check -p camel-cli --no-default-features --features slim-http` until clean; then `cargo check -p camel-cli` (default) until clean.

**Tests:**
- `slim_closure_excludes_controllable_set`: after rework → passes (Task 2.1 red turns green).
- `slim_plus_grpc_resolves_grpc_only`: after rework → passes.
- `default_closure_matches_golden`: MUST STILL PASS — this is the closure-identity proof; if it fails, the `full` list diverges from today's default set (fix the list, never the golden). The comparison filters `camel-core feature "lang-*"` and `camel-core feature "camel-language-*"` lines on BOTH sides (live and golden) — cargo tree renders feature nodes for dep-declaration activation but never for feature-forwarding, and the lang features moved from declaration to forwarding by design (erases 10 golden lines mechanically, zero closure change); package-level presence of camel-language-* crates in default stays asserted by the surviving package lines.

**Acceptance:**
- `cargo test -p camel-cli --test feature_profiles` — 5/5 pass.
- `cargo check -p camel-cli` and `cargo check -p camel-cli --no-default-features --features slim-http` and `cargo check -p camel-cli --no-default-features --features slim-http,grpc` all exit 0.
- `cargo build -p camel-cli --release` exits 0.
- `cargo fmt --check`, `cargo clippy -p camel-cli --all-targets -- -D warnings` exit 0.

- [x] 2.2

### Task 2.3: Compile matrix + slim release binary

**Files:**
- `openspec/changes/clidiet/evidence/matrix.txt` (new)

**Steps:**
1. Run the compile matrix from the workspace root, recording command + exit code + wall time per row into `evidence/matrix.txt`:
   - `cargo check -p camel-cli` (default/full)
   - `cargo check -p camel-cli --no-default-features --features slim-http`
   - `cargo check -p camel-cli --no-default-features --features slim-http,grpc`
   - `cargo check -p camel-cli --no-default-features --features slim-http,lang-js`
   - `cargo check -p camel-cli --no-default-features --features slim-http,lsp`
   - `cargo check -p camel-cli --features jemalloc` (spec scenario "jemalloc remains the sole opt-in override": the override still compiles, gauges included; the `#[global_allocator]` backing half of the scenario is covered by the unchanged main.rs wiring, whose integrity Task 1.2 step 2 verified)
2. Build the slim release binary for Phase 3: `cargo build -p camel-cli --release --no-default-features --features slim-http`, then `cp target/release/camel /tmp/clidiet-camel-slim` (keep it OUT of the repo tree; note the exact path in matrix.txt).
3. Record in matrix.txt the slim closure size (`tree_lines` equivalent shell one-liner: `cargo tree -p camel-cli --no-default-features -e no-dev --prefix none --locked | sed -E 's| \(/[^)]*\)||g; s| \(\*\)||g; s| \[\*\]||g' | sort -u | wc -l`) next to the default closure size (both counts use the plain `-e no-dev` closure so they compare like-for-like; for context, the feature-edge golden is 2955 normalized lines).

**Tests:**
- matrix.txt rows: all six `cargo check` rows exit 0 (any non-zero → STOP and report, do not paper over).

**Acceptance:**
- `evidence/matrix.txt` exists with six green check rows + the seventh folded clippy-slim row (folded forward from the t2.2 review) + the two closure-size counts.
- `/tmp/clidiet-camel-slim` exists and `--version` prints the workspace version.

- [x] 2.3

## Phase 3: Measurement + sweep

### Task 3.1: Post-change full-build measurements + dedup delta

**Files:**
- `openspec/changes/clidiet/evidence/results-full.txt` (new)
- `openspec/changes/clidiet/evidence/audit-phase-a.md` (modified)

**Steps:**
1. Rebuild the default release binary: `cargo build -p camel-cli --release`; copy it to `/tmp/clidiet-camel-full`.
2. Measure with the replica script (n=30 per mode, from the evidence dir): `bash bench-cold-local.sh marker /tmp/clidiet-camel-full 30 full-post` and `bash bench-cold-local.sh help /tmp/clidiet-camel-full 30 full-post`.
3. Compute for both this run and the committed baseline files `evidence/results/full-base.marker.txt`, `evidence/results/full-base.help.txt`, `evidence/results/full-base.marker.rss.txt`, `evidence/results/full-base.help.rss.txt` (conductor-measured at the base commit on the baseline release binary and committed BEFORE Phase 3 begins — if any of the five is missing, STOP and report `baseline-missing`, do not self-measure): median and p90 of ms, median of RSS kb, plus exact binary sizes (`stat -c %s` on `/tmp/clidiet-camel-full` and the recorded baseline size in `evidence/results/full-base.size.txt`). Write the comparison table to `results-full.txt` including the dedup delta rows (baseline vs post-change default).
4. Append the table to `audit-phase-a.md` replacing the `<!-- BASELINE_TABLE -->` marker (baseline numbers land there if not already).

**Tests:**
- Band checks (assert in results-full.txt with PASS/FAIL per row): marker median within ±5% of baseline marker median; help median within ±5%; max-RSS medians within ±2048 kb; binary size delta zero or explained in one line.

**Acceptance:**
- `results-full.txt` contains: n=30 marker + help medians/p90s, RSS medians, byte sizes, delta rows, and explicit band verdicts — timing + help-RSS + size PASS under the final paired/tagged protocol; marker-RSS INCONCLUSIVE with regime-noise attribution (protocol note added post-review).
- The fixture booted in marker mode (no ERROR-exit-before-marker samples; any FAIL sample rows are listed with count).

- [x] 3.1

### Task 3.2: Slim-build measurements + win table

**Files:**
- `openspec/changes/clidiet/evidence/results-slim.txt` (new)

**Steps:**
1. Measure the slim binary: `bash bench-cold-local.sh marker /tmp/clidiet-camel-slim 30 slim-post` and `bash bench-cold-local.sh help /tmp/clidiet-camel-slim 30 slim-post`.
2. Compute the same statistics; write the full-vs-slim comparison (marker median, help median, RSS medians, byte sizes, closure counts from matrix.txt) to `results-slim.txt`.
3. The fixture must boot on the slim binary — the http consumer bind + timer→log marker prove the profile is self-coherent (spec scenario "slim build compiles and boots an http route").

**Tests:**
- Marker mode on slim binary: zero ERROR samples (any error → STOP and report; do not retry silently).

**Acceptance:**
- `results-slim.txt` contains the full-vs-slim table with medians/p90s/RSS/sizes; slim marker samples all reached BENCH_ROUTE_READY.

- [x] 3.2

### Task 3.3: CONTEXT.md updates, bless record, rc-kyq15 sweep verification

**Files:**
- `crates/camel-cli/CONTEXT.md` (modified)
- `openspec/changes/clidiet/bless-record.md` (new)
- `openspec/changes/clidiet/evidence/sweep-rc-kyq15.md` (new)

**Steps:**
1. CONTEXT.md: add a "## Build profiles" section stating the canonical allocator policy (default = system allocator; `jemalloc` = sole opt-in override backing musl production builds; mimalloc removed — bd rc-rrz6a) and the feature-profile surface (`full` = today's closure via `default = ["full"]`; `slim-http` = named `--no-default-features` baseline; `lsp` and `lang-*` individually selectable; the bundles-unconditional bridges jms/sql/redis/opensearch/ws/cxf/xslt/xj remain linked in every profile — deferred camel-bundles-side).
2. bless-record.md: transcribe EVERY round present in `.bless.json` (untracked by design) — verdicts, sessions, one-line findings — with no hard-coded round count; the file must match `.bless.json`'s rounds array exactly (compare lengths dynamically, do not assume a number).
3. Sweep rc-kyq15: verify its only listed item rc-mlzuq is closed (it is — landed `camel job <name> --help` declared-interface render); `rg -n "TODO|FIXME" crates/camel-cli/src/commands/job/` and check hits are not jobhelp-wave leftovers; write the disposition to `evidence/sweep-rc-kyq15.md` (expected: nothing outstanding, zero deferrals from this sweep).

**Tests:**
- `rg -n "slim-http" crates/camel-cli/CONTEXT.md` → ≥1 hit; `rg -n "jemalloc" crates/camel-cli/CONTEXT.md` → ≥1 hit (the policy section names both the sole override and the slim surface; it also states mimalloc's removal).
- sweep-rc-kyq15.md names rc-mlzuq as closed with its landing note.

**Acceptance:**
- CONTEXT.md Build-profiles section present; bless-record.md matches .bless.json's rounds array exactly (same count, same verdicts); sweep disposition file exists.

- [x] 3.3
