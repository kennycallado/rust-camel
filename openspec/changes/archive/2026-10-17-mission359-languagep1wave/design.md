# Design: mission359-languagep1wave

## Approach

### A — Simple template mode (Phase 1)

Add `parser::parse_template(input) -> Result<Expr, LanguageError>`:

1. **Outer trim (F4)**: trim the whole source first (as `parse` does) to absorb
   a YAML block scalar's final newline. Internal whitespace is verbatim.
   Empty-after-trim → `ParseError`.
2. **Whole-slot rule (F2/F13)**: if the trimmed source is exactly one
   `${...}` token spanning it, return the parsed slot atom directly
   (`Expr::Header`, `Expr::Body`, `Expr::BodyField`, `Expr::ExchangeProperty`,
   `Expr::ExceptionMessage`, `Expr::LanguageDelegate`), so evaluation returns
   the native `Value` unchanged — `Null`, JSON arrays, and numbers included.
   A missing slot therefore yields `Null`.
3. **Mixed template (F2/F13)**: otherwise split into literal segments and
   `${}` slots. Literal text is copied verbatim (whitespace, quotes, `=`, `<`,
   `null`, `true` are not special). A missing slot in a mixed template renders
   the empty string (existing interpolation rule).
4. **Raw literals (F3, approved breaking)**: a source with no `${}` slot is a
   raw string (`Expr::StringLit`). `'abc'` → `'abc'`, `true` → `"true"`,
   `42` → `"42"`, `null` → `"null"`, `"a\n"` → the two-char escape text. This
   is the intended value/expression semantic.
5. **Unknown language (F14)**: a slot that parses as `lang:expr` with a valid
   lowercase language name is a `LanguageDelegate`; an unregistered language is
   an **evaluation-time** `EvalError`, not a compile error. A slot naming
   nothing (`${unknown}`) stays a compile-time `ParseError`.
6. **Unclosed slot (M3)**: an unclosed `${` in a source with no completed slot
    and preceding literal text (`Got ${body`) remains literal text (existing
    behavior). A source beginning with an unclosed `${` (`${body` or `${`),
    or an unclosed `${` after a completed slot (`${header.h} ${`), is a
    `ParseError`. This unclosed rule takes precedence over the raw-literal
    rule: a source containing an unclosed `${` is never classified as a raw
    literal.

`create_expression` calls `parse_template`; `create_predicate` calls `parse`
(unchanged).

**Test migrations (F3)**: the following existing expression tests change under
raw/template semantics — `lib.rs` lines 295, 304, 313, 322, 331, 383, 497,
668, 677, 686, 789, 1206, 1220, 1259. Predicate-style comparisons
(383/1206/1220/1259) and the non-finite-number check (789) migrate to
`create_predicate`; the literal-shape tests update their assertions to raw
text (and are renamed). Reason recorded in the task commit.

### A/DSL — top-level `${}` gate (F6/F7)

Add one crate-internal helper in `camel-dsl/src/yaml.rs`, used by the shared
`route_step_to_declarative_step` path (JSON reuses it via
`route_dsl_to_declarative_route`):

`fn bare_string_value_source(value: serde_json::Value) -> ValueSourceDef`

- Applies **only to a top-level `Value::String`**: if it contains `"${"`, return
  `ValueSourceDef::Expression(LanguageExpressionDef { language: "simple", source })`;
  else `Literal`.
- Non-string top-level values and **nested** strings inside object/array
  literals stay literal (F7). It is used by the `SetBodyData::Literal` arms of
  `SetBody`/`Transform` **and** by the `literal` branch of `parse_value_source`
  (I4), so the object `value:` forms (`set_body: {value: ...}`,
  `set_header: {value: ...}`, `set_property: {value: ...}`) and the config
  `log: {message: ...}` form are gated identically. `log:` keeps its bare
  string → Simple expression route.
- Env substitution runs *before* route parsing and is trusted (ADR-0032); a
  substituted `${...}` becomes a template slot. **Routes substitute `${env:...}`
  ONLY**: `${arg:...}` is not substituted on the route path (empty argument
  lookup, `env_interpolation.rs`), so an unescaped `${arg:NAME}` fails to load;
  args are substituted only on the `camel-cli` job path (`job/document.rs`,
  `interpolate_with_args`). The `$${...}` escape yields literal `${...}` text
  that the gate then treats as a `lang:` slot (no literal path until rc-luew6).
