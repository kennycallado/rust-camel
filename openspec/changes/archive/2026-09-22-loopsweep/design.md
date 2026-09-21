# Design: loopsweep

## Approach

Three mechanical conversion patterns, applied per site shape, plus the
ADR-0069 R1 marker for spawned service loops. **Exact per-site
assignment (37 total — 23 P1 + 9 P3 + 5 M):**

- **P1 — enclosing timeout (23 sites).** Readiness/sleep-poll loops and
  wait-for-completion-marker loops; today's shape is either
  `loop { if ready().await { break; } assert!(Instant::now() < deadline);
  sleep(poll).await; }` (the assert is unreachable when `ready().await`
  parks forever), a bare sleep-poll with no deadline at all, or a
  channel loop whose completion marker may never arrive (timeout expiry
  = failure, loud). Convert to
  `tokio::time::timeout(budget, async { loop { .. } }).await.expect("<what was waited for> within <budget>")`;
  `budget` derives from the site's existing deadline constant plus a
  margin (fresh polls: 30× poll interval, floor 10s for I/O readiness).
  Sites:
  - camel-core `route_controller_tests.rs:4021`,
    `runtime_consistency_test.rs:300`, `:357`
  - camel-integration-test `http_partner_test.rs:504` (request-head
    read-until-complete in the spawned partner),
    `log_assertion_test.rs:232`
  - camel-test `redis_repositories_test.rs:1332`,
    `redis_sentinel_test.rs:508`, `:577`, `:650`, `seda_test.rs:305`
  - camel-component-seda `src/lib.rs:3008`
  - camel-component-grpc `tests/server_auth_test.rs:569` — pipeline
    stand-in waits for the `CamelGrpcClientStreamComplete` marker;
    a per-iteration `Err(_) => break` exit would silently pass the test
    without the completion exchange, so the enclosing-timeout form
    preserves fail-loud completion semantics
  - camel-direct `direct_tests.rs:555`, `:1158`
  - camel-jms `component.rs:1781`, `:2173`
  - camel-master `acquisition_budget.rs:65`, `delegate_errors.rs:39`,
    `leadership_state_gauge.rs:336`, `stale_stamp.rs:74`, `:213`
  - camel-redis `topology_tests.rs:709`
  - camel-template `tests/common/mod.rs:166`
- **P2 — per-iteration timeout (0 sites).** The lint-blessed drain shape
  (`match tokio::time::timeout(d, rx.recv()).await { Ok(v) => ..,
  Err(_) => break }`) remains a legitimate bounding form for true
  quiescence drains, but no site in this set qualifies — the sole
  channel loop (grpc, above) requires fail-loud expiry.
- **P3 — bounded-retry wrap (9 sites).** Loops whose termination is
  `policy.should_retry(attempt)` keep their retry logic and gain an
  enclosing overall `tokio::time::timeout` budget with loud expect, so a
  policy regression (infinite attempts, huge delay) fails the test
  instead of hanging it. Sites:
  `camel-container src/lib.rs:2668`, `camel-jms src/consumer.rs:898`,
  `camel-kafka src/consumer.rs:1437`, `camel-sql src/consumer.rs:1678`,
  `src/producer.rs:1298`, `camel-ws src/lib.rs:4999`, `:5041`,
  `camel-xj src/component.rs:604`, `camel-xslt src/component.rs:356`.
- **M — allow-test-wait marker (5 sites).** Spawned accept-loop test
  servers (`tokio::spawn(async { loop { accept().await .. } })`): the
  server must run until teardown, so any deadline kills it mid-test —
  the ADR-0069 R1 service-loop exception (test spawns it, owns the
  handle/listener, bounds client-side readiness assertions; teardown is
  bounded by test-runtime drop). Marker text:
  `// allow-test-wait: spawned test-server accept loop; teardown-bounded by test runtime (ADR-0069 §13.2 R1)`.
  Sites: benchmarks/loadgen `cli_runtime.rs:810`, `:874`; camel-test
  `component_emission_test.rs:663`, `http_test.rs:587`; camel-redis
  `src/executor.rs:880`.

Budget sizing: prefer the site's existing deadline constant; else 30× the
poll interval; never below 10s for I/O-bound readiness. Generous over
flake-inducing.

Ratchet arithmetic: converted and marked sites both leave the count, so
the new ceiling is exactly **548 − 37 = 511** (no double subtraction).
After all conversions, rerun the lint, write 511 to
`scripts/xtask/ratchet-unbounded-wait.max`, and prune the 37 entries
from the embedded inventory comment block. The ceiling may only
decrease.

## Affected crates

Per-crate site counts (sum = 37):

- camel-core: 3 (route_controller_tests, runtime_consistency ×2)
- camel-test: 7 (component_emission, http, redis_repositories,
  redis_sentinel ×3, seda integration tests)
- camel-master: 5 (acquisition_budget, delegate_errors,
  leadership_state_gauge, stale_stamp ×2)
- camel-jms: 3 (component ×2, consumer)
- camel-integration-test: 2 (http_partner, log_assertion)
- camel-direct: 2, camel-redis: 2 (executor, topology_tests),
  camel-sql: 2 (consumer, producer), camel-ws: 2,
  benchmarks/harness/loadgen: 2 (cli_runtime ×2)
- camel-container: 1, camel-component-grpc: 1 (server_auth),
  camel-component-seda: 1, camel-kafka: 1 (consumer),
  camel-template: 1 (tests/common), camel-xj: 1, camel-xslt: 1
- scripts/xtask: ratchet file only (ceiling + inventory), no code change

## Architecture boundaries

Test bodies and one ratchet data file only. No runtime crate changes, no
public API, no DSL/Components/Services boundary crossings. Hexagonal
architecture untouched; `cargo build --workspace` output is bit-identical
for non-test profiles.

## Phases

### Phase 1: Convert all 37 loop sites per-shape
- **Goal:** every flagged loop is deadline-bounded or carries a justified
  marker; affected tests compile and locally-runnable ones pass.
- **Dependencies:** lint inventory at 9ad4290f; P1/P2/P3 patterns above.
- **Externally-visible types/interfaces:** none.
- **Deliverable:** per-crate-group commits on feature/loopsweep.
- **Exit-criteria:** `cargo test -p <crate>` green for each touched crate
  (runnable subset); `cargo clippy -p <crate> -- -D warnings` clean;
  fmt clean.

### Phase 2: Lower the ratchet and verify
- **Goal:** ceiling reflects the burn-down; gates green.
- **Dependencies:** Phase 1 complete.
- **Externally-visible types/interfaces:** ratchet ceiling 548 → 511.
- **Deliverable:** ratchet-unbounded-wait.max updated + inventory pruned.
- **Exit-criteria:** `xtask lint-unbounded-wait` OK at ceiling 511;
  fmt + clippy (affected crates) + relevant xtask lints green.

## Alternatives considered

- **Bounded retry counters instead of timeouts** (rewrite loops as `for _
  in 0..N`): rejected — hides the real wait semantics, changes what the
  test asserts, and the lint does not recognize counter bounds.
- **Paused-clock (`start_paused`) everywhere** (httpsweep 04b8fc9f
  pattern): rejected as default — these loops await real I/O (Redis,
  brokers, processes); auto-advance would stall. Applicable only where a
  loop is pure sleep/backoff, which none of the 37 are.
- **Marker everything**: rejected — the mission requires conversion first;
  markers are the rare exception, not the tool.
