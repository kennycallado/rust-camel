# Proposal: env-driven-knobs — unit-tier document env layer

## Why

`camel test` (unit tier, LEAN) resolves `${env:NAME:-default}` default-only across all
three of its interpolation seams (route files, inline routes, doc-side identifier
fields — rc-4hexo parity, b47b6b36). Ambient env is never consulted (deliberate
hermeticity, rc-93wct Q5). Test authors therefore have NO supported way to STEER
interpolation per-document: the documented R-REPOSITORY-STUB pattern
(`${env:CACHE_REPO_NAME:-persistent}` steering to an in-memory stub) has no injection
mechanism left in the unit tier, and per-doc fixture variation (CI vs local timing
strings, endpoint names, steering knobs) is impossible without editing route files.

bd rc-l7m7t tracks the supported mechanism. The papal consult (e_opus,
ses_f7d7b982effeGHRiWlWUMoBRte) locked option (c) in its pure form: a document
`env:` layer applied through the existing injectable lookups — zero camel-dsl
changes, boot parity untouched.

## What Changes

- Unit-tier `*.test.yaml` documents MAY declare an optional `env:` map of string
  fixture values.
- One doc-sourced closure (`&|name| doc.env.get(name).cloned()`) is threaded through
  the three existing seams: `load_from_file_with_env` (routeFiles /
  routeFilesFromRoot), `interpolate_yaml_source` (inline routes), and the
  doc-identifier interpolation pass (`interpolate_identifier`).
- The layer is strictly document-sourced: no ambient reads, no passthrough allowlist,
  no harness provisioning (unit tier stays maximally hermetic — ADR-0069 §4 ambient
  inheritance stays off).
- Typing semantics DO NOT change: substituted leaves keep string typing; int-typed
  fields carrying a placeholder still fail (boot parity). Numeric knobs
  (`circuit_breaker.open_duration_ms`) are NOT served by this change — that class is
  re-scoped to the rc-v1sw coercion track (the bd's "(c) covers BOTH" claim is
  corrected as part of this change's bd follow-up).

## Impact

- **Spec deltas:** openspec/specs/mock-testkit — one ADDED requirement (document env
  layer), two MODIFIED (route-source lookup, doc-identifier lookup; renamed to drop
  "default-only" from their names).
- **Code:** crates/camel-cli/src/commands/test/{document.rs,runner.rs} only. No
  camel-dsl, no scenario tier, no camel-integration-test (leased files untouched).
- **Docs:** camel-cli CONTEXT.md hermeticity paragraph; unit-testkit user docs.
- **bd:** rc-l7m7t note correcting the covers-both claim + refreshing the
  (a)↔rc-v1sw link.

## Out of scope

- Numeric/boolean knob typing (rc-v1sw track; option (a) long-term).
- Scenario-tier env behavior (LayeredEnv already covers it; leased zone).
- Passthrough allowlist in the unit tier (papal: doc-strict).
- 0.42.0 release-notes migration note (tracked separately on rc-l7m7t).