- **Breaking (I4)**: the top-level gate changes previously-literal strings that
  merely contain `${...}` — for example `set_body: "echo ${HOME}"` or a JS
  template payload — from accepted literals to compile-time `unrecognized
  token` failures. The helper commit carries `!:` / `BREAKING CHANGE:`; this is
  recorded in the risk budget.

### A/callsite migration (F1/F8)

- `camel-processor/src/aggregator.rs:845-852`: replace
  `create_expression(expr)` + `value.as_bool().unwrap_or(false)` with
  `create_predicate(expr)` + `predicate.matches(incoming).await` (strict bool;
  `Err(LanguageError)` propagates as `CamelError::ProcessorError` carrying the
  Simple type-mismatch text — the aggregator has no `EvalMeta`, so no
  `ExpressionFailed` variant; pre-existing gap filed as rc-dhppc). Preserve the
  existing `"${body} == 'DONE'"` source (line 2444 test) — as a predicate it
  still works. `create_expression` at `:782` is a genuine value use and stays.
- `camel-core`: `compile_language_expression` and the value wrappers in
  `step_resolution.rs` (`compile_value_source`, `compile_sort_expression`,
  `compile_message_id_expression`, `compile_key_expression`) plus
  `route_compiler_ext` correlation/sort/sequence are value uses and stay
  Expression/template. Predicate verbs already use
  `compile_filter_predicate`/`compile_language_predicate`. Add per-wrapper tests
  in the `step_resolution` test module and a resequence test in the
  `route_controller_tests.rs` module (`route_controller::tests`, included from
  `route_controller.rs`; `route_compiler_ext.rs` has no test module).
- `camel-component-file::resolve_filename` already uses
  `SimpleLanguage::create_expression`; template mode makes
  `${header.id}-01.txt` → `X-01.txt`.
- `camel-integration-test`: one end-to-end route exercising bare `log:`/
  `set_body:` interpolation through the EXISTING scenario/log-capture harness
  (`tests/common::run_logs_document` + `lock_run` + a
  `tests/fixtures/logs/` fixture), not a hand-built runtime.
- `log:` forms (m1): a bare `log: "..."` string AND `log: {message: "<plain>"}`
  are ALWAYS Simple expressions (existing `LogMessageData::Literal` arm); only
  `log: {message: {value: "..."}}` goes through `parse_value_source` and the
  top-level `${}` gate.

### B — JS normalized changed-only write-back (F5/I2)

Do **not** add a `JsEngine` trait method and do not add a separate blocking
normalization job. Capture the normalized pre-run snapshot inside the **same
worker evaluation job**, immediately before the script runs, **only for
mutating evaluation (`sentinel.is_none()`)**; read-only evaluation does not
capture it (the field is set to the inbound body), using the conversion the
result body already passes through. Minimal plumbing: add one
public field `body_snapshot: serde_json::Value` to `JsEvalResult`
(`engine.rs`), populated in `worker.rs::eval_cached` by a NEW body-only helper
`fn extract_body_snapshot(ctx: &mut Context, detector: Option<&ArrayDetector>)
-> Result<serde_json::Value, JsLanguageError>` that reads the installed
`camel.body` and reuses `js_to_value_with_detector` (not the state-wide
`extract_camel_state`), called before evaluation, and by the three other
`JsEvalResult { .. }` construction sites (`worker.rs` test blocks,
`expression.rs` mocks, `language.rs`). This is an explicit public-shape
change to bless. In `JsMutatingExpression::evaluate`, compare
`result.body != result.body_snapshot` and write `value_to_body(&result.body)`
only when they differ; `value_to_body(Value::Null)` → `Body::Empty`.

Null semantics (I1): JS exposes `Bytes` and `Empty` as `null`, so a JS
`camel.body = null` on those bodies has `result.body == body_snapshot == Null`
and is a no-op; on `Text`/`Json`/`Xml` it makes the body `Body::Empty`. Rhai
exposes `Bytes` as a Blob and `Empty` as unit: `body = ()` on Rhai `Bytes` is a
real change → `Body::Empty`; on Rhai `Empty` it is unchanged.

### C — Rhai (F9–F12)

