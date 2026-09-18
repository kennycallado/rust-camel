# rc-9720m — camel-bundles bridge optionality (mission 115 slimblockers)

## Implementation

- `feat(bundles): optional bridge set behind http-static` (6f146541) — eight
  bridge deps (cxf, jms, opensearch, redis, sql, ws, xj, xslt) optional,
  activated by `dep:` entries on the EXISTING http-static feature; zero new
  `[features]` keys (lint-gate-forwarding Rule 2 requires boot-consumer
  camel-cli to forward every key; its dependency wiring was out of zone).
  Boot registration, BridgeCleanup xslt/xj fields, and BootHandle jms/cxf
  pool fields + teardown steps 1/3 gated on the carrier; ctx.stop() and
  close_all() stay unconditional. camel-master stays unconditional.
- Rename ride-along (owner grant post-108): `feat(cli): rename slim-http to
  slim-benchmarks` (fd97159f) with one-release alias.

## Verification

- feature_profiles suite ×3 green, `default_closure_matches_golden` green
  WITHOUT fixture regeneration at v0.49.0 (golden pin held).
- `cargo xtask lint-gate-forwarding` exit 0 (zero new keys).
- Compile matrix green: camel-bundles default/no-default/all-features;
  camel-cli default/slim-http/slim-benchmarks/all-features;
  camel-config; camel-integration-test.
- camel-bundles tests green in both feature polarities (8/7 lib tests);
  kafka polarity green. camel-cli FULL suite green (36 result-ok blocks,
  0 failures). clippy `-D warnings` both polarities; fmt clean.
- Cargo.lock zero drift vs base 37ec0cb6.
- camel-bundles `--no-default-features` closure: 7 of 8 bridges absent;
  enabling http-static restores all 8. Size: slim 55,712,576 → 55,652,352 B
  (−60,224, dead-code elimination of gated registration; crates stay linked
  via camel-cli's own deps — see evidence/slim-size.md).

## Remaining deferral (the camel-cli bridge-forward mission)

camel-cli's own eight unconditional bridge deps keep the crates in every
camel-cli profile. Full slim exclusion needs: per-bridge camel-bundles
features + camel-cli forwards (lint-clean as one change), camel-cli own-dep
optionalization, removal of the http-static carrier + cfg-key renames,
SLIM_FORBIDDEN_PREFIXES re-add, and the redis survivor (camel-config →
camel-redis-repo path, camel-config zone). Golden regeneration may be
required there — adjudicated by that mission.

## Reviews

r_glm per-task (T1.1, T1.2 APPROVE-WITH-FINDINGS, all applied), r_glm
inter-phase APPROVE ×2, holistic + e_glm stage-4 verdicts in the parked
report (.opencode/fleet/inbox/slimblockers-parked.json).
