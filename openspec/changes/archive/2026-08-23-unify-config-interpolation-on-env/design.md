# Design: unify-config-interpolation-on-env

## Approach

Single-phase unification (e_opus verdict ses_fd5e18284ffew36WueAY1tjAmC — the two-landing
strategy is obsolete; Landing 1 shipped on main via `2db55a7b`). One syntax, one resolver,
one fail-closed contract, zero hand-maintained allowlists on the non-security side.

### Resolver unification (T1)

`camel_dsl::env_interpolation::interpolate_env` (already `pub`, `env_interpolation.rs:36`)
becomes the single placeholder engine. camel-config already depends on camel-dsl
(`Cargo.toml`, non-dev). Layering direction config→dsl is correct (ADR-0055: no cycle — dsl
must not dep config; verified one-way). No code moves to camel-api (Q7 resolved: don't).

### Global walk (T2)

Traversal is executable because it runs on the merged RAW `toml::Value` BEFORE
deserialization (today the flow deserializes first and then hand-walks typed fields,
`config.rs:1780-1781`; `CamelConfig` does not derive `Serialize`, so a post-deserialize walk
would need another hand-maintained visitor — rejected). New flow:

1. Load + merge Camel.toml (and includes) into one `toml::Value` tree.
2. Recursive walk over every string leaf (nested tables + arrays of tables), path-aware
   (field names like `security.native.bearer_token`, `components.timer.period`).
3. Dispatch per path: leaves under a STRICT prefix route through
   `resolve_fail_closed` (strict gate — legacy `{{` rejection, `interpolate_env`, residual
   rejection); all other leaves resolve through prefix-gated `interpolate_env` (uniform
   fail-closed).
   **Strict-class principle** (criterion, stated): a section is STRICT iff its string leaves
   reach an external authenticator or connection secret — credential stores, connection URLs
   that may carry userinfo, password/token/secret fields. On these leaves a residual
   placeholder marker left as literal text is a silent authentication bug, so the full-form
   escape (`$${env:...}`) is REJECTED to keep residual-marker gates meaningful. The prefix
   list is the mechanism; the principle is the invariant that tells review when to extend it.
   **Strict prefix set** (single declared home): `STRICT_PREFIXES: &[&str] = &["security", "datasources", "idempotent_repo", "cache_repo"]` — one const in camel-config consumed by BOTH
   the walk's dispatch and the tests (tests assert against the same const, never a hand-copied
   literal). `idempotent_repo`/`cache_repo` join at class-creation: main gained redis repo
   config (`sentinel_password`, URL userinfo — hand-redacted Debug marks them credential-bearing)
   during this change's planning window; classification per the principle above. NOTE: these
   are TOP-LEVEL sections (`[idempotent_repo]`, `[cache_repo]`) — there is no `[repositories]`
   table in CamelConfig; profile forms (`[default.idempotent_repo]`) resolve to top-level
   names in the merged tree the walk operates on.
   The gate invokes the resolver when the leaf contains `${env:` OR `$$` (the standalone
   escape needs conversion on every leaf, including ones without a full `${env:` form);
   the legacy `{{` pre-scan runs unconditionally on raw leaves before any resolution.
4. Deserialize the resolved tree into `CamelConfig`.
5. DELETE `resolve_string_in_place` + its 12 call-sites, the non-security hand enumeration in
   `resolve_placeholders`, and `PropertiesResolver` from the load path entirely (its public
   API may remain exported for compatibility, but nothing in the Camel.toml load path calls
   it). Kills rc-0wvi's dash trap by deletion.
6. The security/datasource hand-enumerated walks INSIDE `resolve_fail_closed` dispatch also
   die — the raw-tree dispatch by path prefix replaces them (same strictness, no struct-field
   enumeration to forget).
7. Anti-regression: a synthetic `[future_section] value = "${env:SOME_VAR}"` (unknown section,
   lands in `CamelConfig._extra` if the struct captures extras — otherwise asserted at the
   raw-tree stage) MUST resolve with zero resolver changes; unit tests cover the recursive
   walker over nested tables and arrays-of-tables paths.

### `$$` escape (T3)

`env_interpolation.rs` regex gains `$${env:...}` → literal `${env:...}`, plus standalone
`$$` → `$`. Zero `$$` usage repo-wide today — no regression surface.

**Escape vs strict-gate interaction (deliberate)**: the STANDALONE `$$` → `$` conversion
leaves no prohibited marker and works on ALL leaves — routes, non-security config, security,
and datasource leaves alike. Only the full form `$${env:X}` conflicts on strict-gate leaves:
it produces residual text `${env:X}`, which the strict gate's residual-marker rejection and
the store guard both REJECT on security/datasource leaves. This is intended: there is no
legitimate reason for a credential field to hold the literal text of a placeholder.

### Legacy `{{` hard-error (T4)

Any `{{` in a Camel.toml string leaf → `ConfigError` with an actionable message
("`Camel.toml` placeholders use `${env:NAME}` / `${env:NAME:-default}`; `{{...}}` is no longer
supported"). The check runs on the raw leaf BEFORE resolution (same pre-scan position as the
current security dash-guard). No deprecation window: pre-1.0, zero on-disk usage, and a silent
warn would perpetuate the "value silently wrong" class this change kills.

### T5 landmine — residual check reorder

