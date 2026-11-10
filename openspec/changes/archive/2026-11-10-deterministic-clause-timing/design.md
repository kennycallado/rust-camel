# Design: deterministic-clause-timing

## Approach

The strict branch of `group_timeouts_are_independent_per_clause` moves from
wall-clock assertions to **exact rhai operation-count assertions** (order
preference 2). No injected clock exists, and none can be added within this
mission's constraints.

### Mechanism 1 ruled out — no clock seam

Per-clause enforcement is `tokio::time::timeout(Duration::from_millis(N),
spawn_blocking(eval_with_engine(...)))` (`first_match`, lib.rs ~1310). There
is no `Instant`-like source a test can drive: the clock is tokio's real
timer. Driving it virtually requires tokio's `test-util` feature — a Cargo
change, forbidden here — and production exposes no script-controlled clock
seam the workload could advance. Two sufficient constraints rule the
mechanism out on their own: no `test-util` in this crate's test graph, and
no production change allowed to add one.

### Mechanism 2 — operation budget (chosen)

rhai enforces per-clause operation budgets, and the counts are exact
integers, machine-independent, load-immune, and instrumentation-immune:

- The group builds ONE engine (`create_base_engine`) with
  `engine.set_max_operations(resolved.max_operations)` and evaluates each
  clause with `eval_ast_with_scope`.
- rhai 1.27.0 creates a fresh `GlobalRuntimeState` (zeroed
  `num_operations`) per top-level eval (`api/eval.rs:197`), so the counter
  resets per clause. The limit fires strictly when `num_ops > max`
  (`eval/data_check.rs::track_operation`): ops == limit passes.
- `on_progress(impl Fn(u64) -> Option<Dynamic>)` fires per operation with
  the running total, so a completed probe run yields the exact total.
- The existing green test `group_budgets_do_not_accumulate_across_clauses`
  already proves counter independence behaviorally; this change upgrades the
  strict branch to the same exact-integer discipline.

Test design (all inside `mod tests`, same file — private items accessible):

1. **Probe**: `RhaiLanguage::create_base_engine` + `on_progress` capturing
   the last seen total into an `Arc<AtomicU64>`, scope bound exactly like
   production (`body = "x"` text, empty header/property maps). Probe the
   single workload `budget_loop_script(K)` → exact `N1`; probe the double
   workload (two loops, one eval) → exact `N2`. Run each probe twice and
   assert the two measurements are equal (machine-independence evidence on
   the running machine).
2. **Budget**: `L = N1 + N1 / 2`, an integer strictly between one and two
   workload costs; panic with a clear message if `N2 <= L` (deterministic
   for fixed K, so if this ever trips it is a rhai-semantics change, not
   load).
3. **Independence assertion** (what the strict branch must not lose): the
   two-workload group under `group_lang(L, 5_000)` returns `None` with
   `eval_log == [0, 1]`. Each clause costs `N1 < L`; a shared budget would
   reach `N2 > L` and cut clause 1.
4. **Shared-budget semantic check**: one clause whose script carries both
   workloads in a single eval under the same `L` fails with
   `ExpressionErrorClass::Limit`. This is the counterfactual: clause 1 of a
   shared-budget group would see exactly this outcome.
5. **Measured quantity printed** every run: `N1`, `N2`, `L`, per-iteration
   cost, branch label. The 20-run evidence in the park shows these.

The strict branch stops using the shared constants: `TIMEOUT_MS = 200`,
`OP_LIMIT = 10^9`, and the `lang` built from them stay in the test untouched
because the instrumentation branch uses them and must remain
byte-identical; the strict tail builds its own `group_lang(budget, 5_000)`.
The 0.6/0.8 wall-clock factors disappear from the strict branch because the
enforcement dimension moved to operations; nothing is lowered. The strict
tail's `execution_timeout_ms` is a non-binding 5_000, mirroring the existing
budget tests. The instrumentation branch stays byte-identical.

### Negative proof

Temporary production edit in `first_match` (worktree only, reverted after
evidence): combine the two clause ASTs with `AST::combine` and evaluate once
— one eval call, one counter, one budget: the definition of a shared budget.
With the edit, the two-workload group hits `N2 > L` and returns `Limit` at
index 0, so the independence assertion (`None` + `eval_log [0, 1]`) fails.
Evidence is captured, the edit is reverted, and the test re-runs green.

## Affected crates

- camel-language-rhai: the strict branch of one lib test + probe helpers in
  `mod tests`. Nothing else.

## Architecture boundaries

Languages crate only; no Runtime/DSL/Component/Service surface touched. The
sandbox guarantees (`create_base_engine`: raw engine, no module resolver,
disabled eval/import) are consumed, not modified — the probe uses the same
sandboxed engine factory production uses.

## Residual risk

The wall-clock dimension of this test is retired, and with it deterministic
detection of a timeout-placement regression (moving `tokio::time::timeout`
outside the clause loop): operation counts are unaffected by timeout
placement, so no remaining test detects that specific change. What still
holds without this test: the timeout CUT path stays covered by
`group_clause_timeout_is_indexed_and_stops` (a clause that exceeds its own
timeout is cut and indexed), and
`group_budgets_do_not_accumulate_across_clauses` covers operation-counter
placement. The property that the order forbids losing — per-clause budget
independence, shared budget cuts the pair — is preserved exactly, in
operations.
