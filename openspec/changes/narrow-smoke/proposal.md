# Proposal: narrow-smoke

## Why

The published CXF bridge smoke enables unrelated integration dependencies. Previous jobs exceeded 60 minutes and then stalled under a 150-minute timeout. This blocks rc-cln4q. Tracking: rc-pqj93, mission 377.

## What Changes

Add `integration-tests-cxf` to camel-test. Enable only the optional CXF component. Make the CXF target and required shared helpers available under either the narrow or full feature. Switch the published bridge smoke to the narrow feature. Reduce its timeout from 150 to 60 minutes only if a measured cold narrow no-run build completes in at most 25 minutes; otherwise retain 150 and report why. Preserve download, checksum, and all three end-to-end assertions.

## Acceptance criteria

- The narrow and full CXF targets compile. Feature-profile tests and narrow clippy pass.
- Record elapsed time for an empty-target narrow build, cache conditions, and full-build timing if affordable. Historical CI timeouts are context, not a measured local baseline.
- The workflow parses and keeps its artifact checks and test assertions unchanged.
- Park the branch with a report. Do not merge, archive, close bd, bump versions, or push.

## Risk budget

No runtime, protocol, release, or dependency-version changes. Existing unconditional dev dependencies remain unless evidence requires a separately approved scope change. A 15–20 minute cold build is a goal, not a result to claim without measurement.
