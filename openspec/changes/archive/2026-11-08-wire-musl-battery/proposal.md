# wire-musl-battery

Run `scripts/tests/musl-fetch-retry-test.sh` in the `ci.yml` quality job (bd rc-zy1qo), next to its two sibling stub batteries.

## Why

`scripts/tests/musl-fetch-retry-test.sh` is offline (python3 stub server, no network/Docker/root) and guards the retry/backoff logic that missions 530 and 536 added to the three `build-native.sh` scripts, including the cache-key sync with `ci.yml`. Nothing runs it in CI, so drift is only caught locally. The quality job already runs the two sibling stub batteries (`verify-release-assets stub tests`, `assurance report body stub tests`) for the same reason.

## What changes

- `.github/workflows/ci.yml` (`quality` job only): add one step `musl fetch retry stub tests` running `bash scripts/tests/musl-fetch-retry-test.sh`, immediately after `assurance report body stub tests` and before `lint-unwrap`, same indentation as the neighbours. No `needs:`, no conditionals, no matrix — the battery is self-contained and runs on every quality invocation like its siblings. No other job or step is touched, and the existing stub-battery steps are not reordered.
