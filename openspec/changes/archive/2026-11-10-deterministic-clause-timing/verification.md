# Verification — deterministic-clause-timing (Tasks 1.1–1.3)

Branch: `feature/rhaitiming2`
Worktree: `/home/shared/rust-camel-worktrees/rhaitiming2`
State under test: Tasks 1.1 + 1.2 applied, uncommitted, in
`crates/languages/camel-language-rhai/src/lib.rs`.

## Environment

- Debug profile (`cargo test`, no `--release`); `debug=0` per workspace profiles.
- Every command ran foreground through
  `systemd-run --user --scope --collect --unit=<fresh unit> -p MemoryMax=12G --setenv=TMPDIR=/tmp`,
  with cwd = the worktree above.
- Host: 12 CPUs. No cargo command was run in the main checkout.
- The only backgrounded processes were the two CPU spinners documented in the
  load-soak section.

## Commands run

```text
systemd-run --user --scope --collect --unit=fleet-rhaitiming2-1.3-neg1       -p MemoryMax=12G --setenv=TMPDIR=/tmp cargo test -p camel-language-rhai --lib group_timeouts_are_independent_per_clause -- --nocapture   # negative proof (temporary edit)
systemd-run --user --scope --collect --unit=fleet-rhaitiming2-1.3-neg2       -p MemoryMax=12G --setenv=TMPDIR=/tmp cargo test -p camel-language-rhai --lib group_timeouts_are_independent_per_clause -- --nocapture   # after byte-exact revert
systemd-run --user --scope --collect --unit=fleet-rhaitiming2-1.3-warm       -p MemoryMax=12G --setenv=TMPDIR=/tmp cargo build --workspace --lib -j4
systemd-run --user --scope --collect --unit=fleet-rhaitiming2-1.3-soak-$i    -p MemoryMax=12G --setenv=TMPDIR=/tmp cargo test -p camel-language-rhai --lib group_timeouts_are_independent_per_clause -- --nocapture   # i = 1..20
systemd-run --user --scope --collect --unit=fleet-rhaitiming2-1.3-plain      -p MemoryMax=12G --setenv=TMPDIR=/tmp cargo test -p camel-language-rhai --lib group_timeouts_are_independent_per_clause -- --nocapture
systemd-run --user --scope --collect --unit=fleet-rhaitiming2-1.3-cov        -p MemoryMax=12G --setenv=TMPDIR=/tmp /nix/store/zfa3acqdxhla9pvl0p9wwxvrm9q1nm2p-cargo-llvm-cov-0.8.7/bin/cargo-llvm-cov llvm-cov -p camel-language-rhai --lib --no-report -- --nocapture
systemd-run --user --scope --collect --unit=fleet-rhaitiming2-1.3-suite      -p MemoryMax=12G --setenv=TMPDIR=/tmp cargo test -p camel-language-rhai --lib
systemd-run --user --scope --collect --unit=fleet-rhaitiming2-1.3-fmt        -p MemoryMax=12G --setenv=TMPDIR=/tmp cargo fmt --check --all
systemd-run --user --scope --collect --unit=fleet-rhaitiming2-gates          -p MemoryMax=12G --setenv=TMPDIR=/tmp bash scripts/gates-local.sh
```

## Negative proof

Temporary edit (reverted, never committed): in
`impl PredicateGroup for RhaiPredicateGroup::first_match`, a branch inserted
immediately before the `for (index, ast) in self.asts.iter().enumerate()` loop,
marked `// TEMPORARY negative proof — revert before commit;`. When
`self.asts.len() == 2` it clones `asts[0]` into a `rhai::AST`, calls
`.combine((*self.asts[1]).clone())` (rhai 1.27, `ast/ast.rs:393`), and evaluates
the combined AST exactly once through the same
`tokio::time::timeout(timeout, spawn_blocking(eval_with_engine(...)))` path with
index `0` for error mapping and the same `strict_predicate_bool` result
handling. One eval call therefore means one shared operation counter for the
two clauses — the counterfactual the test is supposed to reject.

Result under the shared counter (verbatim):

```text
running 1 test
branch=strict
strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6

thread 'tests::group_timeouts_are_independent_per_clause' (564996) panicked at crates/languages/camel-language-rhai/src/lib.rs:5213:18:
per-clause budgets must let both clauses complete: IndexedPredicateError { index: 0, error: EvalFailure { class: Limit, position: Some(ErrorPosition { line: 1, column: 18 }), detail: Some("max-operations 900012 exceeded") } }
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test tests::group_timeouts_are_independent_per_clause ... FAILED

failures:

failures:
    tests::group_timeouts_are_independent_per_clause

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 251 filtered out; finished in 0.18s

error: test failed, to rerun pass `-p camel-language-rhai --lib`
```

Line numbers in the panic output refer to the temporarily-edited tree (the negative-proof insertion shifted the file by ~47 lines); the same independence assertion sits at lib.rs:5166 after the byte-exact revert.

The test fails because the shared counter charges `double_ops=1200014` against
`budget=900012`, so the combined eval trips `Limit` at index 0.

