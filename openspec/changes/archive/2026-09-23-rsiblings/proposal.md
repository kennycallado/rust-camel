# Proposal: rsiblings

## Why

bd rc-lys6a (P2, discovered-from rc-n3t73 / mission 218): when a pattern
violation co-occurs with a non-pattern defect in the same failed
`anyOf` (e.g. a blank MCP TLS `cert_path` plus an unknown `tls` key),
the pattern de-collapse arm in `camel-lint/src/rules/rschema.rs`
reports ONLY the pattern leaves — the collapsed diagnostic that carried
the sibling `additionalProperties`/`type` defect is dropped. Lint then
under-reports real boot failures (both defects are rejected at boot:
`non_empty_path` and `deny_unknown_fields`), which is the exact
lint-approves-what-boot-rejects bug class rc-n3t73 fixed for the
single-defect case. Today the gap is documented as a KNOWN LIMITATION
comment (first-error-wins, mirrors serde stop-at-first).

## What Changes

Choose ONE bd fix option and defend it in design.md:

- (a) append the non-pattern nested errors as leaf diagnostics too, or
- (b) keep the collapsed parent alongside the pattern leaves.

Scope: the pattern de-collapse pass (pass 2) of the `AnyOf` arm in
`rschema.rs` only. The permission targeted pass (pass 1) and every
non-AnyOf arm are untouched. Tests in
`crates/camel-lint/src/rules/rschema/tests.rs`; corpus end-to-end pin
via a new negative fixture + baseline entry
(`crates/camel-cli/tests/fixtures/lint-corpus/`).

Affected crates: camel-lint (rule + tests), camel-cli (corpus fixture
only). No schema regeneration, no DSL change, no canonical_json or
`diagnostic_message` change.

Excluded: permission-pass sibling subsumption (stays documented
first-error-wins); rc-sghtz (bind/name drift); rc-a4xap (both-blank
two-leaf pin); rc-dtljr (CONTEXT.md note).

## Acceptance criteria

- Co-occurrence (pattern + additionalProperties sibling): blank
  `cert_path` + unknown `tls` key yields BOTH diagnostics — the
  pattern leaf AND the sibling leaf, each anchored on its own authored
  span.
- Co-occurrence (pattern + type sibling): blank `cert_path` +
  non-string `key_path` yields BOTH diagnostics.
- Single-defect regression: pure-pattern cases (existing blank-path
  tests) and pure-non-pattern cases (unknown key alone → collapsed
  anyOf diagnostic) stay byte-identical.
- Corpus byte-exact pins stay green:
  `rschema_exception_disposition_oneof_unchanged`,
  `rschema_rest_binding_oneof_unchanged`, existing corpus baseline.
- Gates: fmt; clippy `-p camel-lint --all-targets -D warnings`;
  camel-lint full suite + corpus; 16 xtask lints;
  `-p camel-lint --lib` AND `--workspace --lib` (byte-exact class
  depends on build graph — both must pass).

## Risk budget

The risk is regressing the d0d88267 (cifix) byte-exactness
guarantee: `canonical_json`/`diagnostic_message` rebuild anyOf/oneOf
messages deterministically across build graphs. Sibling diagnostics
must route through `diagnostic_message(nested)` (non-AnyOf kinds keep
jsonschema `Display` verbatim — scalar/single-key echoes cannot
reorder). Pure-pattern and pure-non-pattern paths must be provably
untouched. Any message drift on pinned classes is out of bounds.
