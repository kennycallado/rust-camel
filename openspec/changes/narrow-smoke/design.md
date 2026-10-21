# Design: narrow-smoke

## Approach

Add `integration-tests-cxf = ["dep:camel-component-cxf"]` without changing the existing `integration-tests` feature list. Gate `tests/cxf_test.rs` with `any` of both features. Broaden only shared support imports, CXF module, bridge runtime, crypto initialization, and direct-send helpers needed by this target. Leave unrelated support modules under the full feature.

Change the workflow feature selector. Reduce its timeout from 150 to 60 minutes only if the measured cold narrow no-run build completes in at most 25 minutes. Otherwise retain 150 and report why, including when only a compiler-cache-warm measurement is available. The threshold leaves headroom for fewer CI build jobs, setup, bridge download, and the three end-to-end tests; local six-job timing is not a direct CI prediction. Update the header comment to name the narrow feature and remove the unmeasured cold-runtime estimate. Keep the published bridge acquisition, checksum verification, environment, filters, and three test-name assertions unchanged.

Measure `cargo test -p camel-test --features integration-tests-cxf --test cxf_test --no-run` using the dedicated empty `CARGO_TARGET_DIR=<worktree>/target-cold-narrow`, not the worktree target that holds xtask. Set `RUSTC_WRAPPER="" CARGO_BUILD_RUSTC_WRAPPER=""` and record sccache statistics before and after. Call the measurement cold only when compiler wrappers are disabled and no cache hits occur. Otherwise label it empty-target, sccache-warm and record hit/miss changes. Record elapsed time, exit status, compiler cache settings, and resource limits. Then compile the full target using the same target directory for compatibility; label that timing incremental, not a cold comparison. Remove the dedicated measurement directory after collecting evidence. Do not spend another multi-hour cold build solely to obtain a baseline. Historical CI timeouts do not establish an exact speedup ratio.

Record measurements in a mission report under this change directory and in rc-pqj93 notes, never in docs/audits. Report the remaining unconditional dependency closure: camel-config with kubernetes (kube), camel-component-sql (sqlx), camel-component-opensearch, camel-component-container (bollard), camel-component-mcp, camel-master, and camel-language-js. Do not claim the narrow feature removes these dependencies.

All cargo batteries use a unique `systemd-run --user --scope --collect` unit with MemoryMax=12G, MemorySwapMax=2G, TasksMax=1024, OOMPolicy=kill, TMPDIR=/home/shared/tmp and CARGO_BUILD_JOBS=6. Stop each unit afterward. No cargo build in main; no temporary files under /tmp.

## Affected crates

- camel-test: additive feature and test-support compilation gates.
- camel-cli: run feature_profiles after the manifest change; do not regenerate its golden unless its actual profile closure changes.
- CI: published-bridge-smoke.yml selects the narrow target.

## Architecture boundaries

No Runtime, DSL, Components, Services, Languages, or Functions behavior changes. The change affects test compilation only. ADR-0055 keeps camel-test the publish-order leaf sink; no new dependency edges are introduced. ADR-0070 staged listeners and existing CXF fixtures remain unchanged. CONTEXT-MAP.md defines these boundaries.

## Alternatives considered

Increasing the timeout retains the unnecessary compilation. Splitting camel-test or moving all unconditional dev dependencies expands scope and risks unrelated tests. Begin with the additive feature and measure the remaining cost honestly.
