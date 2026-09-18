# Proposal: slimblockers

## Why

Mission 102 (clidiet, bd rc-g0009) created the `slim-http` profile but deferred
two hard edges to a camel-bundles-side mission: camel-bundles depends
UNCONDITIONALLY on the eight bridge crates (jms, sql, redis, opensearch, ws,
cxf, xslt, xj), and camel-template hard-depends on camel-language-minijinja +
minijinja. Neither can be excluded from any consumer profile while those deps
stay unconditional (bds rc-9720m, rc-wcs3v).

The deferral lease expired when mission 102 archived; this change owns
`crates/camel-bundles` and `crates/components/camel-template`. camel-cli was
FORBIDDEN at design time (mission 108 owned it); mission 108 landed on
2026-09-18 and the owner granted ONE bounded camel-cli addition on resume:
the slim-benchmarks rename (scope item 4). The camel-cli bridge-forward diet
stays deferred to a dedicated camel-cli mission. camel-config and camel-dsl
remain out of zone.

## What Changes

1. **rc-9720m — camel-bundles bridge optionality.** The eight bridge deps
   become `optional = true`, activated by `dep:` entries on the EXISTING
   `http-static` feature (camel-bundles `default` is unchanged — it already
   contains http-static). Registration sites in `boot()` and the `BootHandle`
   pool fields are cfg-gated on `feature = "http-static"`. Why aggregate and
   not per-bridge features: `cargo xtask lint-gate-forwarding` Rule 2 requires
   every boot consumer (camel-cli, `boot-consumer = true`) to forward EVERY
   camel-bundles feature key; camel-cli was forbidden zone at design time, so any new
   feature key fails the mandatory lint. Zero new keys is the only lint-clean shape;
   per-bridge features ride the next camel-cli mission together with the
   consumer forwards and SLIM_FORBIDDEN_PREFIXES updates. Empirically
   validated: the bridges stay active in every camel-cli default build (full
   forwards `camel-bundles/http-static`) and forwarding-seeded activation
   renders no `cargo tree -e features` lines, keeping the camel-cli default
   deptree byte-identical.
2. **rc-wcs3v — camel-template minijinja optionality.** `camel-language-minijinja`
   and `minijinja` become optional dependencies enabled by `dep:` entries in
   the crate's `default` list — no other feature keys (named features in
   default render extra cargo-tree lines, and `dep:` entries set no named
   feature; both verified empirically). Engine-coupled modules (bundle,
   closure, component, endpoint, lifecycle, producer, reload, template_set,
   and gated lib exports) are cfg-gated on `feature = "default"`; the public
   config/error modules and private path_util/uri stay engine-free. The
   crate becomes all-or-nothing: default = full component,
   `default-features = false` = engine-free. The hard edge is removed at the
   source: external `default-features = false` consumers drop the engine
   immediately. camel-bundles keeps consuming with
   defaults (the workspace-inheritance rule forbids a member-level flip, and
   the workspace-table entry is shared with forbidden consumers), so
   in-workspace exclusion of minijinja, the named re-enable feature, and the
   cfg-key rename ride the next camel-cli mission.
3. **Measurement.** slim-http release binary size measured before/after in the
   worktree. Expected delta: zero this mission — the bridge crates remain in
   camel-cli's slim closure through camel-cli's OWN unconditional deps (the
   camel-cli-side half of the deferral). The measurement documents that
   honestly; the size win materializes when the next camel-cli mission
   optionalizes camel-cli's own edges.
4. **slim-benchmarks rename (zone-granted on resume, 2026-09-18).** Mission
   108 landed, freeing camel-cli. The profile feature `slim-http` is renamed
   `slim-benchmarks` (owner ruling 2026-09-17: the slim profile is
   benchmark-oriented) with `slim-http = ["slim-benchmarks"]` kept as a
   one-release alias. Bounded surface: camel-cli/Cargo.toml feature lines +
   the three slim-http literals in tests/feature_profiles.rs. The camel-cli
   bridge-forward diet (per-bridge features, own-dep optionalization,
   SLIM_FORBIDDEN_PREFIXES re-add) stays deferred to a dedicated camel-cli
   mission — NOT granted here.

## Acceptance Criteria

- `default_closure_matches_golden` stays green WITHOUT fixture regeneration
  (feature_profiles suite ×3). Any forced regen: STOP, park BLOCKED.
- `cargo xtask lint-gate-forwarding` green (no new camel-bundles feature keys).
- Compile matrix: camel-bundles (default, --no-default-features,
  --all-features), camel-template (default, --no-default-features), camel-cli
  (default, slim-http, --all-features) all green.
- camel-cli full test suite green; fmt + clippy on touched crates
  `--all-targets -D warnings`.
- Boot behavior unchanged for default consumers; the eight bridges absent from
  camel-bundles' own `--no-default-features` closure (camel-component-redis
  remains via the unconditional camel-config → camel-redis-repo path — out of
  zone, documented deferral; camel-xj drags camel-xslt transitively whenever
  enabled, documented).
- Slim binary size before/after reported with numbers.
- e_glm stage-4 review verdict recorded before parking (owner ruling
  2026-09-17).

## Affected Crates

- `camel-bundles` — manifest dep: entries on http-static, cfg-gated
  registration + BootHandle.
- `camel-template` — optional engine, cfg-gated modules.
- `camel-cli` (rename only) — slim-benchmarks feature rename + one-release
  slim-http alias + the test literals and CONTEXT.md mentions.

Risk budget: cargo-tree rendering semantics (validated empirically pre-spec),
BootHandle field gating (contained: no external consumer touches the pool
fields), golden fixture stability (pinned by test, zero-line diff verified).

Bd: rc-9720m, rc-wcs3v (discovered-from rc-g0009).
