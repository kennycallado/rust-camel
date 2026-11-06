# Design: scenario-finally-hooks

Binding design (e_opus 2026-11-05 ruling, verified against the tree; mission 502's spec was
blessed over three rounds — this file restates the settled design with the tree anchors the
plan review confirmed).

## 1. Grammar: document-level `finally:`

`RawDocument` (document.rs ~412, `deny_unknown_fields`) gains `finally:
Option<serde_yaml::Value>` — a raw node, converted during validation so errors name positions
(the `inbound:`/`logs:` precedent). `ScenarioDocument` gains
`finally: Option<Vec<ScenarioAction>>`, sibling of `scenario:`. NOT a `before:` key: `before:`
is the head of the action list and needs no mechanism.

Allowed set, exactly: `sleep`, `sql`, `surreal`, `redis` (delete-only). The `finally:` walk
maps each raw entry through the existing `build_action` converter (document.rs:968); a
`send`/`receive`/`validate` entry fails at load with a message naming the action key AND
the `finally:` position (e.g. `` `finally[0]`: `send` is a verdict action; `finally:`
accepts only prepare-class actions: sleep, sql, surreal, redis-del ``). Position naming:
`DocError::Validation`'s Display hardcodes the `scenario[{index}]` prefix
(document/error.rs:171), so converter errors cannot be relabeled by message edits —
`DocError` gains a `FinallyValidation { index, message }` variant rendering
`doc-validation: finally[{index}]: {message}`; the finally walk maps every converter error
(grammar, allowed-set rejection, demand gate) through it. `finally: []` (declared but
empty) is a load-time `doc-validation` error, mirroring the empty-`prepare` law in
`State prepare actions`.

Grammar validation precedes the demand gate (the `sql:` precedent, document.rs ~876-909):
a featureless build with a malformed `sql:` finally entry reports the grammar error; a valid
one reports the named demand-gate error via the feature-off twin.

## 2. New `redis` del prepare action

`ScenarioAction::RedisDel { datasource: String, keys: Vec<String> }` — structured,
`{redis: {datasource, del: [keys...]}}`, visibly distinct from the `validate` redis target
(`{redis: {datasource, key, type, ttl?}}`, a read). Accepted in BOTH `scenario:` and
`finally:` lists (binding ruling). Parse arm rides the existing action converter
`build_action` (document.rs:968) alongside the sql/surreal action arms (~1229-1250),
feature-split `redis_del_action_from_raw` /
feature-off twin like sql (validate grammar first, then the named demand-gate error).

Load-time rules (every build): `datasource` is a configured datasource name, never
env-interpolated (identifier law); `del` is non-empty; every key is a document-authored
literal that MUST NOT contain glob metacharacters — rejected through
`glob_metacharacters_in` (document.rs:526-545), naming the key. No `FLUSHDB`, no `FLUSHALL`,
no pattern delete, no generic command runner. Pattern delete is deferred to rc-ql8eq.

Execution (`redis` feature): resolve the datasource through the same `DatasourceCatalog`
the booted composition root built (one datasource name, one connection handle; the executor
MUST NOT construct its own — the `Redis state assertion` law), issue one `DEL` over the
literal keys, treat a driver error as apparatus failure with ADR-0051-sanitized text and the
URL redacted; missing keys are not errors (`DEL` is idempotent). The feature-off
`run_action` arm fails closed with the named demand-gate error (defense-in-depth, sql arm
precedent). The executor lives in the new `src/redis_action.rs`, beside `sql_action.rs`
(the `execute_sql_prepare` precedent), with `runner/redis_validate.rs` as the
connection-resolution reference.

## 3. Compile boundary (plan-review F1)

