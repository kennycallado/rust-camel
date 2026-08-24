# Tasks: stagec-lint

Single-phase change (no `## Phase N` headings by design; see design.md).

## camel-lint + camel-cli

### Task 1: Endpoint origin annotation + R-MOCK-IN-PRODUCTION rule

**Files:**
- `crates/camel-lint/src/route_view.rs` (modified)
- `crates/camel-lint/src/document.rs` (modified)
- `crates/camel-lint/src/rules/rmock.rs` (new)
- `crates/camel-lint/src/rules/mod.rs` (modified — add `pub mod rmock;`)
- `crates/camel-lint/src/diagnostic.rs` (modified)
- `crates/camel-lint/src/engine.rs` (modified)

**Steps:**
1. In `route_view.rs`, add `pub key: Spanned<String>` to `Endpoint` (additive; existing rules ignore it).
2. In `document.rs`, populate `key` in `endpoint_for` (document.rs:540 area) from the walker's `path` argument by: taking the FINAL dot-delimited segment of the path, then stripping a terminal `[i]` array index if present — so `routes[0]...steps.to` → `to`, `...endpoints[1]` → `endpoints`, object-form `enrich.uri` → `uri` (resulting keys: `to`, `from`, `uri`, `wire_tap`, `enrich`, `poll_enrich`, `endpoints`, `dead_letter_channel`). `key.span` = the endpoint URI's span (same span the Endpoint already carries). In `route_view.rs`, the route-level `from` slot's synthesized endpoint (route_view.rs:172-176 area) gets `key` = spanned `"from"` (span = the from URI's span).
3. In `diagnostic.rs`, add enum variant `RMock` with stable Display string `R-MOCK-IN-PRODUCTION` (baseline contract, never renamed), placed per enum convention.
4. Add `pub mod rmock;` to `rules/mod.rs` (current list: rdeprecated, rschema, rsecret, rsyn, ruriknown). Create `rules/rmock.rs`: unit struct `RMockRule` implementing `Rule` (model: `rules/rdeprecated.rs`). `analyze` guards `doc.parse_failure`, iterates `doc.route_view.endpoints()`, emits `Diagnostic { code: DiagnosticCode::RMock, severity: Severity::Warning, span: endpoint.uri.span.clone(), message: MIGRATION_MSG.to_string(), fix: None }` for every endpoint with `key.value` == `"to"` or `"endpoints"` AND `uri.value.starts_with("mock:")`. One diagnostic per occurrence. `MIGRATION_MSG` = `inline mock: send in production route; declare intercepts (skipTo/divertCopyTo) in a *.test.yaml instead - see the testing guide (docs/src/testing)`.
5. In `engine.rs` `with_default_rules`, register `.with_rule(Box::new(crate::rules::rmock::RMockRule))`; rename BOTH stale-count tests: `all_five_rules_registered` → `all_six_rules_registered` (assert six) and `all_five_rules_silent_on_valid_doc` → `all_six_rules_silent_on_valid_doc` (its mock-free fixture is the owner of the MODIFIED "Valid document yields no diagnostics" scenario — note that ownership in a comment).

**Tests:** (in `rmock.rs` inline `#[cfg(test)]`, following rdeprecated.rs test conventions — inline YAML route fixtures, engine built per test; fail before steps 1-5, pass after)
- `to_mock_warns_once_with_migration_message`: route with `- to: "mock:out"` → exactly one R-MOCK-IN-PRODUCTION Warning; span covers the `mock:out` URI; message contains `intercepts` and `skipTo`.
- `endpoints_recipient_list_warns_per_occurrence`: route with `endpoints: ["mock:a", "mock:b"]` → two diagnostics with distinct spans.
- `mock_with_query_params_warns`: `- to: "mock:out?count=2"` → one diagnostic.
- `two_to_mock_steps_warn_twice`: steps `to: "mock:first"` and `to: "mock:second"` → two diagnostics, distinct spans.
- `non_mock_send_silent`: `- to: "kafka:orders"` → zero RMock diagnostics.
- `non_interceptable_origins_silent`: route containing `wire_tap: "mock:tap"`, `enrich: "mock:enr"`, `poll_enrich: "mock:poll"`, `enrich: {uri: "mock:uri"}` (object form, second enrich step), `from: "mock:src"` (route 2+ via multi-route file), and `dead_letter_channel: "mock:dlq"` → zero RMock diagnostics.
- `parse_failure_skips_rule`: malformed YAML → RMock contributes nothing (R-SYN does).
- `engine_registers_six_rules`: `with_default_rules` engine exposes 6 rules (the renamed count test — engine.rs).

**Acceptance:**
- `cargo test -p camel-lint` passes (new count = existing + 7 new tests; one existing test renamed per step 5).
- `cargo check -p camel-lint` exits 0.
- `cargo fmt --check --all`, `cargo clippy -p camel-lint -- -D warnings` exit 0.
- Hex-arch boundary test still passes (`cargo test -p camel-core --test hexagonal_architecture_boundaries_test`).

- [x] 1

### Task 2: CLI fixture-path suppression + corpus baseline

**Files:**
- `crates/camel-cli/src/commands/lint.rs` (modified)
- `crates/camel-cli/tests/lint_corpus.rs` (modified)
- `crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron` (modified)
- `crates/camel-cli/tests/lint_test_doc_skip.rs` (modified — the CARGO_BIN_EXE lint CLI precedent)

