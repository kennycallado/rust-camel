# Design: rhai-big-int-json-number

## Approach

Design E from the pre-flight ruling (e_opus, 2026-11-04, GO-WITH-CONSTRAINTS,
binding): bind big inbound integers as the existing `json number` wrapper and
convert only the exact inbound image back outbound.

1. **Inbound** — `json_to_dynamic` (`converter.rs`): the `as_u64()` arm, which
   currently refuses with `"u64 > i64::MAX"`, binds
   `JsonNumber::new(n.to_string())`. serde_json renders a `u64`-backed
   `Number` as the exact plain decimal, so the token is the exact source
   image. Recursion already covers nesting; every caller (body, headers,
   properties) goes through this single arm.
2. **Outbound** — `dynamic_to_value` (`converter.rs`): the `JsonNumber` arm
   asks a new helper `as_u64_above_i64(token)`. `Some(u)` yields
   `Value::from(u)` (a `u64`-backed `Value::Number`); `None` keeps the refusal
   with the stable label `json number`.
3. **Helper** — `json/mod.rs`: `pub(crate) fn as_u64_above_i64(token: &str)
   -> Option<u64>`. Accepts only canonical unsigned decimal tokens — all
   ASCII digits with NO leading zero — that parse as `u64` and exceed
   `i64::MAX`; a token such as `"09223372036854775808"` refuses. Doc-commented
   (missing-docs ratchet). No `unwrap` (lint-unwrap); `JsonNumber` imported
   explicitly (lint-glob-imports). The refusal-case acceptance test carries a
   leading-zero token in addition to the mission's three
   (`"1.5"`, `"5"`, `"18446744073709551616"`).
4. **Transaction diff** — `transaction.rs`: new `Kind::JsonNumber` variant in
   `kind()`; `rhai_values_differ` compares two same-kind `JsonNumber`s by
   token (mirrors `blobs_equal`). Untouched entries produce no delta and keep
   their original value handle. Script-mutated containers recurse and the
   nested wrapper converts through the outbound arm bit-identically.
5. **Comparisons** — no change: `guard_pair::<JsonNumber, i64/f64/String/bool>`
   registrations (`json/host.rs:334-341`) already refuse with `type-mismatch`;
   the acceptance tests pin this.

## Affected crates

- `camel-language-rhai`: `converter.rs` (both arms + Q4 doc wording),
  `transaction.rs` (new kind + token equality), `json/mod.rs` (helper),
  `lib.rs` (snapshot doc text + tests), `CONTEXT.md`.
- Repo root: new `CHANGELOG.md` (mission requirement; the repo previously had
  none — release notes come from `cargo xtask changelog` over conventional
  commits, and this change keeps that gate intact).
- `openspec/specs/language-value-boundary` via this change's delta specs only.

## Architecture boundaries

Languages boundary only. `camel-language-api` types (`Value`, `Body`,
`LanguageError`) are consumed, not changed. No Runtime/DSL/Components/
Services surface moves. Other languages keep refusing big integers inbound:
the delta specs scope every edit to rhai.

## Alternatives considered

- **Float binding (design B)** — rejected: lossy, violates sealed Q4.
- **String binding (design C)** — rejected: destroys the numeric type; script
  arithmetic and downstream JSON re-serialization break silently.
- **Widen `Value`/serde to keep u64 native in rhai** — rejected: rhai integers
  are `i64`; a native u64 does not exist. Touching every language for a rhai
  fix is out of scope.
- **Lazy (on-read) conversion instead of eager snapshot** — rejected: the
  pre-run snapshot backs the `header()`/`property()` readers (task 3.3
  contract); lazy binding would fork the read model. Eager binding is the
  ruling's design E.

## Test-design notes (for the blessing experts)

- `conversion_error_precedence_body_before_header` is rebuilt per the mission:
  after this change no serde_json `Value` is refused inbound, so the test
  pins the NEW behavior — body `Body::Json(u64::MAX)` plus an identical
  header both bind, `evaluate` returns `Ok`, header stays bit-identical. The
  body-before-header snapshot precedence remains defined for the
  forward-compat unknown-`Body`-variant arm (unconstructable today), so the
  old fixture is unbuildable.
- `conversion_error_precedence_header_before_property` (not in the mission
  list, same fixture evaporation) is rebuilt on the live write-back
  precedence: a mutating script assigns `timestamp()` (refused outbound) to
  both a header and a property; `eval_mut_sync` validates header deltas
  before property deltas, so the error target is `header entry`.
- `group_conversion_failure_is_indexed_to_clause_zero` (not in the mission
  list) is inverted into the group-path coverage for the ruling's normative
  behavior 2: a big-u64 header no longer refuses pre-loop; the group
  succeeds. No pre-loop refusal fixture remains constructible.