`resolve_fail_closed` (config.rs:1911) rejects `contains("${")` AFTER `PropertiesResolver`
runs. Under the new syntax `${env:...}` is INTENDED content — the check as-is would reject
every valid config. Reorder: (1) unconditional legacy `{{` rejection on the raw leaf (this
SUBSUMES the old separate dash pre-scan — any `{{env:X:-default}}` legacy form dies here, so
the old dash trap cannot survive into resolution; compatibility with the v0.33 security
behavior is preserved through this broader pre-scan), (2) `interpolate_env` pass, (3)
residual check rejects only malformed/unconsumed `${` (e.g. truncated `${env:`,
`${notenv:...}` — NOT a consumed `${env:NAME}` and not a standalone-escape `$`). The
security walk's strictness (residual rejection) is preserved; its meaning narrows correctly.
The `:-` separator is native DSL syntax under `${env:}` and resolves normally.

### Uniform fail-closed (T6, Q9 approved)

`${env:X}` with X unset and no `:-default` → `ConfigError` naming the field, on EVERY config
leaf. Matches route-file DSL semantics exactly ("referenced but unset" is an operator error
everywhere). Optional values document `:-default`. `resolve_string_in_place`'s
warn-and-keep-original dies with it.

## Architectural Boundaries

- camel-config gains no new dependencies (camel-dsl already present).
- camel-dsl change is additive (escape branch in the regex + tests) — route semantics frozen.
- The data/control plane boundary is untouched; resolution stays at config load.

## ADR References

- ADR-0055 (publish topology): config→dsl one-way dep verified; no cycle introduced.
- ADR-0033 (fail-closed startup): uniform fail-closed disposition.
- ADR-0051 (credential boundary): store guard unchanged, carries forward.
- dead-config-policy spec: this change retires the allowlist class the policy tracks.

## Testing Strategy

camel-dsl: escape tests + route regression. camel-config: syntax migration, global-walk
coverage (incl. the anti-regression "new section resolves without code change" test), legacy
rejection, fail-closed uniformity, passthrough collision cases, T5 reorder. Full gates per
AGENTS.md at close.

## Task Decomposition (T1-T8, executable)

Task-number mapping: the design-level T1-T8 below describe WORK AREAS; the executable
tasks.md consolidates them as: tasks.md Task 1 = escape (design T3); tasks.md Tasks 2-3 =
walk + rewiring + legacy rejection + uniform fail-closed (design T1+T2+T4+T5+T6, split so
each dispatch fits one worker); tasks.md Task 4 = route regression; tasks.md Task 5 =
exhaustiveness guard replacement; tasks.md Task 6 = store-guard verification (proposal §7
guard carry-forward / ADR-0051 — verification only); tasks.md Task 7 = docs (design T7); tasks.md Task 8 (design T8 — CLI boundary) = CLI load-error surfacing (added post-plan-bless: run.rs swallowed every from_file error into silent empty-config defaults, killing the fail-closed contract at the CLI boundary — spec scenario "CLI surfaces load errors instead of silent defaults" added with it).

Materialization note (reviewer finding, load-bearing): the merged raw tree must be
materialized AFTER the config builder merges main file + includes + `CAMEL_*` env overrides
(deserialize builder output to `toml::Value`, walk, then `from_value::<CamelConfig>`) —
walking the pre-builder value would miss include-file and env-override placeholders.

- **T1 — Resolver reuse**: `interpolate_env` becomes the only placeholder engine in the
  Camel.toml load path; `PropertiesResolver` is removed from the load path entirely (public
  API may remain exported for external compatibility only).
- **T2 — Global walk**: rebuild `resolve_placeholders` as recursive string-leaf walk
  (typed + untyped, prefix-gated `${env:`); delete `resolve_string_in_place` and its 12
  call-sites; delete the non-security dash trap (rc-0wvi) by deletion.
- **T3 — `$$` escape**: regex + tests — standalone `$$` → `$` asserted on ALL leaf classes
  (route, non-security config, security, datasource); full `$${env:FOO}` → literal on routes
  and non-security config leaves; full form rejected on security/datasource leaves by the
  residual-marker gate (test).
- **T4 — Legacy `{{` hard-error**: pre-scan on raw leaves, actionable message, tests incl. a
  security field carrying `{{` (belt-and-braces with the existing strict gate).
- **T5 — Residual check reorder**: security walk sequence becomes legacy `{{` rejection → `interpolate_env`
  → reject malformed `${` only; regression: valid `${env:X}` security config passes, malformed
  `${env:` rejected.
- **T6 — Uniform fail-closed**: `${env:X}` unset-no-default → `ConfigError` on every leaf;
  tests for otel.endpoint, routes glob, log_level (previously warn-continue fields).
- **T7 — Docs sweep**: one interpolation chapter; camel-config README, schema.md,
  env-interpolation.md, DSL README cross-links; remove the two-syntax boundary notes added by
  the v0.33 security landing.
- **T8 — CLI boundary**: `camel run` config-load error surfacing — `load_config_or_default`
  helper gates the empty-config fallback on `Path::try_exists` of the MAIN file only; parse,
  include, and unresolved-placeholder errors abort with a visible error instead of silent
  defaults. Added post-plan-bless (user-confirmed bug: silent-default startup on bad config).

## Phases

Single-phase change — tasks T1..T8, no phase headings.
