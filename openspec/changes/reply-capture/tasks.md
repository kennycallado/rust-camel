# Tasks: reply-capture

Single-phase change (no `## Phase N` headings by design; see design.md).

## camel-cli

### Task 1: Parsing layer — `expectReply` on inputs

**Files:**
- `crates/camel-cli/src/commands/test/document.rs` (modified)
- `crates/camel-cli/src/commands/test/document_tests.rs` (modified — add tests)

**Steps:**
1. In `document.rs`, add `#[serde(deny_unknown_fields, rename_all = "camelCase")] pub struct ExpectReply { #[serde(default, deserialize_with = "deserialize_option_input_body")] pub body: Option<InputBody>, pub headers: Option<HashMap<String, serde_json::Value>> }`. Reuse the existing `deserialize_option_input_body` helper (the same one inputs use for optional bodies); if its exact name differs in the file, use the actual existing helper for `Option<InputBody>` fields verbatim.
2. Add field `pub expect_reply: Option<ExpectReply>` (serde camelCase → `expectReply`) to `TestInput`.
3. Extend `parse_test_document` validation: (a) an `expectReply` with neither `body` nor `headers` → `TestDocError` via a new variant `TestDocError::InvalidReply(String)` mirroring `InvalidBeans` with message `expectReply must declare body or headers`; (b) relax the expects-mandatory check: a document MAY have empty/omitted `expects` when at least one input declares `expectReply`; a document with neither endpoint expectations nor any `expectReply` keeps failing with the existing expects-mandatory message (update the message to append ` unless an input declares expectReply`).
4. No runner changes in this task.