- `src/string_shadow.rs`: register `trim(&str) -> String`, called at the end of
  `create_base_engine` after `StandardPackage`/json host. **Trim only, no
  `_inplace` aliases (F9).** Document `make_upper`, `make_lower`, `clear`,
  `truncate`, `remove`, `crop`, `replace`, `pad` as in-place with pure
  alternatives (`to_upper`, `to_lower`, `sub_string`). The shadow applies to
  the mutating engine too (test).
- `eval_sync` pushes the `properties` map into the read-only scope. Extract the
  `header()`/`property()` registration into
  `fn register_snapshot_readers(engine: &mut Engine, headers: rhai::Map,
  properties: rhai::Map)` and call it from `create_eval_engine` and
  `eval_mut_sync` with the pre-eval snapshot maps (rc-v4phf).
- Rejection hint (F11): **rhai only**. Change it to name the working surface
  `headers["k"] = value`, `properties["k"] = value`, `body = value`. The JS hint/error
  is already accurate and is untouched.
- Read-only AST walk (F10): extend the existing
  `find_read_only_mutation` to also reject `Stmt::Assignment` (including
  `OpAssignment`) whose LHS root name is `body`, `headers`, or `properties`,
  resolved by descending `Variable`/`Index{lhs}`/`Dot{lhs}`. **Conservative**:
  a local variable shadowing those names is also rejected (documented
  false-positive). This is a **Rhai-only addition**: JavaScript already has a
  compile-time AST rejection of the statically-decidable `camel.*` surface
  (`camel-language-js/src/expression.rs::reject_read_only_mutation`, tests
  1184-1326) plus a runtime proxy for undecidable dynamic dispatch; that JS
  behavior is untouched. Rhai has no runtime proxy, so its walk is the only
  mechanism for these forms. Method-call mutations (`headers.clear()`) remain a
  gap → rc-qpjps. The `has_header()`/`has_property()` drift is rc-w1mds (F12).

### Docs and context (M4/M5/M6/M7)

- M4: add "Simple template mode" vs "Simple predicate mode" glossary entries in
  `crates/languages/CONTEXT.md` and the Simple crate `CONTEXT.md`, explicitly
  disambiguating Simple template mode from the MiniJinja template engine
  (ADR-0047) and RouteTemplate JSON-tree substitution (ADR-0008).
- M5: `rhai.md` documents pure alternatives where they exist and the
  copy-then-mutate pattern for `replace`/`pad`/`remove`/`clear`; records that
  `trim_start`/`trim_end` and the `*_ascii`/`trim_to_string` variants are
  absent from rhai 1.26.1.
- M6: document the top-level `${}` gate (including `set_header_if_absent`) on
  the real YAML-DSL value pages `docs/src/yaml-dsl/step-verbs/basic.md`
  (body/header/property) and `docs/src/yaml-dsl/step-verbs/transformation.md`
  (transform), and add a decision entry to `crates/camel-dsl/CONTEXT.md`.
- Bench: `crates/languages/camel-language-simple/benches/expression_eval.rs`
  holds obsolete comparison scripts (`header_eq`, `header_gt`, `body_contains`)
  on `create_expression`; migrate them to the predicate bench or rename to
  value semantics.
- M7: document that rhai script readers observe the pre-run snapshot while JS
  readers observe the live mutated maps.
- Literal-`${` workaround: until rc-luew6, emit a literal `${...}` through an
  explicit `language:` + `source:` value source with a quoted rhai/js string.
  Because env interpolation walks EVERY string before parsing, an env/arg token
  inside that string must be escaped as `$$` (for example
  `language: rhai, source: '"$${env:X}"'` yields the literal `${env:X}`); a
  non-env token needs no escape (`source: '"${header.h}"'` yields literal
  `${header.h}`). `$$` is NOT a Simple-template escape: at the Simple source
  level after substitution, `$${header.h}` is `$` plus a live slot. In a
  route file, the env pass first turns `$$` into `$`, so `$${header.h}` reaches
  Simple as the whole slot `${header.h}` and retains its native value.
  Write `$$$${header.h}` in a route file to render `$` plus the slot.
- rc-zwy7e (JS map write-back + float drift) is NOT absorbed. This change
  normalizes the **body** only (`JsEvalResult.body_snapshot`); the analogous
  header-map/property-map normalization stays in rc-zwy7e, which has been
  annotated with this design reference.

## Affected crates

