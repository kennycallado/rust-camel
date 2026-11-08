# musl-fetch-resilience

Make the musl toolchain download in the bridge native builds survive a transient 504 storm (bd rc-1c47h, mission 530).

## Why

`Full Tests` run 42462417839 failed at `Build JMS bridge binary`: `bridges/jms/build-native.sh` downloads the 89 MB musl toolchain from a GitHub release asset with `curl --retry 5 --retry-delay 5` — a ~25 s window that six consecutive HTTP 504s exhausted; the asset returned 200 minutes later. The tarball re-downloads on every CI run because the cache dir (`${GRADLE_USER_HOME:-/tmp}/musl-toolchain-cache`) is not restored in the `full-tests-linux` job, and 25 s of patience does not cover a release-asset outage.

## What changes

- Retry with backoff in both `bridges/jms/build-native.sh` and `bridges/cxf/build-native.sh` (block kept textually aligned): explicit loop with exponential backoff (5/10/20/40/60 s, capped at 60 s) inside a ~10 minute total budget (`MUSL_FETCH_BUDGET_SECONDS`, default 600), same `curl -fSL --retry-all-errors -C -` semantics per attempt (partial transfers resume), exhaustion exits 1 naming the URL, attempt count and elapsed budget; pinned `MUSL_TOOLCHAIN_SHA256` check, the "asset may have been replaced" message and the cache-hit fast path stay untouched; `MUSL_TOOLCHAIN_URL_OVERRIDE` allows pointing the fetch at a test stub (default unchanged, SHA check still fails a wrong payload).
- Offline battery `scripts/tests/musl-fetch-retry-test.sh` in the `verify-release-assets-test.sh` style: python3 `http.server` stub on 127.0.0.1 (ephemeral port) answering 504 N times then 200; scenarios `retries_until_success`, `gives_up_after_budget`, `sha_mismatch_aborts`, `cache_hit_skips_download`; sources the production functions via `build-native.sh --print-musl-fetch` so the tested logic cannot drift from what CI runs; no network, Docker or root; server cleaned up on exit.
- Workflow cache in `full-tests-linux` (.github/workflows/ci.yml): `actions/cache` for `bridges/jms/.gradle-docker-cache/musl-toolchain-cache` and the CXF counterpart, keyed on toolchain version + pinned SHA, before the bridge build steps; the Reclaim steps stop deleting the `musl-toolchain-cache` subdir so the post-job cache save has something to save. (Offline validation covers YAML parse + path mapping via the existing release-workflow precedent; runtime save/restore behavior is CI-verified.)
