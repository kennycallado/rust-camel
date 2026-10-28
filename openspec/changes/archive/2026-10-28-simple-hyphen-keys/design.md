# Design: simple-hyphen-keys

## Approach

Option B (see proposal): document the hyphen resolution rule and retarget one misleading parse
error. Two touch points, both inside `camel-language-simple`.

### 1. Targeted diagnostic (parser.rs)

Both parse entry points funnel a whitespace-stripped token slice into `parse_or`:
`parse()` (predicate mode, line ~174) and `parse_slot_expression` (template slots, line ~430).
A bare non-lookup `Token::Word` atom makes `parse_identifier` fail with reason
`unknown identifier`; that error propagates unchanged today.

Add one helper, called at both `parse_or(...)?` sites, wrapping the propagated error:

```rust
fn retarget_spaced_hyphen_error(err: LanguageError, tokens: &[Token], input: &str) -> LanguageError
```

- Fires only when `err` is `LanguageError::ParseError` with reason `unknown identifier` AND the
  token slice is EXACTLY the triple `Word(l), Minus, Word(r)` (callers strip whitespace-only
  `Text` before `parse_or`, so the slice length is 3) where `l` is a PREFIX-QUALIFIED lookup
  word (starts with `body.` / `header.` / `property.` / `exchangeProperty.`) and `r` is a bare
  non-lookup word (`is_lookup_word(r)` false). Keyless lookups do NOT fire: for `l == body` or
  `l == exception.message` there is no dotted prefix to extend, so the suggested unspaced form
  would be an invalid identifier — those keep the generic error.
- Exact-slice matching is load-bearing: a prefix/contains match would over-fire on call shapes —
  `header.n - max(1, x)` starts with the same triple but fails on the bare `x` inside the call
  args, nothing to do with hyphens. The exact triple can never parse successfully (`r` is a bare
  non-lookup word not followed by `(`), so the helper only rewrites one error into a better one.
  No successful parse, AST, evaluator, or limit change.
- Extract `fn is_lookup_word(word: &str) -> bool` from `parse_identifier`'s existing `is_lookup`
  condition (`body`, `exception.message`, `body.`, `header.`, `property.`, `exchangeProperty.`
  prefixes) and reuse it in both places (single source of truth).
- Message names both facts and the fix, quoting only operator source (trusted configuration,
  same as the existing `expr` field — payload-free): e.g.
  `spaced hyphen: \`header.some - key\` parses as subtraction, and \`key\` is not a lookup; write
  a hyphenated key without spaces: \`header.some-key\``.
- Safety: a bare non-lookup word is ALWAYS a parse error in Simple, so the triple
  `Word(lookup), Minus, Word(non-lookup)` can never appear in a successful parse — the helper
  only rewrites one error into a better one. No successful parse, AST, evaluator, or limit change.
- Non-fires (must stay generic/unchanged): `key - 1` (left not a lookup), `body - key` and
  `exception.message - key` (keyless left side — no prefix to extend), `header.n - 1`
  (right is `IntLit`), `header.some - header.other` (right is a lookup, parses fine),
  `header.n - max(1, x)` (slice longer than 3; real error is the bare `x`), `check-in` alone
  (no `Minus`), `header.some - key == 'x'` (comparison tail → generic error; acceptable),
  deferred-construct errors (`in is deferred`, …).

### 2. Docs

- `docs/src/languages/simple.md`, lookups paragraph (~line 91): document the rule — a hyphen
  followed by a letter/underscore continues a lookup key only when written without spaces
  (`header.some-key`, works standalone and in compounds); a hyphen before a digit or whitespace
  is subtraction (`header.n-1`, `header.some - key`); the spaced form is a parse error whose
  message names the unspaced fix. Bracket/quoted key lookup does not exist in Simple.
- ADR-0092 (`docs/adr/0092-simple-scalar-expressions.md`, `## Deferred` at ~line 197):
  append the limitation note — bracket/quoted key lookup and keys written with a spaced hyphen
  are not supported; the resolution rule above is the normative behavior.

### Spec outcome

No grammar or semantics change → no `language-value-boundary` delta. `skip_specs: true` in the
change `.openspec.yaml`. ADR-0092 note + docs are the recorded-decision surface.

## Affected crates

- `camel-language-simple`: parser error-path helper + `is_lookup_word` extraction; tests.
- `docs`: `src/languages/simple.md`, ADR limitation note.

## Architecture boundaries

Languages-boundary only. No Runtime/DSL/Component/Service changes; no Exchange data in
diagnostics (positions and operator source only, matching existing `ParseError` payloads).
Simple's compile-once/evaluate-many model is untouched.

## Alternatives considered

- **(A) Bracket/quoted lookup `header["some-key"]`** — rejected: new token surface + spec delta
  + ADR revision for a benefit 428 already delivered (unspaced keys work in compounds);
  near the mission's "new operators" non-goal; risks the 428 fast-path interplay
  (`hyphenated_lookup` condition) for a P3.
- **Do nothing** — rejected by the mission: the generic `unknown identifier` is precisely the
  misleading failure B must remove, and the rule is undocumented in `simple.md` today.

## Tests (executable, per task)

Task 1 (parser + eval, `camel-language-simple`):
- `spaced_hyphen_subtraction_targets_key_error`: `parse("header.some - key")` errs with message
  containing `spaced hyphen` and `header.some-key`; same for `parse("header.some -key")`,
  `parse_template("${header.some - key}")`, and `parse_template("a ${header.some - key} b")`.
- `generic_unknown_identifier_preserved`: `parse("key - 1")` errs containing
  `unknown identifier`; `parse("check-in")` still errs `unknown identifier`;
  `parse("body - key")` and `parse("exception.message - key")` stay generic
  `unknown identifier` (keyless left side, helper must not fire); `parse("header.n - max(1, x)")`
  stays generic `unknown identifier` (slice longer than the exact triple).
- `subtraction_and_lookup_unchanged`: `parse("header.n - 1")` → `BinOp{Header("n"), Sub, IntLit(1)}`;
  `parse("header.some - header.other")` → `BinOp{Header("some"), Sub, Header("other")}`;
  `parse("header.some-key == 'x'")` → comparison over `Header("some-key")` (compound comparison);
  `${header.some-key + 1}` stays `Header("some-key") + 1` (existing test, must stay green).
- `missing_hyphenated_key_is_null` (lib.rs, beside `test_property_and_header_keys_preserve_hyphens`):
  expression over an Exchange lacking `some-key` yields `Value::Null` / empty interpolation;
  compound `${header.some-key + 1}` with the header present evaluates `"3"` for `"2"`.
- `hyphenated_key_comparison_evaluates` (lib.rs): `create_predicate("header.some-key == 'x'")`
  matches `true` with header `some-key = "x"`, `false` with `"y"` (evaluation counterpart of the
  AST-level comparison assertion).

Task 2 (docs):
- `docs/src/languages/simple.md` states the resolution rule + restriction (prose review; mdbook test).
- ADR §Deferred gains the bracket/quoted-lookup limitation + rule note.