**Steps:**
1. In `lint.rs`, add `pub fn is_stagec_exempt_path(path: &Path) -> bool` — true when any component pair in the path is `tests` followed by `fixtures`. After the engine run, filter out `DiagnosticCode::RMock` diagnostics when `is_stagec_exempt_path` holds for the linted file; other codes unaffected. Print behavior for filtered output: identical to zero findings.
2. In `tests/lint_corpus.rs`, apply the same `is_stagec_exempt_path` suppression to emitted diagnostics before baseline comparison (import from the crate — it is `pub`).
3. Add 4 baseline entries to `lint-corpus-baseline.ron` (per-file `(code, severity)` collapse): `examples/yaml-dsl/config/mock-demo.yaml`, `examples/yaml-dsl/config/intercepts-demo.yaml`, `examples/yaml-dsl/config/routes-eip-advanced.yaml`, `examples/json-dsl/config/routes-eip-advanced.json` — each `("R-MOCK-IN-PRODUCTION", "warning")` with justification comment: `inline to: mock: in a camel-run demo route; teaching fixture pending migration to test-doc intercepts (ADR-0064 Stage C warn phase)`.
4. Add CLI-level tests in `tests/lint_test_doc_skip.rs` (following its CARGO_BIN_EXE conventions): fixture-path suppression pins AND the delta-scenario owner for "test documents are skipped": extend the skip coverage with a second test whose `*.test.yaml` fixture contains an `intercepts:` block with `mock:` targets (`intercepts: {kafka:x: {skipTo: mock:y}}`), asserting the info skip line prints and stderr/stdout contain no `R-MOCK-IN-PRODUCTION`.

**Tests:**
- `mock_rule_warning_does_not_affect_exit_code`: lint a route file whose only finding is `to: mock:out` → diagnostic printed, exit 0 (pins the Warning/exit contract through the binary or run() — follow the file's existing invocation style).
- `fixture_path_suppresses_only_mock_rule`: a fixture under a `tests/fixtures/`-shaped temp path containing `to: "mock:result"` AND a real unknown-option error → no R-MOCK-IN-PRODUCTION, unknown-option Error still emitted, exit 1 (code-scoped suppression).
- `test_doc_with_mock_intercepts_skipped_no_rmock`: the new skip test from step 4 (delta scenario owner).
- Corpus: `cargo test -p camel-cli --test lint_corpus` green with the 4 new entries (emitted == baseline).
- `xslt_xj_fixtures_exempt`: corpus run contributes no R-MOCK entries for `crates/components/camel-xslt/tests/fixtures/xslt-param-namespace.yaml` or the xj twin (implicit in set-equality; assert via the green corpus run).

**Acceptance:**
- `cargo test -p camel-cli --test lint_corpus` passes (4/4 incl. set-equality with new entries).
- CLI lint tests pass; `cargo test -p camel-cli` full suite green.
- `cargo fmt --check --all`, `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 2

### Task 3: Docs, CONTEXT rule table, and ADR-0064 gap refresh

**Files:**
- `crates/camel-lint/CONTEXT.md` (modified)
- `docs/src/testing/index.md` (modified)
- `docs/adr/0064-two-tier-testing-contract.md` (modified)

**Steps:**
1. `crates/camel-lint/CONTEXT.md`: update ALL count/enum surfaces — intro "five lint rules" count, the rule table (new row: code `R-MOCK-IN-PRODUCTION`, severity Warning, meaning: intercept-replaceable inline mock send in a route file, origin scope `to`/`endpoints`, fixture-path exemption, escalation note per ADR-0064 §5), the DiagnosticCode language block (add `RMock`), the Severity language block (Warning list gains R-MOCK-IN-PRODUCTION), and the "How do I add a new lint rule?" section if it hardcodes the count.
2. `docs/src/testing/index.md`: in the Intercepts section area, add a short note that `camel lint` now warns `R-MOCK-IN-PRODUCTION` on inline `to: mock:`/`endpoints: mock:` sends in route files (exempt: `tests/fixtures/` paths and `*.test.yaml`), pointing to this guide for migration. STE discipline.
3. `docs/adr/0064-two-tier-testing-contract.md`: append a dated amendment AFTER the Known unit-tier gaps list — original gap lines stay unchanged; the amendment (repository convention, cf. ADR-0056 `> Amendment (date): ...` form) states that the rc-07qh (bean registry) and rc-66c5 (reply capture) gaps were closed by bean-test-registry and reply-capture, and that Stage C's warn-phase lint (this change) completes the §5 program. This is the deferred F1 from the reply-capture holistic review, sanctioned in proposal/design (docs scope).
4. Verify end-to-end from worktree: `cargo run -p camel-cli -- lint examples/yaml-dsl/config/mock-demo.yaml` → exit 0, output contains `R-MOCK-IN-PRODUCTION` warning line; `cargo run -p camel-cli -- lint crates/components/camel-xslt/tests/fixtures/xslt-param-namespace.yaml` → no R-MOCK line (fixture exemption through the real binary).

**Tests:** (executable verification, not #[test])
- `mock-demo-warns`: step-4 first command → exit 0 + warning line present.
- `xslt-fixture-exempt`: step-4 second command → exit 0 + no R-MOCK-IN-PRODUCTION in output.
- `cargo xtask lint-context-citations` exits 0.
- `cargo test -p camel-cli --test lint_corpus` stays green.

**Acceptance:**
- All verification commands pass from the worktree (real exit codes).
- `cargo fmt --check --all` exits 0.
- Docs use STE; ADR amendment follows house conventions.

- [x] 3
