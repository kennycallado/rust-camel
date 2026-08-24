# Design: stagec-lint

## Approach

1. **Endpoint origin annotation** (`crates/camel-lint/src/route_view.rs` +
   `document.rs`): `Endpoint` gains `pub key: Spanned<String>` recording the
   URI-bearing key it was emitted from (the walker's `path` segment with any
   `[i]` index stripped: `to`, `from`, `uri`, `wire_tap`, `enrich`,
   `poll_enrich`, `endpoints`, `dead_letter_channel`). `endpoint_for`
   (document.rs:540 area) already receives the path — it populates the field;
   the route-level `from` slot's synthesized endpoint (route_view.rs:172-176)
   gets `key = "from"`. Additive field; existing rules ignore it.

2. **Rule** (`crates/camel-lint/src/rules/rmock.rs`, new): unit struct
   `RMockRule` implementing `Rule`, modeled on `RDeprecatedRule`. `analyze`
   guards `doc.parse_failure`, walks `doc.route_view.endpoints()`, and emits
   one `Diagnostic { code: RMock, severity: Severity::Warning, span:
   endpoint.uri.span, message: MIGRATION_MSG, fix: None }` for every
   endpoint with `key.value` in `{to, endpoints}` whose `uri.value` starts
   with `mock:`. One diagnostic per occurrence (positional convention).
   `MIGRATION_MSG`: `inline mock: send in production route; declare
   intercepts (skipTo/divertCopyTo) in a *.test.yaml instead - see the
   testing guide (docs/src/testing)`.

3. **Diagnostic code** (`crates/camel-lint/src/diagnostic.rs`): new variant
   `RMock`, stable Display string `R-MOCK-IN-PRODUCTION` (baseline contract).

4. **Registration + count test** (`crates/camel-lint/src/engine.rs`):
   `.with_rule(Box::new(crate::rules::rmock::RMockRule))` in
   `with_default_rules`; `all_five_rules_registered` updated to assert six.

5. **Fixture-path suppression** (`crates/camel-cli/src/commands/lint.rs`):
   pub helper `pub fn is_stagec_exempt_path(path: &Path) -> bool` — true
   when any path component sequence is `tests/fixtures`. After the engine
   run, `camel lint` filters out `R-MOCK-IN-PRODUCTION` diagnostics for
   exempt files (other codes unaffected). The corpus test
   (`tests/lint_corpus.rs`) applies the SAME helper before comparing — so
   exempt files contribute no expected diagnostics and need no baseline
   entries. The engine stays source-only and path-free; suppression is a
   caller concern (spec-pinned).

6. **Corpus baseline**: 4 entries — `examples/yaml-dsl/config/mock-demo.yaml`
   (1 occurrence), `examples/yaml-dsl/config/intercepts-demo.yaml` (1),
   `examples/yaml-dsl/config/routes-eip-advanced.yaml` (6),
   `examples/json-dsl/config/routes-eip-advanced.json` (6) — 14 occurrences
   collapsing to 4 per-file `(code, severity)` entries, justification:
   "inline to: mock: in a camel-run demo route; teaching fixture pending
   migration to test-doc intercepts (ADR-0064 Stage C warn phase)".

7. **Docs**: `crates/camel-lint/CONTEXT.md` — all count/enum surfaces
   (intro rule count, rule table row for `R-MOCK-IN-PRODUCTION`, DiagnosticCode
   list +`RMock`, Severity Warning list, "How do I add a rule" count if
   hardcoded); `docs/src/testing/index.md` intercepts-section note that
   `camel lint` now warns on inline `to: mock:`/`endpoints: mock:` sends
   (excluding `tests/fixtures/` paths and `*.test.yaml`); `docs/adr/0064`
   dated amendment (original gap lines unchanged, ADR-0056 amendment form)
   recording rc-07qh/rc-66c5 closures + Stage C warn-phase completion.

## Affected crates

- `camel-lint`: route_view.rs (+`key` field), document.rs (populate),
  rules/rmock.rs (new, unit tests per-origin), diagnostic.rs (+1 variant),
  engine.rs (registration + count test).
- `camel-cli`: lint.rs (suppression helper + filter), corpus baseline
  entries, corpus test filter line.
- No other crate changes.

## Architecture boundaries

- camel-lint depends only on camel-api (hex-arch boundary test enforced);
  the `key` annotation is internal to the view — zero dependency changes.
- Warning severity never affects `camel lint` exit codes (0 clean / 1 Error
  only / 2 misuse) — the lazy-migration contract of ADR-0064 §5.
- Path-based suppression lives in the CLI caller, not the engine — the
  engine's source-only contract (`lint(source)`) is untouched.
- Test documents skipped via the existing shared predicate
  (`is_test_document`).
- Escalation to Error severity is a documented future change (gated on
  ecosystem conversion; the e_opus A1 gates rc-66c5/rc-07qh are closed by
  this branch's stack — recorded in the spec note, not encoded).

Single-phase change (one coherent slice, 3 tasks).

## Alternatives considered

- **Warn on every send-point key (no annotation)** — rejected: interception
  covers only `To`-compiling sends (endpoints.rs:107-123; WireTap
  125-151 and Enrich bypass it); flagging them would advise a migration
  that does not exist (e_opus round-1 Critical).
- **Reachability analysis from Camel.toml** — rejected: the lint CLI takes
  one file and the engine is source-only; the `tests/fixtures/` path
  convention is the honest syntactic proxy for "camel run never loads it"
  (ADR-0064 §5 fixture legitimacy).
- **Rewrite the 4 in-tree demo routes instead of baselining** — rejected:
  the demos teach the pattern the rule flags; baseline entries with
  justification are the established honest mechanism.
- **Error severity now / `--deny` flag** — rejected: ADR-0064 §5 lazy
  migration; escalation is a future severity flip, no flag surface today.
- **Interceptable-URI registry** — rejected per e_opus A3.