**Tests:** (in `document_tests.rs`; fail before steps 1-3, pass after)
- `expect_reply_absent_keeps_behavior`: minimal doc without `expectReply` → parses, `expect_reply` is `None`.
- `expect_reply_body_parses`: input with `expectReply: {body: "done"}` → parses; `expect_reply().body` is text `done` (assert against the `InputBody`'s text form the same way input-body tests do).
- `expect_reply_json_body_parses`: input with `expectReply: {body: {status: ok}}` (YAML map) → parses; body's JSON form contains `status == "ok"` (mirror how input JSON-body tests assert).
- `expect_reply_headers_parse_json_values`: input with `expectReply: {headers: {count: 2, flag: "yes"}}` → parses; headers map holds `count → Value::Number(2)` and `flag → Value::String("yes")`.
- `expect_reply_empty_rejected`: input with `expectReply: {}` → error message contains `expectReply must declare body or headers`.
- `expect_reply_unknown_field_rejected`: input with `expectReply: {body: "x", bodi: "y"}` → document error mentioning the unknown field.
- `reply_only_document_valid`: doc with NO `expects` key, one input with `expectReply: {body: "done"}` (route inline, minimal) → parses without error.
- `no_expects_no_expect_reply_rejected`: doc with NO `expects` and inputs WITHOUT `expectReply` → error message contains the updated mandatory message incl. `unless an input declares expectReply`.

**Acceptance:**
- `cargo test -p camel-cli --lib` passes (count grows by 8).
- `cargo fmt --check --all`, `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 1

### Task 2: Capture + evaluation + report plumbing

**Files:**
- `crates/camel-cli/src/commands/test/runner.rs` (modified)
- `crates/camel-cli/src/commands/test.rs` (modified)
- `crates/camel-cli/tests/test_replies.rs` (new)

**Steps:**
1. `deliver_input` returns `Ok(Exchange)` (the oneshot reply) instead of `Ok(())`; the `Ok(_)` discard becomes `Ok(reply) => return Ok(reply)`. Retry/race and `Err` (doc error exit 2) logic untouched.
2. `run_phases` collects replies: `Vec<Exchange>` in input order (delivery stays strictly sequential).
3. Reply evaluation in the existing evaluate phase, via ONE production helper in runner.rs: `fn evaluate_reply_expectation(expect: &ExpectReply, reply: &Exchange, label: &str) -> EndpointResult` — selects the reply message as `reply.output.as_ref().unwrap_or(&reply.input)`, checks the expected body via `fn reply_body_eq(expected: &InputBody, actual: &Body) -> bool` (new runner.rs function, ~8 lines, variant-tagged equality mirroring camel-mock's private `body_eq`; text-to-text exact; `Json/Json` arm = serde_json structural equality; exporting camel-mock's would violate the no-camel-mock-change boundary) and/or the expected headers as an exact submap of `message.headers` (serde_json::Value equality), and returns one result row labeled `label` (= `reply[i] <input.to>`, e.g. `reply[0] direct:in`) with pass/fail + a deterministic failure detail (sorted keys) naming expected vs actual (body or headers, whichever mismatched). The runtime loop calls `evaluate_reply_expectation` per asserted input; the unit test below calls it directly.
4. `TestDocResult` carries reply results by REUSING the existing `EndpointResult` row shape: reply rows are appended to `endpoint_results` with `endpoint` holding the reply label `reply[i] <input.to>` (e.g. `reply[0] direct:in`); the driver in `test.rs` prints one `PASS`/`FAIL` line per reply row and counts them into the `N passed, M failed` summary (no driver branching needed — rows flow through the existing path). Failed reply = assertion failure (exit-1 class). Documents with no `expectReply` produce zero reply rows (byte-identical output to today).
5. Docs-of-record for implementer: the failure detail must be deterministic (sorted keys) so tests can assert substrings.

**Tests:**
- runner.rs inline `#[cfg(test)]` unit tests (precedent: beans.rs):
- `reply_output_message_precedence` (UNIT, in runner.rs): construct a hand-built `Exchange` with `input` body `A` and `output` body `B`; call `evaluate_reply_expectation(&ExpectReply { body: Some(B), headers: None }, &exchange, "reply[0] direct:in")` → row passes; with expected body `A` → row fails. Pins output-first precedence at the evaluation boundary regardless of DSL reachability (no lean-set step sets `exchange.output`).
- Integration tests in `tests/test_replies.rs` (TDD — write first following test_beans.rs/test_intercepts.rs conventions: temp_dir, run helper, `#[tokio::test(flavor = "multi_thread")]`, `// allow-unwrap`, `steps: - to:` DSL):
- `reply_captured_not_asserted_without_expect_reply` (RED guard first — must pass BEFORE implementation: run a doc without `expectReply`, verify behavior/output identical to today: PASS lines for mock endpoints only, no reply lines).
- `reply_body_asserted`: route `from: direct:in` steps `set_body: {value: "enriched"}` then `to: mock:out`; input `{to: direct:in, body: "x", expectReply: {body: "enriched"}}`; `expects: {mock:out: {count: 1}}` → run passes; result carries a passing reply row `reply[0] direct:in`. RED before implementation: expectReply unknown field (parse fails) or no reply row — observe actual RED reason and record in report.
- `reply_body_mismatch_exit_1`: same doc but `expectReply: {body: "wrong"}` → run result has one failed reply row; doc-level outcome is assertion failure (the equivalent of exit 1 — assert the runner result shape the CLI maps to exit 1), FAIL line present.
- `reply_headers_asserted`: route steps `set_header: {key: stamp, value: "yes"}` then `to: mock:out`; input `expectReply: {headers: {stamp: "yes"}}` → passes.
- `reply_json_body`: route steps `set_body: {value: {status: ok}}` (JSON form); input `expectReply: {body: {status: ok}}` → passes.
- `reply_composes_with_expects`: doc with both `expects: {mock:out: {count: 1, bodies: ["enriched"]}}` and `expectReply: {body: "enriched"}` → both rows pass, exit-equivalent 0.
- `multiple_replies_pair_by_order`: ONE fixture shape — two routes: `from: direct:first` steps `set_body: {value: "first-done"}` then `to: mock:oa`; `from: direct:second` steps `set_body: {value: "second-done"}` then `to: mock:ob`; two inputs — first `{to: direct:first, body: "x", expectReply: {body: "first-done"}}`, second `{to: direct:second, body: "y", expectReply: {body: "second-done"}}` → both reply rows pass in input order.
- `reply_header_mismatch_is_assertion_failure`: route steps `set_header: {key: stamp, value: "no"}` then `to: mock:out`; input `expectReply: {headers: {stamp: "yes"}}` → the runner result carries exactly one FAILED reply row in `endpoint_results` (assertion-failure class: no `doc_error` on the result). Exit-code CLI pin for this class is Task 3's `reply_mismatch_exits_1_cli` (body-mismatch doc) — the class is shared.
- `reply_with_bean_stub`: doc with `beans: {enricher: {kind: setBody, config: {body: enriched}}}`, route `from: direct:in` steps `bean: {name: enricher, method: enrich}` then `to: mock:out`, input `expectReply: {body: "enriched"}` → passes (cross-feature composition with bean-test-registry).

**Acceptance:**
- `cargo test -p camel-cli --test test_replies` passes 9/9.
- `cargo test -p camel-cli --lib reply` passes (1 unit test: output precedence).
- `cargo test -p camel-cli --test test_beans --test test_intercepts --test test_runner` still pass.
- `cargo test -p camel-cli --lib` unchanged count from Task 1.
- `cargo fmt --check --all`, `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 2

### Task 3: CLI-level exit codes + execution matrix

**Files:**
- `crates/camel-cli/tests/test_replies.rs` (modified — add tests)

**Steps:**
1. Add CLI subprocess tests pinning exit codes and output lines through the real binary (same invocation shape as test_beans.rs subprocess tests).
2. Scenario-mapping comment block at top of test_replies.rs: every delta scenario → owning test (incl. Task 1 parsing tests and MODIFIED scenarios).

**Tests:**
- `reply_mismatch_exits_1_cli`: subprocess on the mismatch doc → exit code 1, stdout contains `FAIL` and `reply[0]`, summary counts the reply row.
- `reply_only_document_exits_0_cli`: subprocess on the reply-only doc (no expects) → exit 0, one PASS reply line, no endpoint lines.
- `delivery_error_still_exits_2_with_reply_declared`: fail-bean doc with `expectReply` declared → exit 2, no PASS/FAIL lines for endpoints or replies (MODIFIED exit-codes skip clause).
- `multi_doc_reply_isolation`: two docs in one invocation — a.test.yaml reply-only, b.test.yaml expects-only → both pass, exit 0.
- `empty_expect_reply_exits_2_cli`: subprocess on `expectReply: {}` doc → exit 2, stderr contains `expectReply must declare body or headers`.

**Acceptance:**
- `cargo test -p camel-cli --test test_replies` passes 9 + 5 = 14/14.
- `cargo test -p camel-cli` (full integration suite) passes.
- `cargo fmt --check --all`, `cargo clippy -p camel-cli -- -D warnings` exit 0.
- Scenario-mapping comment complete (every delta scenario owned).

- [x] 3

### Task 4: Docs, example, and citations

**Files:**
- `docs/src/testing/index.md` (modified)
- `examples/yaml-dsl/config/reply-demo.yaml` (new)
- `examples/yaml-dsl/config/reply-demo.test.yaml` (new)
- `crates/camel-cli/CONTEXT.md` (modified)

**Steps:**
1. In `docs/src/testing/index.md`, add `### Reply assertions` (sibling of `### Intercepts` / `### Bean stubs` under `## Declarative camel test`): document `expectReply: {body?, headers?}` (exact-match v1; string-or-JSON body; JSON-valued headers submap), the reply-message contract (`output` when the route set one, else the final input message; nothing in the lean set sets `output` today), pairing-by-delivery-order, failure classification (mismatch = exit 1 FAIL line; delivery error = exit 2, evaluation skipped), and the reply-only relaxation of `expects`. STE discipline.
2. Create `examples/yaml-dsl/config/reply-demo.yaml`: production-shaped route `from: direct:enrich` with steps `set_body: {value: "enriched"}` then `to: log:replied` — routes only.
3. Create `examples/yaml-dsl/config/reply-demo.test.yaml`: `routeFiles: [reply-demo.yaml]` (doc-relative), `intercepts: {log:replied: {skipTo: mock:replied}}`, one input `{to: direct:enrich, body: "plain", expectReply: {body: "enriched"}}`, `expects: {mock:replied: {count: 1, bodies: ["enriched"]}}` — shows reply + endpoint + intercept composing.
4. Verify end-to-end from worktree: `cargo run -p camel-cli -- test examples/yaml-dsl/config/reply-demo.test.yaml` → exit 0, PASS lines for `mock:replied` AND the reply row.
5. In `crates/camel-cli/CONTEXT.md`, extend the test-command pointer sentence(s) with reply assertions pointing to the docs section anchor.

**Tests:** (executable verification, not #[test])
- `example-runs-green`: the step-4 command exits 0 with both PASS lines.
- `lint-context-citations`: `cargo xtask lint-context-citations` exits 0.
- `cargo test -p camel-cli --test lint_corpus` stays 4/4 (new example must lint clean or get a justified baseline entry following the intercepts-demo precedent).

**Acceptance:**
- All three verification commands pass from the worktree.
- `cargo fmt --check --all` exits 0.
- Docs use STE; example pair mirrors intercepts-demo/beans-demo conventions.

- [x] 4
