# Design: jmscredkey

## Approach

Spec-led alignment change. The production behavior already landed in
mission redactconv (d69cfa71): the denylist branch of
`camel_api::redact::redact_url_with_query_allowlist` checks
`is_credential_shaped(&minimal_decode_pair(raw_key))` and renders a
bare `<redacted>` when the key itself carries a credential shape
(`redact.rs`, key-position symmetry, bd rc-yvjp3). This change only
brings the spec canon in line, then pins the behavior at the
component boundary.

The delta restates the whole MODIFIED requirement (OpenSpec replaces
the requirement block at archive time), so the restatement carries
every existing sentence and scenario forward unchanged. Two edits
land inside it:

1. The key-rendering parenthetical gains a pointer to the exception:
   "(the rendered key keeps its original encoded bytes, except under
   the key-position credential-shape exception below)".
2. One new paragraph after the kept-pair paragraph, worded from
   ADR-0076 appendix "Key-position credential-shape symmetry":
   a denylist-matched pair whose raw key's single-pass minimal
   decode contains `@` AND (`:` OR `//`) renders as a bare
   `<redacted>`. Otherwise the pair keeps `{raw_key}=<redacted>`.

Two scenarios join the six existing ones. "Credential-shaped key
never echoes" covers `%3A`/`%40` in both hex cases and the literal
`user:pass@host` form. "Non-shaped sensitive keys keep their key
names" covers `password`, `jms.userName`, a lone-`@` key, and the
`pass%77ord` encoded-name rule, all with exact outputs taken from
the landed camel-api pins.

## Affected crates

- camel-component-jms: test-only. Two pins in
  `src/config.rs::tests` calls `redact_broker_url` with the
  credential-shaped-key fixtures and asserts the exact outputs,
  matching the existing exact-output pins in that module.
- camel-api: no change. Its pins
  (`allowlist_suppresses_credential_shaped_key_uppercase`,
  `_lowercase`, `allowlist_suppresses_literal_credential_shaped_key`,
  `allowlist_keeps_well_known_key_names_visible`,
  `allowlist_keeps_lone_at_key_visible`) already cover the canonical
  helper.

## Architecture boundaries

Components layer only. The spec delta governs the jms capability
surface. `redact_broker_url` stays a thin delegate over
`camel_api::redact` (data plane unchanged, no new dependency). The
DSL, runtime, and services layers are untouched. Respects ADR-0051
(over-masking safe), ADR-0076 (strictest wins, key-position
symmetry), and the e_opus ban on direct spec-canon edits: the canon
changes only through `openspec archive`.
