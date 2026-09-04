# Design: nextest-pilot

## Approach

Introduce cargo-nextest as the executor for the container-free ubuntu Rust
library-test job only, with a checked-in `.config/nextest.toml` `ci` profile that
encodes the adjudicated policy (e_gpt counter-review §5.4, bd rc-mhsn):

- `retries = 1` — one diagnostic retry. Not tolerance: with
  `flaky-result = "fail"` a retry-passing test is reported FLAKY and
  fails the gating job. Detection, not forgiveness.
- `slow-timeout = { period = "30s", terminate-after = 3 }` — effective
  termination ceiling ~90s. Nextest semantics: the period repeats
  `terminate-after` times before kill; the design ceiling is therefore
  period x terminate-after, documented here because the counter-review
  flagged the period-alone reading as a common error. Current library-test-job
  slowest binaries run in ~6s, so ~90s is a generous ceiling that a
  legitimate library test cannot hit.
- `failure-output = "immediate-final"`, `fail-fast = false` — full signal
  per run; one failure does not hide the rest.
- Verified against cargo-nextest 0.9.143 (the locally installed version):
  `flaky-result` is a real profile/CLI option (`--flaky-result
  <pass|fail>`, default "from profile").

CI wiring (`ci.yml`, Unit Tests ubuntu leg): add a pinned-SHA
`taiki-e/install-action` step (`tool: cargo-nextest@0.9.143`) mirroring the
existing cargo-llvm-cov/cargo-audit convention, then replace
`cargo test --workspace --lib` with `cargo nextest run --workspace --lib
--profile ci`.

Scope guards (hard, per e_gpt cost ruling): testcontainers, K3s, and
bridge suites stay on `cargo test` — process-per-test would break
process-local shared-container fixtures (Redis, Kafka, OpenSearch,
SurrealDB OnceCells in camel-test). The Full Tests job, the macOS
build+smoke job, and ci-weekly.yml are untouched. No doctest job is
added: the Rust library-test scope runs `--lib` only today, so nextest's doctest
omission changes nothing.

Selection parity: `cargo nextest run --workspace --lib` builds the same
test targets cargo test selects for `--lib`; the pilot's first CI run
records the nextest-reported test count against the known cargo baseline
(sum of per-binary counts, e.g. camel-http 280, camel-component-ws 133)
in bd rc-mhsn. Divergence >0 blocks rollout.

Measurement (pilot exit criteria): for two weeks record wall time,
nextest-reported process count, and any FLAKY/slow-timeout events on
main runs in bd rc-mhsn. Rollout decision (widen scope or revert) is a
human call recorded there.

## Affected crates

- None. Changes: `.github/workflows/ci.yml` (one install step, one run
  line), `.config/nextest.toml` (new file).

## Architecture boundaries

No Runtime/DSL/Component boundary is crossed — this is test-execution
infrastructure. It aligns with the test-determinism capability canon. It does not amend
ADR-0069, which governs `.test.yaml` full-tier scenario documents; this
pilot changes only Cargo `--lib` test execution. Permanent policy, if the
pilot is accepted, gets its own focused ADR per the counter-review
governance ruling — out of scope here.
