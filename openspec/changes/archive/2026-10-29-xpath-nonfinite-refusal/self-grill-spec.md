# Self-grill record — xpath-nonfinite-refusal (spec stage)

**Mode:** self-grill-proposals skill (mission order: e_gpt only for a genuine
ruling; no genuine dispute — audit sealed Q4 covers the core decision).

## DP-1: Non-finite refusal via EvalFailure { class: Arithmetic }

**Questions generated:**
1. [glossary] Does the `arithmetic` class conflict with the existing error taxonomy?
2. [sharpen] Does "naming the non-finite class" conflate value-class labels with type names?
3. [scenario] What input would the refusal get wrong?
4. [cross-ref] Do sibling languages already implement this refusal pattern?

**Answers (with citations):**
1. No. `ExpressionErrorClass::Arithmetic` documents "overflow, division by zero,
   NaN rejection" (`crates/camel-api/src/error.rs:90-91`), stable label
   `arithmetic` (`error.rs:112`). It is the canonical class for this case.
2. The detail names the value class with stable labels `+Inf`/`-Inf`/`NaN` —
   bounded enum-class names, not document content. `TypeMismatch` is the wrong
   axis: "`expected`/`actual` are TYPE names, never runtime values"
   (`crates/languages/camel-language-api/src/error.rs:39-45`). The type here is
   correct (number); the value is unrepresentable — a value refusal.
3. `string(1 div 0)` yields an XPath String ("Inf") → converts as a string,
   correctly not refused. Only `SxdValue::Number` reaches the refusal arm
   (`camel-language-xpath/src/lib.rs:180`). Predicates route through `run_query`
   (`lib.rs:197-199`), so both verbs are covered; sealed Q3 strict-bool
   (`lib.rs:200-209`) is unchanged — a predicate failed before (Null →
   TypeMismatch "null") and fails after (Arithmetic refusal); only the
   diagnosis improves.
4. Rhai spec scenario "Non-finite float is refused" (language-value-boundary);
   JS `camel-language-js/src/value.rs:185` names the class in its message.
   House style: refuse, name the class, no debug/stringify fallback.

**Outcome:** confirm
**Self-grill mode:** self-grill-proposals skill

## DP-2: Integral fidelity via half-open exactness intervals

**Questions generated:**
1. [glossary] Is "JSON integer form" consistent with audit terminology?
2. [sharpen] Are the exactness intervals precise enough to forbid a lossy cast?
3. [scenario] What boundary value breaks the projection?
4. [cross-ref] Does any existing test depend on the float form of integers?

**Answers (with citations):**
1. Yes — audit V2: "integers degrade to floats (`count(...)` → `2.0`, M type
   change)" (`.opencode/fleet/language-boundary-audit-matrix-20261002.md:98`).
2. Intervals: `[-2^63, 2^63)` → i64, `[2^63, 2^64)` → u64, else `from_f64`.
   Every integral f64 strictly below 2^63 is ≤ 2^63 − 1024; every integral f64
   in [2^63, 2^64) is ≤ 2^64 − 2048 — the `as` casts are exact, never lossy.
   Half-open upper bounds avoid the `2^63 as i64` saturation trap.
3. `n = 2^63` → u64 branch, exact; `n = 2^64` → float branch; `-0.0` → `0`
   integer form (JSON has no −0 integer; documented in the code comment);
   `-2^63` exactly → i64, exact.
4. The `count` test asserts against `serde_json::Number::from_f64(2.0)`
   (`camel-language-xpath/src/lib.rs:403-405`). The tasks tighten this
   assertion to pin integer form (`as_i64`/`as_u64`) explicitly, removing any
   reliance on serde_json cross-type numeric equality.

**Outcome:** confirm (refinements folded into task specs)
**Self-grill mode:** self-grill-proposals skill

## Verdict

BLESS — spec delta is structurally valid (`openspec validate` clean),
requirements are consistent with the sealed boundary contract, no genuine
ruling required.

## Amendment — r_gpt plan-review fixes (2026-10-29)

r_gpt REJECT on the first plan review; all five findings verified legitimate
and applied:
1. u64 branch lacked `n >= 2^63` — negative integrals (e.g. `-2^64`) would
   saturate to `0` via `as u64`. Guard added (design + tasks + regression
   test `negative_integral_two_pow64_stays_float_no_zero_saturation`).
2. Spec scenario "magnitude 2^63 or larger → float" contradicted the
   exact-u64 rule (2^63 IS exactly representable in u64). Scenario corrected
   to magnitude ≥ 2^64 (outside both exact intervals).
3. serde_json 1.0.151 `Number` equality is variant-strict (`_ => false`,
   verified in the locked source `number.rs` `impl PartialEq for N`) — the
   count test expectation must be REPLACED with integer form, not kept.
   Design claim corrected; task step 3 rewritten.
4. Property roundtrip lacked the re-parse leg; loop closed
   (doc → nodeset → string-value → doc → nodeset → string-value).
5. `JsonValue::Number::from(...)` constructor typo fixed to
   `serde_json::Number::from(...)`.
Reviewer note also applied: determinism property compares via `to_bits()`
(NaN != NaN). Exactness constants (2^63−1024, 2^64−2048) independently
confirmed by the reviewer.

## Amendment 2 — second review round (2026-10-29)

1. The first fix for the roundtrip leg was repeatability, not a roundtrip
   (second evaluation reused the ORIGINAL body). Corrected: the first
   returned string-value becomes the input of a second document
   (`<roundtrip>sv</roundtrip>`, alphanumeric alphabet keeps it XML-safe,
   guarded by an explicit no-`<&>` assert); `/roundtrip` string-value must
   equal the first.
2. design.md test-summary line corrected: `2^63` is the exact-u64 case,
   `2^64` (both signs) is the float/no-saturation case.

## Amendment 3 — holistic review (2026-10-29)

Holistic review found a normative conflict with the canonical spec: the
ADDED "XPath number projection refuses non-finite values" requirement
mandates class `arithmetic`, while canonical "Unrepresentable values are
refused" generically mandates `conversion` for unrepresentable values.
Archiving both unamended would leave two incompatible requirements.
Reconciled by adding a `## MODIFIED Requirements` block for the canonical
requirement that carves XPath number projection as an explicit exception:
non-finite XPath numbers (`+Inf`, `-Inf`, `NaN`) use `arithmetic` with a
payload-free detail; every other refusal keeps `conversion`.

## Amendment 4 — final (2026-10-29)

Second holistic pass: APPROVE-WITH-FINDINGS. Findings 1-3 (spec conflict,
conformance coverage, file size) confirmed resolved. Finding 4 (proposal
risk-budget equality claim) fixed by conductor: proposal now states
integer-form projection preserves the numeric value while intentionally
changing JSON representation form for variant-sensitive equality.
