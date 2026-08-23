# Proposal: unify-config-interpolation-on-env

## Why

bd rc-w5bf (P2, rescoped 2026-08-22 after e_opus verification ses_fd5e18284ffew36WueAY1tjAmC).
The security half of the placeholder story shipped on main (v0.33 squash `2db55a7b`:
`resolve_security_fail_closed` + `resolve_datasources_fail_closed` + store guard). What remains
is the structural defect class plus two live traps:

1. **Two syntaxes, two resolvers, two failure semantics.** Route files use
   `${env:VAR:-default}` (camel-dsl `env_interpolation.rs`, fail-closed on missing). Camel.toml
   uses `{{env:VAR:default}}` single-colon (`PropertiesResolver`, warn-and-keep-original via
   `resolve_string_in_place`). The split is a documented foot-gun (it produced the maintainer's
   own `{{env:X:-default}}` conflation and bd rc-xb19's dead-doc recipes).
2. **The allowlist class survives.** Two hand-maintained walks carry the self-admitted
   obligation "MUST be extended or placeholders silently survive" (`config.rs:1224` non-security
   enumeration; `resolve_security_fail_closed` security walk). The next contributor adding a
   config section revives rc-xb19's failure mode.
3. **Non-security dash trap** (bd rc-0wvi): `{{env:X:-def}}` on the 12 `resolve_string_in_place`
   leaves silently mints `-def`. Zero credential blast radius, but wrong values nobody wrote.
4. **No escape hatch**: a config value that must carry the literal text `${env:FOO}` is
   unwritable — the DSL regex has no `$$` escape.

Pre-1.0 with zero on-disk `{{` usage in `.toml` files repo-wide: the migration window is free.

## What Changes

1. `${env:NAME}` / `${env:NAME:-default}` becomes THE Camel.toml placeholder syntax, resolved
   by reusing `camel_dsl::env_interpolation::interpolate_env` (already `pub`; camel-config
   already depends on camel-dsl — Q7 resolved: direction correct, no move).
2. Global string-leaf walk (typed fields + untyped `toml::Value` maps), prefix-gated on
   `${env:` — component-owned `${body}`/`${file:...}` expressions untouched. Kills the
   non-security allowlist; the security walk stays as the stricter gate but calls
   `interpolate_env` internally (syntax converges, strictness remains).
3. Hard-error on legacy `{{` in Camel.toml (actionable message pointing to `${env:}`).
   No deprecation window — pre-1.0, zero on-disk usage.
4. `$$` escape on the DSL resolver (`$${env:FOO}` → literal `${env:FOO}`, standalone
   `$$` → `$`). The standalone escape works on ALL leaves; the full escaped form succeeds on
   routes and non-security config leaves, and is rejected on security/datasource leaves via
   the residual-marker gate. Zero `$$` usage repo-wide today (verified).
5. **T5 landmine fix**: the security walk's residual check `contains("${")` (config.rs:1911)
   conflicts with the new syntax — reorder so the DSL pass runs first; the check's meaning
   becomes "reject malformed/unconsumed `${`" only.
6. **Uniform fail-closed** (Q9, maintainer-approved): a `${env:X}` placeholder with X unset and
   no `:-default` aborts startup on ALL config leaves, security or not. Docs show `:-default`
   for optional values.
7. Store guard `ensure_no_placeholder_markers` carries forward unchanged (already live).
8. Docs sweep: collapse the two-syntax explanation into one interpolation chapter
   (camel-config README, schema.md, env-interpolation.md, DSL README cross-links).

## Acceptance Criteria

- `bearer_token = "${env:X}"` resolves from env; X unset → `ConfigError` (uniform fail-closed).
- `${env:X:-default}` honored; `{{env:...}}` anywhere in Camel.toml → actionable hard error.
- A NEW config section's string leaves resolve with zero resolver code changes (the class is
  dead — anti-regression test proves it).
- `${body}`/`${file:...}`/`${1}` values pass through untouched; `$${env:FOO}` yields the
  literal on routes and non-security config leaves (security leaves reject it).
- Route-file `${env:}` interpolation semantics unchanged (regression test).

## Affected Crates

`crates/camel-config` (walk, parser reuse, legacy rejection), `crates/camel-dsl`
(`$$` escape in env_interpolation), `docs/`.

## Risk Budget

Q9 flips sloppy-but-working configs (optional leaves with unset vars) from warn to abort —
intentional, approved. Pre-1.0 with zero on-disk legacy usage keeps migration cost near-nil.
Roadmap predecessor rc-xb19 closed; dash-trap rc-0wvi superseded by this change's T2.
