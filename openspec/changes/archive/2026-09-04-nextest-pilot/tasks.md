# Tasks: nextest-pilot

## CI / test-execution infrastructure

### Task 1.1: Add the nextest `ci` profile

**Files:**
- `.config/nextest.toml` (new)

**Steps:**
1. Create `.config/nextest.toml` with exactly this content:
   ```toml
   # Adjudicated pilot policy (bd rc-mhsn, epic rc-99d5): the ubuntu
   # Rust library-test job runs under this profile. retries are
   # diagnostic — flaky-result = "fail" makes a retry-pass fail the
   # gating job. slow-timeout ceiling is period x terminate-after
   # (~90s), not the period alone.
   [profile.ci]
   retries = 1
   flaky-result = "fail"
   slow-timeout = { period = "30s", terminate-after = 3 }
   failure-output = "immediate-final"
   fail-fast = false
   ```
2. In the worktree, run `cargo nextest list --workspace --lib --profile ci`
   and confirm it exits 0 — nextest rejects unknown profile keys and
   invalid value types at profile-load time, so exit 0 proves the file
   parses under the installed cargo-nextest 0.9.143.
3. One-shot behavioral probe (in the worktree, ~180s, then deleted):
   create a scratch crate at `nextest-pilot-probe/` (repo root, NOT under
   `crates/`) that is standalone by construction:
   - `nextest-pilot-probe/Cargo.toml`: `[package]` with
     `name = "nextest-pilot-probe"`, `version = "0.0.0"`,
     `edition = "2021"`, plus an empty `[workspace]` table so cargo
     treats it as its own workspace and never a member of the repo
     workspace.
   - `nextest-pilot-probe/src/lib.rs` with two tests:
     `hang_ceiling_probe`: `#[test] fn hang_ceiling_probe() { loop {} }`,
     and `flaky_probe` whose fail-once state lives OUTSIDE the process
     (nextest retries run in a fresh process, so a static resets):
     ```rust
     #[test]
     fn flaky_probe() {
         let sentinel = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
             .join("first-attempt-done");
         if sentinel.exists() {
             std::fs::remove_file(&sentinel).unwrap();
             return; // retry (fresh process): pass
         }
         std::fs::write(&sentinel, b"1").unwrap();
         panic!("first attempt fails; nextest retry passes");
     }
     ```
     Remove `nextest-pilot-probe/first-attempt-done` BEFORE the probe
     run if it exists (leftover from an aborted run would make both
     attempts pass and the probe useless).
   - Copy the repo profile in:
     `mkdir -p nextest-pilot-probe/.config && cp .config/nextest.toml
     nextest-pilot-probe/.config/nextest.toml` (nextest resolves the
     profile from the built workspace's root, so the probe tests the
     byte-identical profile file).
   Run `cargo nextest run --manifest-path
   nextest-pilot-probe/Cargo.toml --profile ci` and assert: nonzero
   exit, a `TERMINATED` (slow-timeout) line for `hang_ceiling_probe`
   arriving around ~90s (three 30s periods, not one), and a `FLAKY`
   line for `flaky_probe` with the run failing overall. Then
   `rm -rf nextest-pilot-probe/` and assert the probe is gone:
   `test ! -e nextest-pilot-probe` and
   `git status --short -- nextest-pilot-probe` prints nothing. (Do NOT
   assert a fully clean `git status --short` here: this task's own
   `.config/nextest.toml` is intentionally still uncommitted at this
   point.)

**Tests:**
- `profile_loads`: empty worktree target dir (or warm cache) → `cargo nextest list --workspace --lib --profile ci` → exit 0, no "unknown key" or "invalid type" diagnostics on stderr.
- `profile_keys_present`: the file exists → `grep -c 'flaky-result = "fail"' .config/nextest.toml` → 1; same check for `retries = 1`, `terminate-after = 3`, `fail-fast = false`.
- `hang_ceiling_probe` (scratch, reverted): profile ci loaded → run probe crate under nextest → exit nonzero, `TERMINATED` report for `hang_ceiling_probe`, no termination before ~3 periods (~90s effective ceiling, proving period×terminate-after semantics).
- `flaky_probe` (scratch, reverted): profile ci loaded → run probe crate under nextest → `FLAKY` report for `flaky_probe` AND overall run conclusion is failure (flaky-result = "fail" gate).

**Acceptance:**
- `cargo nextest list --workspace --lib --profile ci` exits 0 in the worktree.
- `grep -q 'flaky-result = "fail"' .config/nextest.toml` exits 0.
- No other file in the repo changed in this task's diff.

- [x] 1.1

### Task 1.2: Switch the Unit Tests ubuntu job to nextest

**Files:**
- `.github/workflows/ci.yml` (modified)

**Steps:**
1. In the `unit-tests` job, after the `Install libclang for bindgen`
   step, add:
   ```yaml
   - name: Install cargo-nextest
     uses: taiki-e/install-action@eba66cc6f87204a1e73f96e528e759b6c1fcf573 # cargo-nextest
     with:
       tool: cargo-nextest@0.9.143
   ```
   (same pinned action SHA already used by the coverage and quality
   jobs for cargo-llvm-cov / cargo-audit).
2. Replace the step currently named `Test (unit only — no Docker
   required)` run command `cargo test --workspace --lib` with
   `cargo nextest run --workspace --lib --profile ci`, keeping the step
   name unchanged so job logs stay greppable.
