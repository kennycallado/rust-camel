# Mission 377: parked for master

- Branch: `chore/narrow-smoke`.
- Worktree: `/home/shared/rust-camel-worktrees/377-narrow-smoke`.
- Rebased implementation HEAD: `6f8234d476d59813b1ee9827e913bc64974b8ae0`.
- Main baseline: `384b54aba6ee2e6c98cdaacc39cb7f2503afb23b`.
- Rebase had no conflicts; wasmtime 49.0.4 and CXF/JMS 0.8.4 pins survive.

## Results

- Cold narrow CXF no-run compile: **449.262 seconds**, exit **0**, dedicated
  empty target, all compiler wrappers disabled. Cargo reported **7m29s**.
- Full integration-tests CXF no-run compile: **540.478 seconds**, exit **0**,
  incremental after narrow; completed inside the 1800-second bound. Cargo
  reported **9m00s**. This is not a cold baseline or a speedup ratio.
- CPU: Ryzen 5 5600G, 12 logical CPUs, six cargo jobs. Rust 1.98.1,
  Cargo 1.98.1. Shared sccache counters include concurrent agents; direct
  compiler invocation bypassed the cache. Full statistics and realtime-clock
  methodology disclosure are in `mission-report.md`.
- The three workflow test names are present in the 44-test narrow listing.
- Timeout: **150 to 60 minutes**, because the completed cold build is below
  the approved **1500-second** threshold. Only the selector, timeout and header
  changed; acquisition/checksum/environment/filters/assertions are preserved.
- feature_profiles: **19 passed**; fmt and narrow all-targets clippy: **0**.
- Conductor registry: **26 passed**, cargo-audit **1** (verified pre-existing
  main failure, follow-up **rc-xjwwk**, exempt rather than green).
  lint-commits skipped by conductor policy; doc-guide-test N/A.
- Bun YAML parse and exact workflow diff-scope checks passed.
- Task reviews and fresh holistic review: **r_gpt APPROVE**, no open findings.
- Only this mission's dedicated cold target was deleted. Ordinary target stays.

## pending_for_master

- Review the branch and run the published-binary smoke in CI;
  integration-verification-deferred-to-CI. No CI duration is claimed.
- Resolve dependency advisories separately via `rc-xjwwk`.
- Integration, archive, issue closure and push remain master/human actions.
  No merge, archive, rc-pqj93 closure, version bump, or push was performed.

Evidence: `openspec/changes/narrow-smoke/mission-report.md` and
`/home/shared/tmp/rc377-*.log`. The final parked commit contains this report;
the rebased implementation HEAD above records the measured source revision.
