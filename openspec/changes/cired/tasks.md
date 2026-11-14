# Tasks: cired

Single-phase repair set: two test-only fixes and one camel-test fixture. No
product code changes. Task order: 1.1 → 1.2 → 2.1, then the closeout block.

## camel-test

### Task 1.1: Guard the rhai registration against workspace feature unification

**Files:**
- `crates/camel-test/tests/dev_watch_script_test.rs` (modified)

**Steps:**
1. Replace the unconditional registration at lines 185-186 with an
   absence-guarded registration, keeping the existing `.expect` message:
   ```rust
   if ctx.resolve_language("rhai").is_none() {
       ctx.register_language("rhai", Box::new(RhaiLanguage::new()))
           .expect("rhai language registration");
   }
   ```
   `resolve_language` exists on `CamelContext` (`camel-core/src/context.rs:639`);
   import nothing new. Do NOT change the error to `.ok()` and do NOT touch any
   other line of the file.
2. Run `cargo fmt --check --all` in the worktree; fix formatting of the edited
   file only if the check fails.

**Tests:** (executable spec — the suite's two existing tests are the contract)
- `script_edit_swaps_route_and_restored_content_skips`: setup —
  workspace-unified build (`camel-core/lang-rhai` on via other members, so the
  built-in registry already seeds `rhai`) → act — run the test binary → assert
  — both tests in the file pass; no `AlreadyRegistered` panic at line 186.
- `partial_success_keeps_failed_route_script_dep`: same setup and command →
  assert — passes; the script-watch swap behavior is unchanged.
- Registration branch (feature off): `cargo test -p camel-test --test
  dev_watch_script_test` → assert — passes; the guard takes the registration
  branch and a fresh `rhai` registration still succeeds.

**Acceptance:**
- `cargo test -p camel-test --test dev_watch_script_test` exits 0 (registration branch).
- `cargo test --workspace --test dev_watch_script_test` exits 0
  (already-registered branch; this command FAILS before the fix — reproduced
  2026-11-14: both tests panic `rhai language registration: AlreadyRegistered`).
- `cargo clippy -p camel-test --all-targets -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 1.1

### Task 1.2: Wait for the descriptor generation commit before asserting it

**Files:**
- `crates/camel-test/tests/dev_watch_proto_test.rs` (modified)

**Steps:**
1. In `proto_edit_recompiles_validator_descriptor` (lines 347-359): keep the
   `wait_for_validation` assert, then replace the immediate unsynchronized
   `assert!(generation(route_id) > gen_before, "schema edit must commit a new
   descriptor generation")` with:
   ```rust
   assert!(
       wait_for(|| generation(route_id) > gen_before, WAIT_BOUND).await,
       "watcher must commit the reload generation for the edit"
   );
   ```
   Keep the following exact assertion (`generation(route_id) == gen_before + 1`,
   "one schema edit must commit exactly one generation") unchanged.
2. In `import_edit_recompiles_validator_descriptor` (lines 383-390): replace
   the immediate `assert!(generation(route_id) > gen_before, "import-only edit
   must commit a new descriptor generation")` with the same `wait_for`-based
   assert (message: "watcher must commit the reload generation for the import
   edit").
3. Update the module doc comment (`//!` lines 13-14) from "generation counters
   are only checked afterwards" to state the reason: `commit` publishes the
   descriptor and increments the generation counter as two separate steps, so
   the counter advance is awaited under the same bounded poll after the
   validation outcome, then asserted exactly.
4. Do not modify `generation`, `wait_for`, `wait_for_validation`, the harness,
   or any product crate.

**Tests:** (executable spec — the suite's existing watched tests are the contract)
- `proto_edit_recompiles_validator_descriptor`: setup — workspace-unified
  build, harness with watcher ready → act — edit `root.proto`, wait for
  validation of the added field, then wait for the generation advance under
  the bounded poll → assert — validation succeeds, generation advances, and
  `generation == gen_before + 1` exactly.
- `import_edit_recompiles_validator_descriptor`: same harness → act — edit
  `common.proto`, wait for validation, wait for generation advance → assert —
  passes.
- `non_watch_mode_never_reloads`: untouched → assert — still passes (no
  watcher, no reload, generations stay 0).

**Acceptance:**
- `cargo test --workspace --test dev_watch_proto_test` exits 0, run twice
  (timing-sensitive suite; the suite is green locally before the fix — the fix
  removes the load-dependent unsynchronized read, verified red in CI run
  43560699207).
- `cargo clippy -p camel-test --all-targets -- -D warnings` exits 0.
- `cargo xtask lint-test-sleep` exits 0 (no sleep introduced; bounded poll
  uses the existing `wait_for`).
- `cargo fmt --check --all` exits 0.

- [x] 1.2

## docs

### Task 2.1: Ship the script fixture where the sweep resolves it

**Files:**
- `crates/camel-test/scripts/enrich.rhai` (new)

**Steps:**
1. Create `crates/camel-test/scripts/enrich.rhai` with a small mutating Rhai
   script matching the documented example (`docs/src/eip/script.md` fence:
   `script:` step with `source_file: scripts/enrich.rhai` inside route
   `script-file-demo`):
   ```rhai
   // Enrich the exchange for the `script-file-demo` example route
   // (docs/src/eip/script.md).
   headers["enriched_by"] = "script-file-demo";
   body = body + " (enriched)";
   ```
   The script must compile as Rhai and be capability-`Mutating` (header/body
   assignment), which the sweep requires for every parsed script site. This
   location is where the sweep resolves the reference: the sweep parses fences
   via `camel_dsl::parse_yaml_to_declarative`, which hardwires
   `ScriptLoadContext::new(None)` (`camel-dsl/src/yaml.rs:111`); with no base
   dir, still-relative paths join the process current working directory, which
   under `cargo test` is the `camel-test` package directory (verified
   empirically via the sweep ledger error path).
2. Do NOT touch `crates/camel-test/tests/fixtures/route_sweep_baseline.tsv`,
   the sweep's key-set assertion, or any prose in `docs/src/eip/script.md`.

**Tests:** (executable spec)
- `route_examples_parse_without_new_rejection`: setup — docs tree with the new
  fixture at `crates/camel-test/scripts/enrich.rhai` → act — run the sweep,
  which parses every YAML fence under `docs/**` and `benchmarks/**` and
  resolves `source_file` against the test process working directory → assert
  — the `docs/src/eip/script.md` fence 1 parses with Mutating `rhai`
  capability and the ledger key set equals the baseline key set exactly.

**Acceptance:**
- `cargo test --workspace --test route_example_parser_sweep` exits 0 (FAILS
  before the fix — reproduced 2026-11-14: `route sweep: ParseError key set
  must equal the baseline key set exactly`).
- `cd docs && mdbook test` exits 0 (documentation gate).
- `git diff --name-only` against the base shows NO change to
  `crates/camel-test/tests/fixtures/route_sweep_baseline.tsv`.

- [x] 2.1

## Closeout

Run after tasks 1.1, 1.2 and 2.1 are all checked, in the feature worktree
(`/home/kenny/dev/rust-camel/.worktrees/cired`). Every command must exit 0;
record exit codes in the mission park.

1. Combined failing set, run TWICE:
   `cargo test --workspace --test dev_watch_script_test --test dev_watch_proto_test --test route_example_parser_sweep`
2. `cargo build --workspace`
3. `bash scripts/gates-local.sh`
4. Scope check (executable; the worktree must be committed and clean first —
   `git status --porcelain` prints nothing — because the diff below covers
   committed changes only):
   ```bash
   git status --porcelain   # must be empty; commit before this check
   violations="$(git diff --name-only 89c52e82d...HEAD \
     | grep -vE '^crates/camel-test/tests/dev_watch_(script|proto)_test\.rs$' \
     | grep -vE '^crates/camel-test/scripts/enrich\.rhai$' \
     | grep -vE '^openspec/changes/cired/')"
   test -z "$violations" || { echo "SCOPE VIOLATION: $violations"; exit 1; }
   ```
   The check exits 0 only when every changed path is one of: the two test
   files, the new fixture, or planning artifacts. Any product `src` file,
   `route_sweep_baseline.tsv`, sweep source, or `AGENTS.md` makes it exit 1 →
   fix before close.

- [x] 3.1
