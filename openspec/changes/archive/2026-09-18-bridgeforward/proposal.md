# Proposal: bridgeforward

## Why

Mission 115 (f99716f8) made the eight camel-bundles bridge deps optional,
hung on the existing `http-static` feature as carrier, but camel-cli kept
its own unconditional dependency edges on the same eight crates
(jms, sql, redis, opensearch, ws, cxf, xj, xslt). The slim binary
therefore kept linking them: −60 KB (−0.11%). The lock was removed, not
the load. This change is the other half: camel-cli learns to EXCLUDE the
bridges (bd rc-9720m, the 115 hinge), so the slim profile finally drops
the bridge chains (sqlx, CXF/SOAP stack, JMS, opensearch, WS, XSLT/XJ) —
expected payload drop in the MBs, not KBs.

## What Changes

- camel-bundles: the eight `dep:` entries leave the `http-static`
  carrier and become eight per-bridge feature keys (`jms`, `sql`,
  `redis`, `opensearch`, `ws`, `cxf`, `xj`, `xslt`), all added to
  `default`. `http-static` reverts to its true, pre-115 meaning: the
  `HttpStaticBundle` registration gate (it was never vestigial). The
  `#[cfg(feature = "http-static")]` transitional gates in
  camel-bundles/src (boot registrations, BridgeCleanup xslt/xj fields,
  BootHandle jms/cxf pools) rename to the per-bridge features.
- camel-cli: eight same-named forwarding features
  (`<bridge> = ["dep:<crate>", "camel-bundles/<bridge>"]`, satisfying
  lint-gate-forwarding Rules 1 and 2), own bridge deps become optional,
  `full` gains the eight, `redis-tls` implies `redis`, and the lint
  catalog registrations in `register_builtin_components_for_lint` gain
  matching cfg gates. The camel-cli `http-static` feature remains as
  the legitimate forward of the bundles gate (carrier semantics gone).
- camel-integration-test: its `sql` feature gains the
  `camel-bundles/sql` forward — the harness is a consumer whose feature
  name shadows the new gate, so Rule 1 requires the forward.
- tests: `SLIM_FORBIDDEN_PREFIXES` re-gains the seven droppable bridge
  prefixes (`camel-component-redis` excepted: camel-config →
  camel-redis-repo keeps it linked in slim — out-of-zone deferral).
- camel-template/minijinja default-features flip is OUT: the 115 design
  records that the named-feature shape requires golden-fixture
  regeneration, and this change's gate is golden-green WITHOUT regen.
  Re-anchored as a follow-up.

## Acceptance criteria

- Golden default deptree fixture green WITHOUT regeneration.
- Slim closure test asserts the seven bridges EXCLUDED; redis exception
  documented.
- `cargo xtask lint-gate-forwarding` green (17 gates, all forwarded).
- camel-bundles + camel-cli compile matrix green (default, slim,
  per-bridge subsets); boot tests updated to per-bridge polarity.
- Slim release binary size delta in the MBs vs the recorded BEFORE
  (55,652,544 bytes); if only KBs, STOP and report.
- 12 xtask lints, fmt, clippy, doc-build green.

## Risk budget

The normalized default closure output must remain byte-identical and
default/full boot behavior unchanged (golden fixture + boot tests are
the proof). No public API changes.
Redis slim retention accepted (out of zone). camel-bundles touches are
limited to the feature-table split, the cfg-key renames it forces, and
its tests — nothing else.