Revert verification (byte-exact copy back from the pre-edit backup):

```text
$ git diff crates/languages/camel-language-rhai/src/lib.rs | grep -c "TEMPORARY negative proof"
0
$ git diff -U0 crates/languages/camel-language-rhai/src/lib.rs | grep '^@@'
@@ -1527,0 +1528,2 @@ mod tests {
@@ -4724,0 +4727,99 @@ mod tests {
@@ -5017,2 +5117,0 @@ mod tests {
@@ -5023,12 +5121,0 @@ mod tests {
@@ -5057,4 +5143,0 @@ mod tests {
@@ -5062,31 +5145,9 @@ mod tests {
@@ -5094,7 +5155,22 @@ mod tests {
@@ -5102,3 +5178,10 @@ mod tests {
```

Every remaining hunk starts at or after line 1527 — all inside `mod tests`.
There is no hunk in the production `first_match` region (~1286).

Re-run after the revert:

```text
branch=strict
strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6
test tests::group_timeouts_are_independent_per_clause ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 251 filtered out; finished in 0.20s
```

## Load soak

Induced load: two busy loops, each `bash -c 'while :; do :; done'`, launched
with `setsid` from `/tmp` so they survive their parent shell.

Timeline of the spinner setup is recorded as measured, including a first
attempt that did not do what it claimed: `$!` inside one compound command
reported the wrapper shell (PID 565253, `S` state) instead of one of the
spinners, so the first attempt left a single confirmed busy loop (PID 565254,
~88–100% CPU) rather than two. The wrapper and the stray loop were killed, and
the pair was re-launched with `setsid` and verified with `ps` before the soak:

```text
 669493 Rs   99.6 bash -c while :; do :; done
 669494 Rs   99.7 bash -c while :; do :; done
```

Exactly two spinners, one thread each on a 12-CPU host. They were killed with
`kill 669493 669494` after run 20 (`pgrep -f '^bash -c while :; do :; done$'`
returned 0 processes afterwards).

Cache warm-up ran before the clean pair was launched, so it carried the one
confirmed busy loop from the first attempt plus build parallelism
(`/proc/loadavg` was `8.58 11.11 11.08` immediately after it):

```text
cargo build --workspace --lib -j4
    Finished `dev` profile [unoptimized] target(s) in 15m 46s
EXIT=0 DURATION=946s
```

Per-run results with both verified spinners alive:

| run | exit | measured line | test result |
| --- | --- | --- | --- |
| 1 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |
| 2 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |
| 3 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |
| 4 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |
| 5 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |
| 6 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |
| 7 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |
| 8 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |
| 9 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |
| 10 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |
| 11 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |
| 12 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |
| 13 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |
| 14 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |
| 15 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |
| 16 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |
| 17 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |
| 18 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |
| 19 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |
| 20 | 0 | `strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6` | ok, 1 passed |

20 of 20 runs passed, and all 20 measured lines are identical — the same
`single_ops` / `double_ops` / `budget` / `per_iteration` values as the
unloaded runs (a single distinct measured field across the whole soak,
`sed 's/.*| \(strict measured:[^|]*\) |.*/\1/' | sort -u | wc -l` → `1`).
Soak runs emitted no compilation output (cache fully warm); each test finished
in 0.21 s (run 16: 0.22 s).

Plain run after killing the spinners:

```text
branch=strict
strict measured: single_ops=600008 double_ops=1200014 budget=900012 per_iteration=6
test tests::group_timeouts_are_independent_per_clause ... ok
EXIT=0
```

Identical to every soak line.

## Instrumented branch (cargo-llvm-cov)

Exit code 0, duration 27 s:

```text
`branch=semantic-instrumented`
`test tests::group_first_match_returns_first_matching_index ... ok` (adjacent context line)
`test tests::group_timeouts_are_independent_per_clause ... ok`
test result: ok. 252 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.42s
```

`branch=strict` occurrences in the llvm-cov log: `0` — the instrumentation
branch was taken, so the strict branch was correctly skipped. The
`group_timeouts_are_independent_per_clause` line passing covers both of its
instrumented-branch assertions, including
`eval_log() == vec![0, 1]` (both clauses evaluated in order under
instrumentation).

The plan's "eval_log = [0, 1] evidence" is satisfied by the passing source assertion at lib.rs:5136-5140 in the semantic branch (the branch never prints eval_log, so no literal log line can contain it).

## Crate suite, formatting, gates

```text
cargo test -p camel-language-rhai --lib        EXIT=0
test result: ok. 252 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.43s

cargo fmt --check --all                        EXIT=0   (no output)

bash scripts/gates-local.sh                    EXIT=0   DURATION=520s
──────── gates-local summary ────────
passed:  28
failed:  0
skipped: 4 — lint-commits lint-bridge-releases doc-guide-test cargo-audit
ran:     28 of 32
```

All 28 locally runnable gates passed; the 4 skipped gates are the network and
docs-toolchain gates the script leaves to CI by default.