`run_action` (runner.rs:636-778) matches `ScenarioAction` EXHAUSTIVELY with no catch-all.
The `RedisDel` variant, its executor, and its `run_action` arm (both `cfg` blocks) land in
the SAME task; test tasks only add tests. The same discipline applies to every other
exhaustive match on `ScenarioAction`/`ScenarioFailure` the variant touches:
`camel-cli` `action_label` and `is_apparatus` (commands/test/scenario.rs:111-136)
gain their arms with their variants (`action_label`'s arm rides the `RedisDel` task;
`is_apparatus`'s rides the `TeardownFailure` task). Both struct additions carry the same
discipline: `ScenarioDocument.finally` (task 1.2) breaks ~11 full struct literals
(camel-cli scenario_tests.rs:313; harness --lib sites boot_scenario_test.rs, inbound.rs,
partner_validate_test.rs, redis_validate.rs, surreal_validate_test.rs, runner_test.rs; and
tests/http_inbound_test.rs, http_outbound_test.rs) — task 1.2 enumerates and fixes them,
with all-targets compile acceptance of both crates; `DocumentOutcome.per_teardown`/`trace`
(task 1.4) breaks its own literal set, enumerated there.

## 4. Post-verdict execution and failure class

Teardown runs after the scenario verdict is fixed, before the report finalises, on BOTH
verdict paths (pass and fail). Reuse the post-verdict apparatus, not a parallel mechanism:

- `ScenarioFailure::TeardownFailure { message: String }` — new apparatus class,
  `#[error("teardown-failure: {message}")]`, exit 2 (CLI `is_apparatus` arm). A teardown
  failure never masks or rewrites the recorded verdict.
- `DocumentOutcome` (runner.rs:419) gains `per_teardown: Vec<Result<(), ScenarioFailure>>` —
  one entry per finally item in declared order, every failure uniformly
  `Err(TeardownFailure)`: the runner wraps every teardown failure AND panic into that class
  (no raw driver errors leak through).
- When any teardown item failed, the FIRST failure also fills the existing
  `final_failure: Option<ScenarioFailure>` post-verdict slot — the structured record for
  callers and tests. Rendering never duplicates: per-teardown rows render every
  `per_teardown` failure (label `finally[i]`); the boot shutdown's own `Err` renders a
  shutdown-labeled row unconditionally (from the `Err`, not from the slot), and
  `record_shutdown_failure(&mut DocumentOutcome, ScenarioFailure)` — extracted for unit
  testing — writes the slot ONLY when `None`. So a teardown failure and a shutdown
  failure BOTH render (teardown rows first, then the shutdown row), each exactly once,
  exit stays 2; when shutdown is the only failure the rendered output is identical to
  today's slot-row output.

Panic containment: each finally item executes inside `catch_unwind` at the document
boundary (no `panic=abort` in profiles; `futures`/`tokio` available). A panicking item is
recorded `Err(TeardownFailure)` and the next item — and the next document — still runs.

## 5. Execution-order trace (spec-bless round 3)

`DocumentOutcome.trace: Vec<ScenarioEvent>`; `enum ScenarioEvent { Action(usize),
LogsEvaluated, VerdictFixed, Teardown(usize) }`, recorded strictly in execution order:
`Action(i)` when scenario action i's execution returns (pass or fail), `LogsEvaluated`
after the logs evaluation, `VerdictFixed` when the pass verdict is fixed — which happens
AFTER the logs evaluation (runner.rs:533-544: `verdict` is `None` when the logs
evaluation failed) — and `Teardown(i)` after finally item i completes. A failed scenario
records NO `VerdictFixed` — the absence is the witness that no verdict was fixed (a
failed logs evaluation is likewise no-verdict). Ordering assertions read the trace, never
wall-clock.

## 6. Boot selection (plan-review F3)

`run_scenario` (camel-cli commands/test/scenario.rs ~337-410) scans only `doc.scenario`;
its redis predicate recognises only the `Validate` redis target. A redis-del-only or
finally-only catalog-needing document would take the smoke path without a catalog. Extend
the scan: `has_sql`/`has_surreal` also consider `doc.finally`; `has_redis` also accepts
`RedisDel` (and finally redis-del), feature-aware cfg twins unchanged. Feature-aware
selection tests pin: finally-sql routes full boot; finally-sleep-only stays smoke;
scenario redis-del alone routes full boot.

## 7. Test design constraints (plan-review F2/F4/F5)

- Parser tests that load `sql:`/`redis:`-carrying documents run under their feature
  (`--features sql` / `--features redis`): the feature-off twins reject valid actions at
  load. Feature-free parser tests use `sleep`-only `finally:` lists. The demand gate is NOT
  weakened.
- `FakeAdapter::send` emits no tracing events, and `log_capture::ensure_capture_subscriber`
  is a process-global first-wins seat. `LogsEvaluated` ordering is covered through an
  integration test in the `tests/log_assertion_test.rs` style (shared install-first helper
  + `RUN_LOCK`), NOT through the fake adapter.
- The failing-scenario teardown test uses a `validate` mismatch on a VARIABLE target with
  the variable PRE-SEEDED to a value that mismatches the expectation (verdict-class
  `validation-mismatch`, feature-free, no adapter). An UNSET variable target yields the
  apparatus class `VarUnresolved` (runner.rs:1191-1195) — wrong class for the
  verdict-path witness. A failing `send` would be apparatus `ActionTransport` — also
  wrong.

## 8. Docs and ADR amendment (task 1, before code)

ADR-0069 amendment: §7 failure taxonomy gains the `teardown-failure` bullet (after
`shutdown-failure`); §8 item 4 amended — the redis prepare activation record: the delete-only
`del` action lands, assertions-only no longer the whole family; §9 gains the teardown
ownership paragraph (author-declared `finally:` keeps the harness never cleaning resources
it does not own); §11 Adopt/Defer gains the post-verdict and no-flush rationale.
`CONTEXT-MAP.md` line ~112 ADR-0069 index entry gains the amendment marker naming
`teardown-failure`. `docs/src/testing/scenario-documents.md` gains "Teardown with
`finally:`" under Isolation and teardown; `scenario-redis.md` documents the `del` action
(replacing the "assertions only" sentence); CHANGELOG gains the user-visible entry.

## 9. Non-goals

No parallel execution, no namespaces binding (rc-ql8eq), no pattern delete, no ephemeral
Redis provisioning, no `before:` key, no change to existing validate targets, no
sql/surreal grammar change beyond `finally:` acceptance.
