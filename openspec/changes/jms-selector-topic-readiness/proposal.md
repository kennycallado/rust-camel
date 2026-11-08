# Proposal: jms-selector-topic-readiness

## Why

bd rc-oxj2t (P2, second occurrence): `jms_consumer_selector_filters_activemq_topic`
(`crates/camel-test/tests/jms_test.rs`) times out in CI Full Tests. CI run
42470660723 (`45a76da3b`, 2026-11-08):

```
11:08:56.676  WARN JMS subscribe failed; retrying           (broker=default …)
11:09:01.678  INFO JMS consumer attach requested; awaiting bridge response   (5 s later)
11:09:31      thread 'jms_consumer_selector_filters_activemq_topic' panicked at jms_test.rs:580
              timeout waiting for jms selector delivers the matching topic message after 35s
```

The test starts a topic consumer and two 3-fire timer producers
(`delay=3000&period=1000&repeatCount=3`) in the same `h.start()`. When the
first subscribe fails and retries 5 s later, all six publishes are already
gone. A non-durable topic subscriber has no backlog, so nothing is ever
delivered and the 35 s bounded wait dies with `Last error: <none>`.

## Why does the first subscribe fail (file:line answer)

1. **Slot-Ready does not imply broker-connected.** The pool publishes
   `BridgeState::Ready` after Phase-2 startup health
   (`crates/components/camel-jms/src/component.rs:1776-1790`,
   `start_bridge_process_with_config` → `wait_for_health(..., Ok(resp.healthy))`),
   which gates on `resp.healthy` only — never `resp.broker_connected`
   (the distinction exists: the test helper `await_broker_connected`,
   `jms_test.rs:698-723`, checks both flags). `install_ready_process`
   (`component.rs:480-492`) flips Ready the moment that health passes.
2. **The consumer subscribes immediately at route start.** The shared
   component eagerly pre-creates the slot (`support/jms.rs:29-61`), so the
   consumer route issues `client.subscribe(...)` at
   `crates/components/camel-jms/src/consumer.rs:597` while the bridge's JMS
   connection to the broker may still be dialing. The bridge rejects the
   subscribe asynchronously (the same CI run shows
   `jms_crash_probe: setup failed: … asynchronous subscribe/startup
   rejection` — rejection during startup, not after readiness).
3. **Retry lands after the publish window.** Non-permanent subscribe
   failures warn `"JMS subscribe failed; retrying"` and sleep
   `reconnect.delay_for(attempt - 1)` (`consumer.rs:684-711`); the JMS
   reconnect default is 5 s initial delay
   (`crates/components/camel-jms/src/config.rs:145-154`,
   `default_broker_reconnect_interval_ms() = 5_000` at config.rs:138-140) —
   exactly the 5 s gap in the CI log. Timer producers fire at +3/+4/+5 s,
   so the attach misses every publish.
4. **No harness readiness barrier exists for route-based JMS tests.**
   `await_broker_connected` is used only by direct-gRPC tests
   (`jms_test.rs:901/1000`, `jms_durable_test.rs:311/394`,
   `jms_tls_test.rs:374`, crash-probe cases). The established
   broker-side readiness pattern lives in
   `jms_disposition_cases.rs:114-136` (`wait_consumer_registered`).
   The Jolokia helper hardcodes `destinationType=Queue`
   (`support/activemq.rs:245-271`), and the shared broker container exposes
   no Jolokia port at all (`support/activemq.rs:40-64`).

**Verdict: not a product bug.** The bridge rejects only during startup
(before its broker connection is live) and the component's transient
retry classification is by-design behavior. The defect is the test
publishing before the subscriber exists.

## What Changes

- `jms_consumer_selector_filters_activemq_topic` restructured into two
  harness contexts sharing one dedicated-broker JMS component:
  consumer context starts first, then a broker-side readiness barrier, then
  the producer context. Assertions, selector URIs, timer configs and the
  35 s bounded waits are unchanged.
- **Barrier**: `wait_until` on a Jolokia `ConsumerCount` read with
  `destinationType=Topic` — new additive helper
  `ActiveMqBroker::topic_consumer_count` (`support/activemq.rs`, mirror of
  `consumer_count`; Queue path byte-identical). Broker-side registration is
  the deterministic fact the CI race hinged on.
- **Dedicated broker** (`ActiveMqBroker::start()` + new additive
  `jms_component_for_broker` in `support/jms.rs`, mirroring
  `shared_jms_activemq` minus the OnceCell): broker-side counters must be
  exact (isolation note at `support/activemq.rs:80-87`) and the shared
  container exposes no Jolokia port. Every run now cold-starts its own
  bridge process, so the barrier is genuinely exercised each time.
- One-line comment in the test explaining the barrier (bd rc-oxj2t).
- `rc-qeelq` (Artemis topic variant) checked: still **open**, the variant
  test does not exist → not covered here, no scope expansion.
- One new-site `allow-test-wait` marker on the eagerly-awaited slot start in
  `jms_component_for_broker` (bounded internally by
  `bridge_start_timeout_ms`, mirroring the three `shared_jms_*` sites).

Out of scope (mission non-goals): selector parsing, the JMS component, the
bridge, the retry interval, `#[ignore]`, longer wait budgets,
publish-until-received loops, `support/activemq.rs` changes beyond the
additive Topic capability.

## Local reproduction evidence (runs shown)

- **Forced mechanism reproducer** (temporary test, not committed; documented
  per mission): producers started first (3 fires at +3/+4/+5 s, done by
  +5 s), consumer attached at +7 s → the 20 s bounded wait timed out with
  nothing received — `repro_publish_before_attach_loses_topic_messages … ok`,
  i.e. publishes before attach are lost on a non-durable topic. 1/1 run.
- **Baseline, unfixed test as-is**: 5 runs, 5/5 green (39.27 s / 42.22 s /
  43.97 s / 40.75 s / 40.24 s). The CI timing (first subscribe rejected
  during bridge cold start) did not fire on the warm local machine — the
  forced reproducer above stands in as the mechanism evidence.
- **Post-fix**: 5 runs, 5/5 green (43.23 s / 43.74 s / 38.65 s / 38.74 s /
  41.05 s), each run cold-starting bridge + dedicated broker.
- **Queue sibling** `jms_consumer_selector_filters_activemq_queue`: green
  (37.71 s).

## Acceptance criteria

- The topic selector test starts its producers only after the broker reports
  the topic subscriber (Jolokia `destinationType=Topic` `ConsumerCount >= 1`).
- Matching message received, non-matching never received, final exchange
  assertions unchanged; 35 s bound unchanged; no retries added.
- `jms_consumer_selector_filters_activemq_queue` still green.
- `support/activemq.rs` Queue path byte-identical; Topic path additive.
- `cargo fmt --check --all` green; `cargo clippy -p camel-test
  --all-targets --features integration-tests -- -D warnings` clean on all
  touched targets (pre-existing main-side `sql_test` len_zero failures
  tracked as bd rc-0e0n3).

## Risk budget

Acceptable: one extra ActiveMQ container + one bridge process for this test
(~15 s per run; the suite already runs dedicated-broker tests). Out of
bounds: anything in the non-goals list.
