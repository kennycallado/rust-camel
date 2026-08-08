# ADR-0054: `#[ignore]` Test Classification and Enforcement Policy

**Date:** 2026-08-07
**Status:** Proposed
**Related:** ADR-0012 (lint+policy pairing precedent), ADR-0049 (lint+policy pairing precedent), ADR-0053 (the WIT break that exposed the gap)

## Context

### Problem

`cargo test --workspace` silently skips `#[ignore]` tests. ADR-0053's
`camel:plugin@1.0.0` change broke 13 buildable WASM tests in
`camel-component-wasm` that the merge gate never ran. The break surfaced only by
manual inspection during the ADR-0053 review cycle. No lint, no ADR, and no CI
coverage existed for `#[ignore]` discipline.

### Existing project pattern

The project already has an integration-test architecture for external services.
Service tests live in `camel-test/tests/` behind `--features integration-tests`,
self-provision dependencies via testcontainers, and run in CI's
`full-tests-linux` job. No `#[ignore]` annotation is involved. The pattern covers
Kafka (KRaft mode, no Zookeeper, `camel-test/tests/kafka_test.rs:1-9`), Redis
(`camel-test/tests/redis_test.rs:1-9`), OpenSearch, K3s, JMS/XML/CXF bridges,
and container/marshal integration. The `camel-test/Cargo.toml` already declares
`testcontainers` and `testcontainers-modules` with the `redis`, `kafka`,
`postgres`, and `k3s` features.

This pattern solves the external-service testing problem without `#[ignore]`. A
test that needs Kafka, Redis, OpenSearch, or K8s follows the camel-test pattern.
A test marked `#[ignore = "requires live <service>"]` duplicates that coverage
with inferior ergonomics.

### Audit finding

7 inline `#[ignore]` tests in component crates (`camel-kafka`,
`camel-redis`, and others) duplicated existing camel-test coverage. They have
been deleted (commit `4d3810f1`). 8 Ollama tests remain genuinely special: they
require a `qwen3.5:4b` model pull that testcontainers cannot provision.

## Decision

The workspace adopts a **closed vocabulary of two `#[ignore]` reason prefixes**.
Every `#[ignore]` in non-test-support code MUST carry exactly one of these
prefixes as its reason string. A test may only be `#[ignore]` for a prerequisite
that CI cannot cheaply satisfy.

### Closed vocabulary

1. **`requires pre-built <artifact detail>`** — a buildable artifact covered by
   a dedicated CI job. Example: WASM guests. The test file MUST appear in
   `allowlist-ignore.txt` (a path consumed by both this lint and the CI job).
   The CI job builds the artifact, then runs `cargo test --ignored` on each
   allowlisted file. Without the allowlist entry, the test will not run in CI.
2. **`slow test: <description>`** — a self-contained test that is slow to run
   and is legitimately excluded from the per-PR gate. The prefix is documentary;
   a future scheduled job can pick these up by grep.

`xtask lint-ignore` enforces the vocabulary. It rejects bare `#[ignore]`,
unrecognized prefixes, and `requires live` specifically with a migration error
pointing contributors to `camel-test`. The complete grammar and rejection rules
live in the xtask implementation; this ADR defines the policy, not the regex.

### External-service tests are not a valid category

Any test that requires an external service (Kafka, Redis, OpenSearch, K8s, a
database, Keycloak, etc.) MUST follow the camel-test + testcontainers pattern
and MUST NOT be `#[ignore]`. The lint treats `requires live` as a migration
error, not a valid prefix, because accepting it would codify an anti-pattern
the project already outgrew. The migration message points contributors at
`camel-test/tests/` and the existing testcontainers fixtures.

### No escape hatch

A contributor who believes their `#[ignore]` reason does not fit either prefix
must either:

1. Reconsider whether the test should exist at all and delete it.
2. Reconsider whether the prerequisite can be cheaply satisfied in CI and
   migrate the test to `camel-test` with testcontainers.
3. Propose a new prefix via a new ADR that amends this one.

A catch-all `other:` prefix or a free-text escape hatch would reintroduce the
exact ambiguity this ADR exists to eliminate. A closed vocabulary with an
escape hatch is not closed.

## Considered options

| Option | Description | Ruling |
|---|---|---|
| **A** | **Three-prefix vocabulary with `requires live`** | Rejected — blesses an anti-pattern the project already solved with testcontainers in camel-test. The original ADR-0054 took this path; review rejected it. |
| **B** | **Do nothing** | Rejected — leaves the WASM CI gap open and any future `#[ignore]` ungoverned. |
| **C** | **Docker Compose for live services** | Rejected — testcontainers already provides superior self-provisioning, and no separate infra file is needed. |
| **D** | **Delete `#[ignore]` from buildable tests** | Rejected — breaks local dev for contributors without the wasm target installed. The annotation serves a legitimate local-dev purpose; the fix is the CI job, not deletion. |
| **E** | **Closed vocabulary with an `// allow-ignore` escape hatch** | Rejected — a closed vocabulary with an escape hatch is not closed. |
| **F** | **Closed vocabulary of two prefixes (`requires pre-built`, `slow test:`) with `requires live` as migration error** | **CHOSEN** — matches the actual categories in the workspace, gives CI a concrete contract, and rejects the anti-pattern. |

