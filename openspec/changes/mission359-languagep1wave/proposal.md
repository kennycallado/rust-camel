# Proposal: mission359-languagep1wave

## Why

The e_opus language-surface audit (ses_eda66ba13ffeLUdkgRDRfLHQkY) found three
P1 correctness bugs plus same-file companions. Planning probes reproduced all
of them (details in `design.md`):

- **A (rc-veg9m)**: `SimpleLanguage::create_expression` runs the *predicate*
  tokenizer, so `Got -> ${h}` TypeMismatches, `<order>${h}</order>` ParseErrors,
  `value is null: ${h}` loses `null`, `${h} 3 items` becomes `B3.0 items`, and
  literal quotes are stripped. rc-fy7lm is one symptom; rc-yhvag is the
  inconsistent bare-string rule.
- **B (rc-dec7b)**: the JS mutating path always rewrites the body — header-only
  JS turns `Body::Xml` into `Body::Text` (losing `application/xml`), and
  `camel.body = null` yields `Body::Json(Null)` vs rhai `Body::Empty`.
- **C (rc-kbdd9)**: rhai `trim()` mutates in place and returns unit, so
  `x.trim() != ""` is always true; `header()`/`property()` are missing from
  script steps (rc-v4phf) and read-only map writes are silently dropped
  (rc-y0c8c).
- **Rider (rc-rjcur)**: `simple.md`/`CONTEXT.md` promise predicate truthiness
  and `== true` coercion the code does not do.

Spec-bless verdict `BLESS-WITH-FIXES` (ses_eda206ea5ffel4E9luYvREOYjZ)
required additional resolution, folded in below.

## What Changes

- **A**: add TEMPLATE parser mode (literal text + `${}` slots) for
  expressions. Exactly one whole `${...}` source returns the slot `Value`
  unchanged (Null, arrays, numbers included); every other template renders a
  string. Whole sources that are *pure literals* (`'abc'`, `true`, `42`,
  `null`, quoted escapes) become raw text — an approved breaking semantic.
  The outer `trim` is retained (YAML block final-newline absorber); internal
  whitespace is verbatim; empty is a parse error. Predicates keep the current
  grammar.
- **A/DSL**: a bare top-level string containing `${}` compiles to a Simple
  template in every value source (`log:`, `set_body:`, `set_header.value`,
  `set_property.value`, `transform:`); nested object/array literal strings stay
  literal. One helper serves the YAML and JSON route compilers. `log:` stays
  Simple. Env/job-argument substitution is trusted (ADR-0032) and accepted;
  the `$${` escape behavior is documented.
- **A/callsites (F8)**: the aggregator completion predicate moves to
  `create_predicate().matches()` (strict bool, errors propagate). Core
  expression callsites are audited: value verbs stay Expression/template,
  predicate verbs stay Predicate. File filename `${header.id}-01.txt` →
  `X-01.txt` is a regression. A bundle-level end-to-end route test is added.
- **B**: JS body write-back only when the result differs from the normalized
  pre-run snapshot captured in the **same evaluation job** (no new engine trait
  method, no extra blocking job), so an unchanged integral-float JSON body
  (`10.0`) is preserved; `null` maps to `Body::Empty`; a JS null on Bytes/Empty
  is a no-op (Rhai `body = ()` on Bytes becomes `Empty`, on Empty unchanged).
- **C**: pure-shadow `trim()` only (no `_inplace` aliases), other in-place
  string methods documented; the mutating engine gets the shadow too. Rhai
  read-only exposes `properties` and the script engine registers
  `header()`/`property()`; the rhai-only rejection hint names the working
  map-assignment surface; read-only assignments to names `body`/`headers`/
  `properties` are rejected conservatively (locals included).
- **Docs**: align `simple.md`/`CONTEXT.md` strict predicates; update
  `rhai.md` string-method and read-surface sections.

**Excluded:** rc-2yb8q, rc-u7bsf, rc-zwy7e, `has_header()`/`has_property()`
implementation (rc-w1mds), method-call mutation rejection (rc-qpjps), and
preserving substituted literal `${...}` (rc-luew6).

## Acceptance criteria

- All Phase-1/2/3 probe symptoms are regression tests that pass; the fourteen
  listed Simple expression tests are migrated or their assertions updated and
  the change reason is recorded.
- Aggregator completion predicate is strict and propagates errors.
- JS header-only preserves `Body::Xml` and integral-float JSON; `body=null`
  yields `Body::Empty`.
- Rhai `x.trim()` returns a string; script-step `property()`/`header()` and
  read-only `properties["level"]` works; read-only `headers["x"] = "1"` is
  rejected.
- `cargo fmt`, `clippy -D warnings`, and the affected-crate tests pass.

## Risk budget

The template split changes expression semantics for comparison-looking value
sources; the aggregator migration removes the one in-tree predicate misuse, and
F8 audits the rest. Raw-literal semantics change `'abc'`/`true`/`42`/`null` in
*expression* positions only; predicate positions are untouched. The
`${}`-gated, top-level-only DSL rule keeps plain and nested literals
byte-identical. `JsEvalResult` gains a public `body_snapshot:
serde_json::Value` field — a breaking public-field addition. No `JsEngine`
trait signature changes. The raw-literal semantics, the pure `trim()` shadow,
the `JsEvalResult` field, AND the top-level DSL `${}` gate are breaking; their
commits MUST carry a breaking
marker (`!` in the Conventional Commit subject or a `BREAKING CHANGE:`
footer), because `cargo xtask lint-commits` rejects the word "breaking" in a
body without a marker. The DSL gate is breaking because a top-level literal
string that merely contains `${}` (for example `set_body: "echo ${HOME}"` or a
JS template payload) now compiles as a Simple template and fails with
`unrecognized token` instead of passing through as a literal.

Bd: rc-veg9m, rc-dec7b, rc-kbdd9, rc-fy7lm (fold), rc-yhvag (fold), rc-rjcur,
rc-v4phf, rc-y0c8c.