3. Confirm NO other job changed, byte-for-byte: compare the worktree
   ci.yml against `HEAD:.github/workflows/ci.yml` job-block by
   job-block (all jobs except `unit-tests` must be byte-identical):
   ```bash
   python3 - <<'EOF'
   import re, subprocess
   def segments(text):
       lines = text.splitlines(keepends=True)
       starts = [i for i, l in enumerate(lines) if re.match(r'^  [a-z][a-z0-9-]*:$', l)]
       out = {}
       for n, i in enumerate(starts):
           j = starts[n + 1] if n + 1 < len(starts) else len(lines)
           out[lines[i].strip().rstrip(':')] = ''.join(lines[i:j])
       return out
   head = subprocess.run(['git', 'show', 'HEAD:.github/workflows/ci.yml'],
                         capture_output=True, text=True, check=True).stdout
   work = open('.github/workflows/ci.yml').read()
   h, w = segments(head), segments(work)
   assert set(h) == set(w), f'job set changed: {set(h) ^ set(w)}'
   changed = [k for k in h if k != 'unit-tests' and h[k] != w[k]]
   assert not changed, f'jobs modified outside unit-tests: {changed}'
   print('scope ok: only unit-tests changed')
   EOF
   ```

**Tests:**
- `yaml_structural`: ci.yml edited → `python3 -c "s=open('.github/workflows/ci.yml').read(); assert '\t' not in s; assert s.count('cargo nextest run --workspace --lib --profile ci')==1; assert s.count('tool: cargo-nextest@0.9.143')==1"` → exit 0.
- `job_count_stable`: `grep -cE '^  [a-z]' .github/workflows/ci.yml` → 11 (push, pull_request, contents + 8 job ids), unchanged from the base commit.
- `scope_guard`: task diff applied → run the job-segmentation python block from Step 3 (segments HEAD vs worktree ci.yml by top-level job key) → prints `scope ok: only unit-tests changed`; any byte difference in another job or any job-set change raises AssertionError.
- `pinned_version`: ci.yml contains `tool: cargo-nextest@0.9.143` exactly once.

**Acceptance:**
- `grep -c 'cargo nextest run --workspace --lib --profile ci' .github/workflows/ci.yml` = 1.
- `grep -c 'tool: cargo-nextest@0.9.143' .github/workflows/ci.yml` = 1.
- The `macos-build-smoke`, `full-tests-linux`, `docker-smoke-test`, `coverage`, `quality`, `bench-smoke`, and `wasm-integration` job steps are byte-identical to the base commit (scope_guard test passes; Task 1.2 step 3 inspection confirms no change outside the `unit-tests` block).

- [x] 1.2

### Task 1.3: Selection parity and baseline measurement

**Files:**
- `.config/nextest.toml` (unchanged, reference)
- `.github/workflows/ci.yml` (unchanged, reference)

**Steps:**
1. In the worktree run `cargo test --workspace --lib -- --list 2>/dev/null
   | grep -c ': test'` and record the number (cargo baseline count).
2. Run `cargo nextest list --workspace --lib --profile ci 2>/dev/null |
   wc -l` and record the number (nextest selected count).
3. Assert the two numbers are equal. If they diverge, STOP and report
   `parity-gap: cargo=<N> nextest=<M>` — do not adjust filters to force
   equality; a divergence means the pilot's selection-parity acceptance
   (delta spec scenario) fails and needs a design decision.
4. Run `cargo nextest run --workspace --lib --profile ci` once in the
   worktree; record wall time (`/usr/bin/time` or shell `time`), the
   summary line's test count, and any FLAKY or slow-timeout lines.
5. Record the baseline in bd rc-mhsn with ONE append in this exact
   format (all fields mandatory):
   `PILOT-BASELINE cargo=<N> nextest=<N> wall=<seconds>s tests_run=<T> flaky=<F> terminated=<R>`
   where `<N>` is the shared selection count, `<seconds>` the wall
   time, `<T>` the nextest-reported tests-run count, `<F>` the number
   of FLAKY lines (0 on a green baseline), `<R>` the number of
   TERMINATED/slow-timeout lines (0 on a green baseline).
   Command: `bd update rc-mhsn --append-notes "PILOT-BASELINE cargo=<N> nextest=<N> wall=<seconds>s tests_run=<T> flaky=<F> terminated=<R>"` (from the repo root, not the worktree).

**Tests:**
- `selection_parity`: both list commands complete → numbers equal → notes recorded; expected: equal on the first run (both select `--lib` targets of the same 50+ crates).
- `pilot_run_green`: `cargo nextest run --workspace --lib --profile ci` in the worktree → exit 0, summary `X passed`, zero `FLAKY` lines, zero `TERMINATED`/slow-timeout lines.

**Acceptance:**
- `bd show rc-mhsn` notes contain a `PILOT-BASELINE cargo=<N> nextest=<N> wall=<seconds>s tests_run=<T> flaky=<F> terminated=<R>` line where both count integers are equal and non-zero, `tests_run` is non-zero, and `flaky=0 terminated=0` on the green baseline.
- Local pilot run in the worktree exits 0.
- `git status --short` in the worktree shows only the two intended changes (`.config/nextest.toml`, `.github/workflows/ci.yml`); no probe artifacts remain.

- [x] 1.3
