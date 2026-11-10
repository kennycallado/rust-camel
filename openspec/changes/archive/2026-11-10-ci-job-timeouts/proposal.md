# Proposal: ci-job-timeouts

## Why

`timeout-minutes` is missing on 24 of 47 jobs across 12 of 23 workflow files (live audit at main `fd938bbb3`; the mission order's estimate said 16 files — the audited set below is authoritative). A hung job on a shared runner burns the 6-hour default, blocks its concurrency group, and leaves spawned processes behind. This is the timeouts-only half of the adjudicated `rc-0nrer` mitigation (e_gpt, SURVIVED); the orphan-cleanup half is filed as `rc-2w5sj` (`discovered-from:rc-0nrer`) and is out of scope here.

## What Changes

- Add an explicit `timeout-minutes` to every job that lacks one and supports the keyword. Values follow an evidence rule (see design): keep every existing value; never lower; 2× observed where evidence exists (gh job timings, in-file comments, sibling-class pins); documented class defaults otherwise.
- No behavioural change beyond the timeout: triggers, steps, matrixes, permissions, concurrency, and `needs:` graphs are untouched. No new steps, no retries, no `continue-on-error`.
- bd: `rc-0nrer` (this change), `rc-99d5` (parent epic), `rc-2w5sj` (deferred cleanup half).

## Acceptance criteria

- All 45 supported jobs (23 existing keeps + 22 additions) carry an explicit `timeout-minutes` value; zero supported jobs lack an explicit timeout. The 2 reusable-workflow caller jobs (`call-release-matrix` in `release.yml`, `release-dev.yml`) are documented `N/A (caller)` — GitHub Actions does not support `timeout-minutes` on `uses:` jobs; adding it would invalidate the workflows. They are bounded transitively by callee timeouts.
- The full 47-job audit table (design.md) travels with the park.
- Every edited YAML parses (`python3 -c 'import yaml; yaml.safe_load(open(f))'`).
- No existing timeout lowered; diff touches only timeout lines (plus job-level placement).
- `bash scripts/gates-local.sh` exit 0.
- actionlint run if available locally, else reported absent.

## Risk budget

- Acceptable: a value too generous (bounded burn, still far below the 6-hour default).
- Not acceptable: a value that kills a legitimate cold-cache release build; lowering an existing timeout; any non-timeout behavioural change.
- No CI behaviour claim beyond this diff: the first real evidence is the next workflow run on `main`.
