# wire-assurance-battery

Run `scripts/tests/assurance-report-body-test.sh` in the CI `quality` job so the 12-scenario offline battery for the hardened report body builder is exercised on every PR instead of rotting silently.

## Why

Trivial change — no spec breakdown needed.

## What changes

- `.github/workflows/ci.yml` (`quality` job only): add one step `assurance report body stub tests` running `bash scripts/tests/assurance-report-body-test.sh`, immediately after `verify-release-assets stub tests` and before `lint-unwrap` — same pattern as the rc-6ep0i precedent.
- No new workflow, no job restructuring, no changes to the battery script or `scripts/assurance-report-body.sh`; the battery is offline and self-contained.
