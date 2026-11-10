# Design: retry-after-cap

## Approach

### 1. Cap formula (the sleep bound)

`run_with_retry` gains one parameter: `total: Option<Duration>` — the same deadline the
call site already passes to `apply_timeout`. All three call sites (materialized chat,
embed, `llm:decide`) have it in scope. The function records
`let start = tokio::time::Instant::now()` at loop entry (tokio's Instant follows the
paused test clock; `std::time::Instant` does not).

On the `Retry-After` path only:

```text
remaining = total.map(|t| t.saturating_sub(start.elapsed()))
cap       = min(policy.max_delay, remaining.unwrap_or(policy.max_delay))
```

So: with no total configured, `cap = policy.max_delay`; with a total configured,
`cap = min(policy.max_delay, remaining)`.

Decision rules, in order:

1. **Budget spent** (`remaining == Some(0)`, including a sleep that just ended at the
   deadline): return `Err(LlmError::Timeout(total))` without launching another attempt.
   This is required for determinism: `apply_timeout` polls the inner future first, so if
   the inner future returned a rate-limit error at the boundary, that error would win —
   the loop converts a spent budget into `Timeout(total)` itself. Tokio
   `tokio::time::timeout` semantics: `Err(_)` only when the future is pending at the
   deadline; a ready inner result is preserved.
2. **Zero cap, positive header** (`max_delay == 0`, header > 0): return
   `RetryAfterCapped { requested, capped_to: 0 }` immediately — no sleep, no further
   attempt. Without this, `max_attempts = 0` (unlimited) plus a zero cap would
   busy-spin on a truncated long wait.
3. **Header at or under the cap** (equality included; a `Retry-After: 0` header under a
   zero cap lands here): sleep the header value as-is (`Duration::ZERO` sleeps nothing),
   carry the original `RateLimit`, no annotation. Exponential backoff (`delay_for`) is
   untouched — the mission forbids changing the sequence for headers under the cap, and
   backoff delays are not re-capped by the remaining budget; the outer `apply_timeout`
   keeps cutting over-deadline backoff sleeps short, as today.
4. **Header above the cap** (cap > 0): log a warn naming both values, sleep `cap`, and
   carry `RetryAfterCapped { requested, capped_to }` as the error exhaustion surfaces.
   `capped_to` is the calculated bound, not proof of a completed sleep.
5. **Post-sleep budget check**: after any capped sleep, if `elapsed >= total`, return
   `Timeout(total)` — a sleep ending exactly at the deadline launches no further
   request.

Attempt/sleep accounting: N attempts produce at most N−1 sleeps.

Justification for `max_delay` as the primary bound: it is operator-configurable per
provider (`network_retry.max_delay_ms`, default 30 s), consistent with the backoff
clamp, and inside provider norms (OpenAI/Anthropic guidance: seconds to minutes). An
operator who raises it consciously accepts longer waits; the honest error tells them the
header was not honoured. The 120 s decide default is justified as an acceptable
configurable-latency budget — not as a guarantee of a fixed attempt count.

### 2. Honest error

New variant in `LlmError` (enum is `#[non_exhaustive]`; adding variants is the
established additive pattern):

```rust
/// Rate limited; the provider-requested wait exceeded the sleep cap.
/// `capped_to` is the calculated bound. Retryable.
#[error("rate limited; retry after {requested:?} exceeds the {capped_to:?} cap")]
RetryAfterCapped { requested: Duration, capped_to: Duration },
```

Display uses `{:?}` (Debug) so subsecond caps keep precision: `750ms` renders as
`750ms`, not `0s`. Contract:

- Added to `is_retryable`. A capped rate limit is still a rate limit; semantics
  otherwise unchanged.
- Carried only for the attempt boundary that produced it: the loop binds a fresh
  result each iteration; the capped error is derived fresh at the exhaustion return
  (`honest_rate_limit`), never carried across attempts.
- Normalization applies at every post-policy return point: the exhaustion return
  happens BEFORE delay computation today (`!should_retry` returns early), so the
  exhaustion arm checks the same eligibility condition — a `RateLimit` with a header
  above the cap is surfaced as `RetryAfterCapped` even when no sleep follows. Returns
  before the policy check (policy `None`), content-started returns, and streaming
  returns keep the raw error, unchanged.

### 3. Decide path deadline

`DecisionEndpoint::create_producer` resolves the total today as
`effective_timeout_secs(provider, global) -> Option<Duration>` and passes `None` through
when nothing is configured. Change it to:

```rust
effective_timeout_secs(provider_timeout, global_timeout)
    .map(Duration::from_secs)
    .unwrap_or(Duration::from_secs(DEFAULT_DECIDE_TOTAL_TIMEOUT_SECS))
```

with `DEFAULT_DECIDE_TOTAL_TIMEOUT_SECS: u64 = 120` in `decision/config.rs`, next to
`DEFAULT_ATTEMPT_TIMEOUT_SECS = 30` (the adapters' existing per-request HTTP bound).
The 120 s default is an acceptable configurable-latency budget: an operator can wait
two minutes for a decision, but a provider cannot hold a route task forever. It is not
a promise of a fixed attempt count. Decision (vs documenting a hard requirement): a
default cannot be forgotten; a documented requirement can. The adapters' 30 s
per-request bound stays — a transport bound, not a nested timeout — and its value is
unchanged (ADR-0021 addendum pattern: connect/read-idle guards are bounds, not
lifetimes).

### 4. Clock seam for tests

`MockProvider` sleeps through `tokio::time::sleep`, so
`#[tokio::test(start_paused = true)]` works unchanged; the crate already uses this
pattern (`provider/genai_adapter_tests.rs`) and `camel-cli` job tests use it too.
Decision-producer deadline tests use a custom `DecisionProvider` whose `decide` future
never resolves — no HTTP attempt, so the adapters' 30 s per-request bound cannot fire
and the total is isolated. No new seam, no new dependency.

## Affected crates

- `camel-component-llm`: `producer.rs` (cap + honest error + `total` parameter),
  `error.rs` (new variant + `is_retryable`), `decision/config.rs` (constant),
  `decision/endpoint.rs` (default total), docs `docs/src/components/llm.md`, specs
  `openspec/specs/llm-provider-adapter/spec.md`, `openspec/specs/llm-decisions/spec.md`,
  ADR-0021 addendum.

## Architecture boundaries

Components tier only. No Runtime, DSL, Services, Languages, or Functions changes.
`NetworkRetryPolicy` (camel-component-api) is consumed, not modified — the cap reads
its existing `max_delay`. ADR-0021 owns the manual-loop composition; the addendum
records the cap as part of the loop's bound set. ADR-0013's backoff clamp stays the
source of `max_delay` semantics.

## Alternatives considered

- **Fixed hardcoded ceiling (e.g. 60 s constant)**: rejected as primary — `max_delay`
  already exists, is configured, and per-provider; a second constant would be a second
  opinion fighting the operator's config. The mission allows it only if no configured
  maximum exists; it does.
- **Cap backoff sleeps at the remaining deadline too**: rejected — observable behavior
  already converges (outer deadline fires `Timeout` mid-sleep); capping backoff would
  add a wasted attempt with zero budget left.
- **Fail fast on `retry_after > cap`**: rejected — the mission requires a capped rate
  limit to stay retryable; fail-fast would also convert a transient condition into a
  route failure.
- **Default deadline for chat/embed too**: out of scope — long generations are
  legitimate; the read-idle guard bounds inactivity only (a trickling peer is not
  bounded — ADR-0021 records that residual and its follow-ups). The mission names the
  decide path.
