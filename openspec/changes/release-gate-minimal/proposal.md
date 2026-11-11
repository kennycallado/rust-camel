# release-gate-minimal

Release preflight requires build and unit-test evidence instead of a fully
green CI run; Full Tests becomes advisory.

## Why

2026-11-11, v0.67.0: the release was blocked by a wall-clock ratio test
(`camel-language-js/tests/js_engine_cache_amortization.rs`, steady state
514.6 µs vs eval1 1993.9 µs, 3.9x against a 4x floor). The test missed by
2.5 % on a loaded runner; every other job was green. Because
`release.yml` required `conclusion == "success"` on the whole run, one
flaky timing test invalidated the SHA and blocked publishing.

Owner ruling (2026-11-11): the release gate must not depend on a single
perfect run. Build and unit tests are the binding evidence; Full Tests is
advisory. A real build or unit-test failure still blocks.

## What changes

- `ci-preflight` considers a commit tested when any completed
  push-on-main `ci.yml` run has an attempt whose `Unit Tests
  (ubuntu-latest)` job is green with both `Build` and `Test (unit only —
  no Docker required)` steps success. Evidence is unioned across attempts,
  so a flaky first attempt that passes on re-run no longer blocks.
- `Full Tests (ubuntu-latest)` is reported as advisory (warning) and never
  fails the preflight.
- The fail-closed properties stay: push-on-main only (PR runs never
  qualify), no run or no green unit evidence fails, API errors fail, and
  `call-release-matrix` keeps `needs: [ci-preflight]`.

## References

- `rc-86u4n` flaky JS amortization ratio test.
- `rc-99d5` flaky-test taxonomy epic.
- `rc-1x9fl` original zero-evidence release incident.
