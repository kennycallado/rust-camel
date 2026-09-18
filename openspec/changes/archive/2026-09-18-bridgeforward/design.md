# Design: bridgeforward

## Approach

Five verified facts drive the design:

1. **lint-gate-forwarding semantics** (scripts/xtask/src/
   lint_gate_forwarding.rs): gates = every camel-bundles `[features]`
   key except `default`. Rule 1: a consumer feature shadowing a gate
   name must forward `camel-bundles/<gate>` — this binds EVERY
   consumer, and camel-integration-test's `sql = ["dep:sqlx"]` shadows
   the new `sql` gate, so that feature gains
   `"camel-bundles/sql"` (mirror of its existing `security` forward;
   closure-safe — camel-cli default already activates
   camel-component-sql, and harness full-boots gain coherent SqlBundle
   registration). Rule 2: camel-cli (the only `boot-consumer = true`
   manifest) must forward every gate through some feature.
2. **`http-static` is a real gate, not vestigial.** Pre-115
   (f99716f8~1) it was `http-static = []` AND gated
   `HttpStaticBundle` registration (the `http-static:` URI scheme,
   ADR-0009). 115 only appended the eight `dep:` carrier entries. So
   the key STAYS, reverts to `[]`, and keeps gating HttpStaticBundle;
   what this change deletes is the carrier content, exactly "what this
   mission obsoletes" per the camel-bundles in-repo carrier comment
   (which assigns the split and cfg renames to this mission).
3. **cargo-tree rendering rules** (empirically established by 115):
   forwarding-seeded features render nothing; `dep:`-activated edges
   render the same package + `feature "default"` lines as unconditional
   edges. camel-cli consumes camel-bundles with `default-features =
   false` (workspace table), so camel-bundles' `default` changes never
   reach the camel-cli golden fixture; per-bridge features seeded via
   the new camel-cli forwards render no lines of their own. Golden
   stays byte-identical without regen.
4. **Redis cannot drop** (verified with `cargo tree`): camel-cli →
   camel-config → camel-redis-repo → camel-component-redis (+ redis
   1.6.0 client) is unconditional and out of zone. Slim keeps redis;
   the other seven bridges drop. `redis-tls` (secure-by-default,
   rc-ayy11) becomes `redis-tls = ["redis", "camel-component-redis/tls"]`
   so TLS still implies the capability.
5. **`otel` coupling is pre-existing**: `otel` forwards
   `camel-component-ws/otel`, which activates the (now optional) ws dep.
   Default/full enable ws anyway; a standalone slim+otel build links ws
   exactly as it does today. Semantics preserved, no new surface.

**Feature shape.** camel-bundles: `jms = ["dep:camel-component-jms"]`
(and likewise sql, redis, opensearch, ws, cxf, `xj = ["dep:camel-xj"]`,
`xslt = ["dep:camel-xslt"]`); `default` gains the eight (package set
identical: pre-115 they were non-optional, post-115 carrier-on-default).
camel-cli: same-named features `jms = ["dep:camel-component-jms",
"camel-bundles/jms"]` etc.; the eight own deps become `optional = true`;
`full` gains the eight. The camel-cli `http-static = 
["camel-bundles/http-static"]` forward stays (Rule 2; now the
static-scheme gate). Known transitivity: camel-xj depends on camel-xslt,
so `xj` alone links the xslt crate while the XsltComponent registration
stays off — accepted and documented (default/full enable both).

**cfg renames.** camel-bundles/src/lib.rs: every "Transitional gate:
rides http-static" site renames to its own bridge feature — BridgeCleanup
`xslt`/`xj` fields + their `stop()` drains, BootHandle `jms_pool`/
`cxf_pool` fields + shutdown steps 1/3, the xslt/xj runtime blocks, the
WsBundle, jms/cxf pool registrations, opensearch/redis/sql bundle sites.
The HttpStaticBundle gate is untouched. Teardown steps 2 (`ctx.stop`)
and 4 (`close_all`) stay unconditional. camel-cli/src/lib.rs
`register_builtin_components_for_lint`: per-bridge `#[cfg(feature =
"...")]` gates on xslt, xj, sql, ws, opensearch, redis, jms, cxf
(slim lint degrades to `unverified-scheme` notes, same accepted class
as wasm/exec).

**camel-bundles test polarity.** The all-bundles probe gates on
`all(eight features)`; the slim probe (`not(feature = "jms"))` asserts
core-only registration and pool-free shutdown.

## Affected crates

- `camel-bundles`: manifest feature split (+8 keys, carrier removed,
  default extended), src/lib.rs cfg renames, test cfg polarity.
- `camel-cli`: manifest (8 optional deps, 8 forwarding features, full
  extension, redis-tls implication), src/lib.rs lint-catalog gates,
  tests/feature_profiles.rs (SLIM_FORBIDDEN_PREFIXES + slim composition
  tests), CONTEXT.md profile notes.
- `camel-integration-test`: one manifest line — `sql` gains the
  `"camel-bundles/sql"` forward (lint Rule 1; the harness is a
  camel-bundles consumer whose `sql` feature shadows the new gate).
- Docs: camel-bundles CONTEXT.md feature-note paragraph if it names the
  carrier (verified during implementation).

## Architecture boundaries

Components zone (camel-bundles cascade, camel-cli manifest/lint
catalog). No Runtime/DSL/Services/API changes. The ADR-0069 §10
boot-cascade contract is preserved: default builds register the
identical component set; feature-off builds register the reduced set by
explicit opt-out. camel-bundles edits are strictly the feature-table
split, the forced cfg renames, and test polarity — the mission's
bounded camel-bundles grant.

## Alternatives considered

- **Delete the camel-bundles `http-static` key entirely:** rejected —
  it gates HttpStaticBundle (predates 115); deletion would ungate the
  `http-static:` scheme and break Rule 2 unless camel-cli kept a dead
  forward.
- **Keep the carrier, optionalize only camel-cli own-deps:** rejected —
  default/full would retain all eight via the carrier forward, and slim
  would drop them, but per-bridge composition (slim + sql) could not
  activate a single bridge on the bundles side: the only bundles-side
  switch would remain the all-or-nothing carrier, and slim+bridge
  builds would link a bridge dep camel-bundles never registers.
- **camel-template minijinja flip here:** rejected — 115 recorded that
  the named-feature shape renders extra deptree lines (golden regen);
  this change's hard gate is golden-green without regen. Follow-up bd.

Single-phase change: the manifest splits, cfg renames, and tests land
as one coherent slice (nothing compiles or lints green halfway).
