# Proposal: drainclaim

## Why

`mode: batch` jobs decide "drained" from the seda queue-depth gauge
(`DRAIN_ZERO_SAMPLES_REQUIRED = 10` in
`crates/camel-cli/src/commands/job/batch.rs`). That gauge does not see
route-pipeline residency: when the seda forwarder hands an exchange to the
route's dispatch channel, the gauge honestly reads zero while the exchange
is still moving. A route whose per-hop delay exceeds the 2.5 s quiescence
window (for example `from: seda:a -> delay: 3000 -> to: seda:a`)
deterministically false-completes with outcome `Completed` while work is
still in flight (bd rc-cd9y7).

The root cause: the drain signal observes queue depth only. No signal
observes exchanges that are accepted but not completed.

The mission-144 draft proposed a composite per-route-counter + live-depth
signal; the spec review rejected it (manufactured co-zero: two counters
read together can interleave to a false zero). This change implements the
prescribed redesign instead: one context-global lifecycle counter.

## What Changes

- **camel-component-api**: `ExchangeEnvelope` gains an optional
  `InFlightClaim(Arc<AtomicU64>)` — an RAII handle that increments the
  context-global accepted-not-completed counter at attach and decrements
  exactly once on drop. `ConsumerContext` carries the counter and attaches
  a claim in `send` / `send_and_wait`; `ComponentContext` /
  `RuntimeObservability` gain a default-`None` counter accessor so
  components can mint claims from handles they already hold.
- **camel-core**: `CamelContext` owns the counter and exposes
  `total_in_flight() -> u64` (single atomic load — linearizable by
  construction). The pipeline drain sites take the claim out of every
  dequeued envelope and hold it across processing; the inline dispatcher
  attaches a claim around its direct call. No gauge, `RuntimeQuery`, or
  per-route-counter changes.
- **camel-component-seda**: the producer attaches a claim at enqueue
  (queue residency); fanout splits one claim per subscriber copy.
- **camel-cli**: batch drain polls `total_in_flight() == 0`; the
  sample-count heuristic, streak state, and `BatchDepthProbe` are deleted.
  Documents with auto-firing consumer routes (`timer:`, `cron:`) stay
  rejected at load by the existing fail-closed scheme gate — now pinned as
  a drain-soundness precondition.
- Excluded: the http/grpc/ws/master raw `sender()` fast path stays
  uncounted (documented spec exception, follow-up rc-nftni); hot-reload
  stop-bookkeeping (`drain_in_flight`) is untouched.

## Acceptance criteria

- A batch job whose route self-feeds through delays never reports
  `Completed` while work is in flight; it reports `Timeout` when the cycle
  never settles.
- Settled work completes as soon as one `total_in_flight()` read returns
  zero; no fixed quiescence window is imposed.
- Every release path — normal completion, push failure, queued-envelope
  drop at route stop, task abort, panic, readiness failure — releases each
  claim exactly once.
- Existing job exit codes, reports, signal handling, and shutdown budgets
  are unchanged.

## Risk budget

- Claim pairing must be airtight at every attach boundary. A leaked
  increment degrades a batch job to `Timeout`, never to a false
  `Completed`.
- The old heuristic is removed, not kept as a fallback.
- No new metric labels or emissions (ADR-0012 surface unchanged).

Bd: rc-cd9y7
