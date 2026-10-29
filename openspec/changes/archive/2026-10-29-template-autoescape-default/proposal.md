# Proposal: template-autoescape-default

bd: rc-6mwze

## Why

ADR-0047 §"Escape-mode contract" made a top-level `{% autoescape %}` block
mandatory in every template, for two reasons: one global HTML default would
corrupt JSON, URL, prompt, and plain-text output, and inline
`language: minijinja` sources have no trustworthy filename for extension
inference. The external `template:` component inherited that rule verbatim.

The inheritance is too strict for the external component: its files DO have an
extension and an operator-owned `[components.template]` config block. Eleventy
and Nunjucks have no `{% autoescape %}` tag, so an operator sharing templates
across toolchains must maintain custom tag shims. Owner decision (2026-10-29,
rc-6mwze): allow an opt-in configured default for the external component only.
Inline minijinja stays mandatory-explicit. The default stays "must declare"
when the key is absent — this is a security posture, never an implicit HTML
fallback.

## What Changes

- New `[components.template]` key `default-autoescape = "html" | "json" | "none"`
  (kebab-case, like the other keys in the block; exact lowercase values).
- Absent key = today's behavior: an entry template without a top-level
  `{% autoescape %}` block still fails at compile.
- Set key = an entry that contains no `{% autoescape %}` statement at all is
  compiled wrapped in that mode. An entry that contains an autoescape
  statement must still pass top-level validation: the config never repairs a
  malformed, misnamed-mode, partial, or duplicate explicit statement, and an
  explicit valid block always wins over the config.
- The configured default applies to the entry only; include targets keep
  their own semantics.
- Invalid values (unknown string, wrong type) fail config load with an error
  naming the key and the allowed values.
- Inline `language: minijinja` is unchanged; the language crate is not
  modified.
- ADR-0047 §"Escape-mode contract" gains a dated amendment: a component-level
  opt-in default counts as an explicit operator decision, valid for the
  external component only; the absent key still rejects.

Excluded (non-goals): filters/functions/globals allowlist, inline minijinja
semantics, escaping-rule changes, route placeholder engine (rc-sanz1), hot
reload behavior.

## Acceptance criteria

- `cargo test -p camel-template` green, including: absent config +
  no block rejects (existing); config "html" escapes a `<` payload; config
  "none" does not; explicit `{% autoescape "json" %}` overrides config
  "html"; invalid config value rejects; an invalid explicit block still
  rejects with config set; inline minijinja still rejects a missing block.
  No sleeps.
- `cargo fmt --check --all`; `cargo clippy -p camel-template
  --all-targets -- -D warnings`; `cargo xtask lint-unwrap`,
  `lint-test-sleep`, `lint-unbounded-wait`, `lint-unsafe-safety`,
  `lint-missing-docs-ratchet`, `lint-no-deprecated`;
  `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-template
  --no-deps`; `cd docs && mdbook test`; `openspec validate --all --strict`.
- Docs: template component page, crate README, and the ADR-0047 amendment
  document the key, precedence (explicit block > config > reject), the
  security note, and that inline minijinja is unchanged.
- Spec delta on `template-component` "Template render semantics" archived.

## Risk budget

- Acceptable: a compile-path parameter threaded through the component
  internals (`template_set`, `endpoint`, `lifecycle`, `reload`) via an
  additive builder method. The public `TemplateComponent::new` signature is
  unchanged (it is consumed by `examples/template-basic`), so no consumer
  breaks.
- Out of bounds: any change to `camel-language-minijinja`, to escaping
  rules, or to reload/fail-closed semantics; a default that applies when the
  key is absent; silently repairing a broken explicit block via the config.
