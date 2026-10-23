# Proposal: config-bool-env

## Why

`${env:}` placeholders coerce into string leaves everywhere and into integer
fields through the provenance-gated typed probe (`env-int-placeholder-typing`),
but booleans have no path at all: `enabled = "${env:FLAG:-true}"` resolves to
the string `"true"`, typed deserialization rejects it, and the probe only
considers clean i64 values. `CAMEL_*` overrides carry strings too, so the only
workaround is per-environment include files. A partner hit this while making a
config deployable. bd: rc-qyhlk.

## What Changes

- camel-config: extend the typed probe in `crates/camel-config/src/env_int_probe.rs`
  so a provenance leaf resolving to exactly `true` or `false` coerces to a TOML
  boolean. Same provenance gating, same bounded cap (8), same smallest-first
  subset search, same first-pass oracle, same original error when no subset
  deserializes.
- camel-dsl: mirror the extension in `crates/camel-dsl/src/env_int_probe.rs`
  (YAML arm) for provenance leaves under `routes:`, same shared cap and
  QUIET-parser oracle; the logging-replay contract is unchanged.
- Specs: add the boolean requirement to `config-env-placeholders` and the DSL
  mirror (`dsl` spec) with the scenarios below.
- Docs: `docs/src/configuration/env-interpolation.md` documents what coerces
  (exactly `true`/`false`) and what does not.

Excluded (non-goals): float coercion, numeric-boolean aliases (`1`/`0`,
`yes`/`no`), case folding, whitespace trimming, interpolation-engine changes,
any file outside camel-config / camel-dsl / specs / the docs page, version bump.

## Acceptance criteria

- Both arms load a boolean field from a default and from a set variable:
  `watch = "${env:CFG_WATCH:-true}"` and `auto_startup: ${env:RC_AUTO:-false}`
  deserialize as real booleans.
- Provenance gating holds in the new path: literal quoted `"true"` at a boolean
  field stays rejected; a string field resolving to `"true"` keeps the string.
- Out-of-set values (`yes`, `TRUE`, `1`, `true ` with trailing space) stay
  rejected with the first-pass error naming the field.
- Boolean candidates count against the same probe cap as integers (mixed
  beyond-cap document keeps the first-pass error).
- Tests fail on main and pass after, in both arms; full gate registry green.

## Risk budget

Low: the probe still runs only after a failed strict deserialize, so every
document that loads today is unaffected. The strictness boundary (exactly
`true`/`false`) is a recorded decision; any future loosening is a spec change.
Known out-of-scope consequence: camel-lint's rschema integer carve-out does not
know about boolean coercion and will flag `${env:X:-true}` at a boolean route
position — filed as a follow-up bd, camel-lint untouched per non-goals.