## Consequences

- **The ABI/contract break class is closed.** Any future change that breaks a
  buildable WASM test will be caught by the CI job that runs `--ignored` on the
  allowlisted files.
- **External-service tests cannot silently hide as `#[ignore]`.** The lint
  rejects `requires live` and points contributors at `camel-test`.
- **CI gains coverage** for camel-test integration tests (Kafka, Redis,
  OpenSearch, K8s) by wiring them into `full-tests-linux`. This is the existing
  pattern, not new infrastructure.
- **Ollama tests remain `slow test:`** — genuinely special because a 4B model
  pull is gigabyte-scale and minutes long, not cheaply provisionable per-PR.
  A follow-up bd issue will decide whether to move them to a nightly workflow
  or keep them as `slow test:`.
- **Allowlist maintenance burden** for `requires pre-built` is real. The
  bidirectional lint catches stale entries at merge time rather than letting
  them accumulate.
- **No escape hatch means occasional ADR amendments.** If a genuinely new
  `#[ignore]` category emerges, it requires a new ADR. This is intentional; the
  bar for expanding the vocabulary should exceed a single contributor's
  convenience.

### Self-grill record

**Questions generated:**

1. Why testcontainers over docker-compose for external-service tests?
2. Why is Ollama special, given testcontainers solves the same problem for
   Kafka and Redis?
3. Why two prefixes and not just one?
4. Why delete the duplicate inline tests instead of moving them to camel-test?
5. Why not run Ollama tests on a nightly schedule instead of marking them
   `slow test:`?

**Answers (with citations):**

1. **Testcontainers over docker-compose.** `camel-test/Cargo.toml` already
   declares `testcontainers` and `testcontainers-modules` (`kafka`, `redis`,
   `postgres`, `k3s`) as workspace dependencies. The existing Kafka and Redis
   integration tests self-provision via testcontainers with no separate infra
   file. Adding a docker-compose layer would duplicate that mechanism and
   re-introduce a parallel infrastructure definition the project does not
   need.
2. **Ollama is special because of model weights, not service lifecycle.**
   Kafka, Redis, OpenSearch, and K3s are stock images testcontainers pulls and
   starts. Ollama tests need a 4B-parameter model (`qwen3.5:4b`) that must be
   pulled on first use. The pull is gigabyte-scale, takes minutes, and runs
   per container, not per test run. Testcontainers has no provision for that
   class of artifact. The 8 remaining Ollama tests
   (`crates/components/camel-component-llm/tests/ollama_live.rs`) are
   genuinely out of reach of the camel-test pattern.
3. **Two prefixes because they have different enforcement shapes.**
   `requires pre-built` must be coupled to an allowlist so the CI job knows
   which files to build artifacts for and run with `--ignored`. `slow test:`
   is documentary: it makes the performance characteristic visible and
   greppable for a future scheduled job but needs no allowlist because no CI
   job runs it today. Collapsing the two prefixes would either force every
   `slow test:` to add a no-op allowlist entry, or drop the allowlist from
   `requires pre-built`, which is the wrong direction.
4. **Deletion over move.** `camel-test/tests/kafka_test.rs` and
   `camel-test/tests/redis_test.rs` already exercise the same component
   surfaces with full lifecycle coverage (broker provisioning, message
   round-trip, error paths). The inline `#[ignore]` tests in component crates
   were truncated skeletons that asserted only that the constructor
   compiled. Moving them would produce duplicates of already-superior
   coverage. Deletion removes the dead code without losing any signal.
5. **Nightly is a follow-up, not a prerequisite for this decision.** A nightly
   workflow that pulls `qwen3.5:4b` and runs the Ollama suite is a separate
   infrastructure decision. ADR-0054 records the `slow test:` classification
   that makes that future job trivial to implement; whether and when to run
   it belongs to its own ADR or CI change. The current decision is
   self-contained.

**Outcome:** approve as revised ADR-0054. The closed vocabulary drops to two
prefixes, `requires live` is a migration error, the existing camel-test +
testcontainers pattern is the canonical answer for external-service tests, and
the self-grill record grounds the Ollama exception in a specific artifact
(gigabyte-scale model weights) that testcontainers cannot provision.
**Self-grill mode:** self-grill-proposals skill
