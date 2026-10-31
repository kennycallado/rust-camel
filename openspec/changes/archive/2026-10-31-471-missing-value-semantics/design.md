# Design: 471-missing-value-semantics

## Decision

Keep MiniJinja strict (fail-closed). Record the three-language missing-value contract as
docs + spec delta + pinning tests. No production code changes, no configurable lenient
mode in this change.

Rationale (from the e_opus finding, bd rc-fyxwh): a lenient MiniJinja print silently
drops data — a typoed header name renders as empty output with no error anywhere.
Silent data loss is the worst failure mode for an integration framework; a loud render
failure is diagnosable in one log line. Simple and Rhai already have documented lenient
reads; changing them would break existing routes. The divergence is therefore a
documentation and test-coverage gap, not an implementation defect.

## Observed behavior (the contract to pin)

| Aspect | Simple (`${header.missing}`) | Rhai (`header("missing")`) | MiniJinja (`{{ headers.missing }}`) |
|---|---|---|---|
| Interpolation, whole slot | `Value::Null` | UNIT → lands as `Value::Null` on the camel Value boundary | render FAILS (strict undefined) |
| Interpolation, mixed template | empty string (`prefix--suffix`) | n/a (string building is explicit in script) | render FAILS |
| Conditional use | bare predicate → `TypeMismatch` (strict bool, no truthiness at the boundary); truthiness only inside `&&`/`||` (null/false/empty-string are false) | predicate on UNIT → `TypeMismatch` (strict bool); `if` on UNIT → Rhai runtime error | `{% if %}` on undefined FAILS the render |
| Null vs empty | distinguishable: `${header.missing} == null` / unset-vs-empty ordering rules | distinguishable: `contains` / `"k" in headers` / `has_property`-style reads | distinguishable only via `is defined` test / `default` filter (verify empirically) |
| Error behavior | `LanguageError::TypeMismatch { expected: "bool" }` | `LanguageError::TypeMismatch { expected: "bool" }`; Rhai runtime error for `if` | `LanguageError::EvalFailure` (strict-undefined kind text, redacted detail) |

Supporting code anchors: `camel-language-minijinja/src/engine.rs` (`UndefinedBehavior::Strict`,
`map_render_error` keeps only the error kind — redaction); `camel-language-simple/src/evaluator.rs`
(`is_truthy`, `is_missing`, `Value::Null => String::new()` in template rendering);
`camel-language-simple/src/lib.rs` and `camel-language-rhai/src/lib.rs` (strict-bool predicate
boundary, `TypeMismatch`); `camel-language-rhai/src/lib.rs` (`header()`/`property()` readers
return `Dynamic::UNIT` for absent keys) and `converter.rs` (`Value::Null` ⇄ `UNIT`).

## Approach

1. **Pinning tests only** — one new integration-test file for MiniJinja (the crate has no
   `tests/` dir today), additions to the existing `tests/scalar_expressions.rs` (Simple) and
   `tests/read_only_surface.rs` (Rhai). MiniJinja templates must wrap in exactly one top-level
   `{% autoescape "none" %}` block (existing lexical validator); tests follow the existing
   `missing_path_fails_under_strict_undefined` assertion style (error message names the
   undefined path, redacted).
2. **Empirical verification of the MiniJinja opt-out**: under `UndefinedBehavior::Strict`,
   check whether `{{ headers.missing | default('') }}` and `{% if headers.missing is defined %}`
   render without error in the pinned MiniJinja version. Pin whichever works as the
   documented opt-out; if neither works, the docs say so and the spec scenario is dropped.
3. **One authoritative docs page** `docs/src/languages/value-boundary.md` with the
   three-language table above (rendered as markdown table + per-language paragraphs),
   added to `docs/src/SUMMARY.md` under *Expression languages*. Cross-links from
   `simple.md`, `rhai.md`, `minijinja.md`, and `languages/index.md`.
4. **Spec delta** on `language-value-boundary`: one ADDED requirement
   "Missing-value semantics follow a per-language contract" whose scenarios map 1:1 to the
   pinning tests.

## Affected crates

- `camel-language-simple`: test-only (predicate-boundary pin for a missing value).
- `camel-language-rhai`: test-only (missing-read UNIT, predicate/`if` failure pins).
- `camel-language-minijinja`: test-only (interpolation + conditional failure pins, opt-out
  verification).
- `docs`: new value-boundary page, SUMMARY entry, cross-links.

## Architecture boundaries

Languages boundary only (per CONTEXT-MAP.md). No Runtime, DSL, Components, Services, or
Functions surface changes. The Language SPI (`camel-language-api`) is consumed as-is.
Relevant ADRs: ADR-0047 (MiniJinja template engine, strict-undefined rationale), ADR-0008
(route templates — explicitly out of scope), plus the `language-value-boundary` spec
requirements already governing strict-bool predicates and redacted evaluation errors.

## Alternatives considered

- **Lenient-print mode for MiniJinja** (opt-in config): rejected for this change — new
  configuration surface needs its own security review (silent data loss by default-adjacent
  behavior); the mission order says docs + tests only unless explicitly configured, and
  the non-goals forbid changing strict-by-default.
- **Make Simple strict on missing in templates**: breaking change to existing routes; empty
  interpolation is the long-standing Simple contract (matches Apache Camel's `${header.x}`
  behavior of rendering empty for absent headers) and is already specced ("Mixed missing slot
  renders empty").
- **Make Rhai fail on missing reads**: breaks the `contains`/`in` absence checks the spec
  already pins ("Absent keys and null values are distinguishable").
