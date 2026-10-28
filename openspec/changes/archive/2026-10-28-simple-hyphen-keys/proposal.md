# Proposal: simple-hyphen-keys

## Why

bd rc-e2p0n (mission 434): the same hyphenated lookup key must not behave differently depending on
context. Mission 428 (commit 45a46d359, landed 2026-10-28) already made unspaced hyphenated keys
work in atom AND compound positions (`${header.some-key + 1}` parses as `Header("some-key") + 1`,
asserted by `slot_hyphen_key_vs_subtraction`). What remains is the **spaced** form: in
`header.some - key` the hyphen is the subtraction operator (per the documented resolution rule:
hyphen followed by digit/whitespace is subtraction), so the right side `key` is a bare
non-lookup identifier and the parse fails with the generic `unknown identifier` — accurate but
unhinting. A user who meant the key `some-key` gets no pointer to the unspaced form.

## Decision

Option **B** (documented restriction + targeted diagnostic) from the mission order. Option A
(bracket/quoted lookup `header["some-key"]`) is rejected because:

- Its headline benefit (hyphenated keys in compound position) already exists since 428.
- It adds grammar surface: new `[`/`]` tokens, quoted-key syntax, `is_grammar_token` expansion,
  primary-atom changes, a `language-value-boundary` spec delta, and an ADR-0092 revision — grammar
  growth ADR-0092 defers ("the grammar surface stays small") and the mission non-goals exclude
  ("new operators") for a P3 that camel-112 (camelCase keys) never hits.

B touches only the parse-error path of already-failing sources: no successful parse, evaluation,
limit, or 428-semantics change. No spec change is required.

## What Changes

- `camel-language-simple` parser: a targeted, payload-free parse error when a compound expression
  has the shape `<scoped-lookup> - <bare-word>` (e.g. `header.some - key`), explaining the hyphen
  resolution rule and the unspaced fix. Error-path only; the generic `unknown identifier` remains
  everywhere else.
- `docs/src/languages/simple.md`: document the hyphen key resolution rule and restriction.
- `docs/adr/0092` limitation note: bracket/quoted key lookup is not supported; record the rule.
- Excluded: bracket/quoted syntax, new operators, `now()`, mutation, deferred constructs
  (float/div/mod/ternary/in/??/pow), camel-112-facing semantics, workspace bump.

## Acceptance criteria

- `parse("header.some - key")`, `parse("header.some -key")`, and template/predicate-slot forms
  fail with the targeted message naming the unspaced form; generic `unknown identifier` is
  preserved for non-lookup left sides (e.g. `key - 1`).
- `header.n - 1`, `header.some - header.other` (subtraction) and unspaced
  `header.some-key`/`${header.some-key + 1}`/`header.some-key == 'x'` parse and evaluate unchanged.
- Missing hyphenated key evaluates to `Value::Null`.
- Gates: `cargo test -p camel-language-simple -p camel-core --lib`, `cargo fmt --check --all`,
  `cargo clippy -p camel-language-simple --all-targets -- -D warnings`, xtask lints
  (lint-unwrap, lint-test-sleep, lint-unbounded-wait, lint-glob-imports, lint-unsafe-safety,
  lint-missing-docs-ratchet), `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-language-simple
  --no-deps`, `cd docs && mdbook test`, `openspec validate --all --strict`.

## Risk budget

Low. Error-message-only parser change; docs-only otherwise. Out of bounds: any grammar, limit
(16 KiB/512/64/4096/1 MiB), strict-boolean, or evaluator change.

bd: rc-e2p0n
