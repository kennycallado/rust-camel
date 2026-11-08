# xml-musl-fetch

Close the XML bridge musl exposure (bd rc-8gpgd): give `bridges/xml/build-native.sh` the same hardened fetch that mission 530 landed for jms/cxf (commit `1e406f310`), plus the matching `ci.yml` cache step and reclaim carve-out.

## Why

Mission 530 hardened the musl toolchain fetch in `bridges/jms/build-native.sh` and `bridges/cxf/build-native.sh` (exponential backoff 5/10/20/40/60 s over a 600 s budget, attempt count and budget in the failure message, `MUSL_TOOLCHAIN_URL_OVERRIDE` for the offline battery, cache-key sync battery) and added `actions/cache` steps for both Gradle caches with reclaim carve-outs. `bridges/xml/build-native.sh` still carries the old block: `curl -fSL --retry 5 --retry-delay 5` (~25 s window), no override hook. `ci.yml` has no cache step for `bridges/xml/.gradle-docker-cache`, and its `Reclaim XML bridge artifacts` step wipes the whole cache dir, so even a cache step would be decorative. The same 504 storm that killed the JMS build can kill the XML one.

## What changes

- `bridges/xml/build-native.sh`: replace the inline fetch block with the landed jms/cxf implementation — extract `verify_archive`, `fetch_musl_toolchain`, `ensure_musl_archive` byte-identical to the jms/cxf bodies (backoff schedule, budget, override hook, exhaustion message, pinned-SHA check, cache fast path), add the `--print-musl-fetch` test hook, keep the script's own arg parse / container guard / traps / Gradle error handling intact.
- `.github/workflows/ci.yml` (`full-tests-linux`): add `Cache musl toolchain (XML bridge)` (path `bridges/xml/.gradle-docker-cache/musl-toolchain-cache`, key `musl-toolchain-xml-v11.2.1-<pinned SHA>`) before `Build XML bridge binary`, and update `Reclaim XML bridge artifacts` to preserve `musl-toolchain-cache` exactly like the JMS/CXF steps.
- `scripts/tests/musl-fetch-retry-test.sh`: extend, not duplicate — add `t_xml_function_parity` and cover all three cache keys in `t_ci_cache_key_sync`; PASS/summary format and offline guarantee unchanged.