- `camel-language-simple`: `parse_template`, wiring, test migration, corpus.
- `camel-dsl`: top-level `${}` helper (shared YAML/JSON), tests.
- `camel-processor`: aggregator predicate migration, core callsite tests.
- `camel-core`: callsite audit/tests.
- `camel-component-file`: filename regression test.
- `camel-integration-test`: end-to-end bundle route test.
- `camel-language-js`: normalized changed-only write-back; null→Empty.
- `camel-language-rhai`: `string_shadow`; read surface; walk; hint.
- `docs/src/languages/{simple,rhai}.md`,
  `docs/src/yaml-dsl/step-verbs/{basic,transformation}.md`,
  `camel-language-simple/CONTEXT.md`, `camel-language-rhai/CONTEXT.md`,
  `crates/languages/CONTEXT.md`, `crates/camel-dsl/CONTEXT.md`,
  `camel-language-simple/benches/expression_eval.rs`.

## Architecture boundaries

Changes stay in **Languages** and the **DSL** that compiles value sources, plus
the Processor/Core that *invoke* the language SPI. No Runtime, Component, or
Services contract changes. The template/predicate split keeps the
`Language`/`Expression`/`Predicate` SPI signatures unchanged; `MutatingExpression`
is unchanged. Rhai/JS stay inside ADR-0032: template literals and substituted
env/arg values are trusted operator config, while `${}` slots evaluate only
trusted fetch expressions. Predicates stay strictly boolean (no rc-rjcur
behavior change). DSL gating is a compile-time control-plane decision only. ADRs:
ADR-0032 (trust boundary), ADR-0006 (JS `script:`), ADR-0012 (log levels),
ADR-0004 (hot-reload snapshot; unchanged).

## Phases

### Phase 1: Simple template engine + DSL gate
- **Goal:** A core (F2/F3/F4/F6/F7/F13/F14) and rider rc-rjcur.
- **Dependencies:** none.
- **Externally-visible types/interfaces:** none public; crate-internal
  `parse_template`, `bare_string_value_source`.
- **Deliverable:** parser split, DSL gate, docs, test migration, corpus.
- **Exit-criteria:** the Simple + DSL A probe tests pass and predicate tests
  are unchanged. Phase 1 does NOT turn the whole workspace green: these tests
  are EXPLICITLY KNOWN-RED until their owning Phase-2 tasks land, and Phase-1
  workers MUST NOT touch them — `camel-processor` aggregator
  `evaluate_completion_predicate_or_combines_any` (~2430) and
  `completion_predicate_expr_completes_on_match` (~2515) (owned by Task 2.1);
  `camel-core` `expression_failure_propagation_test.rs` ~279 / ~794 and
  `verb_diagnostics("script_readonly")` ~409-412 (owned by Task 2.2).

### Phase 2: A callsite migration + integration (F1/F8)
- **Goal:** aggregator predicate, core audit, file regression, bundle route.
- **Dependencies:** Phase 1.
- **Externally-visible types/interfaces:** none public.
- **Deliverable:** migrated callsites, regression/integration tests.
- **Exit-criteria:** aggregator strict predicate tests pass; file filename
  test passes; integration route passes.

### Phase 3: JS body write-back + Rhai fixes (B, C: F5/F9–F12)
- **Goal:** rc-dec7b, rc-kbdd9, rc-v4phf, rc-y0c8c.
- **Dependencies:** none — B and C are independent of Phases 1–2. Phase 3 is
  executed last by ordering only (the integration route in Phase 2 uses the
  Phase-1 template engine, not Phase 3).
- **Externally-visible types/interfaces:** `JsEvalResult` gains a public
  `body_snapshot: serde_json::Value` field (breaking public-field addition; no
  `JsEngine` trait signature change). No other public surface changes.
- **Deliverable:** JS write-back, rhai `string_shadow`, read surface, walk, docs.
- **Exit-criteria:** B/C probe tests pass; clippy clean.

## Alternatives considered

- Reuse predicate grammar and strip operators — cannot tell literal `->` from
  an operator and loses `null`.
- Always-template every bare string — churns `Literal` assertions and route
  metadata; `${}`-gated, top-level-only keeps plain/nested literals stable.
- JS engine-side assignment flag or a public engine normalize method — both
  were rejected (I2): the snapshot is captured in the same worker job and
  carried on `JsEvalResult`, so no new trait method and no extra job; a
  value-diff after normalization treats a same-value assignment as a no-op.
- Runtime-immutable rhai read-only maps (JS-style) — rhai has no such facility;
  AST-walk rejection is the available mechanism.
