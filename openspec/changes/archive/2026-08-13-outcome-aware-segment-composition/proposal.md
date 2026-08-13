# Proposal: outcome-aware-segment-composition

## Why

A demo surfaced three defects that share one missing contract: outcome-aware
Segments (EIPs implementing `OutcomePipeline`, governed by ADR-0025) have no
stated invariant for what they may report as `Completed` after their work
failed. The result is silent data corruption with TTL persistence:

- **rc-20yn (P0, keystone):** `recipient_list` whose recipients ALL fail returns
  `Ok(original)` (the unchanged inbound exchange). When used inside `cache:`'s
  `on_miss`, the adapter wraps that into `Completed(original)` and the cache
  writes the inbound body back under the key for the full TTL — silently, at no
  log level. On `timer:` routes this is invisible; on `http:` routes the
  double-consume of `Body::Stream` masks it as rc-n8rc.
- **rc-n8rc (P1):** `Body::Stream` consumed on the error path (HTTP symptom-masker
  of rc-20yn). Fixing rc-n8rc alone would make the HTTP case ALSO silently cache
  the inbound body.
- **rc-65yi (P1):** body lost when `cache_peek_stale` runs inside a `do_try`
  catch that shares a key with a `cache:` step — empty 200, no log.

The unifying invariant the codebase lacks, and which all three violate:

> **When a Segment's attempted work results in zero successes (an operational
> failure), the Segment SHALL report `Failed(error)` — or `Stopped(exchange)`
> only for an intentional halt. It SHALL NOT report `Completed`.**

## What Changes

**Included (one atomic change):**
- ADR-0058 pinning the composition invariant + cache write-back trust rule.
- `recipient_list` all-failed semantics: when zero recipients succeeded AND at
  least one errored, return `Err(last_error)` (not `Ok(original)`). The existing
  `Result → PipelineOutcome` adapter then yields `Failed`, and `cache:` already
  skips write-back on `Failed` (`cache_eip.rs` step 3).
- Preserve: empty-recipient-expression no-op (`Ok(original)` is legitimate —
  nothing was attempted), partial-success aggregation (some recipients errored
  but at least one succeeded), `stop_on_exception` default `false` (Apache Camel
  parity).
- rc-n8rc + rc-65yi verification against the landed invariant; fix only where
  the reproducer proves necessary.
- Composed-path integration tests (rc-fgcu): `timer → cache → recipient_list`
  poison; `do_try / cache_peek_stale` stale-serve.

**Explicitly excluded:**
- Changing `stop_on_exception` default (breaks Apache Camel parity).
- Namespace-on-intake header redesign (rc-t6eq backlog, epic rc-vy6w).
- `camel-http` producer/consumer header policy (ADR-0057, owned by rc-eoft,
  epic rc-vy6w).

**Affected crates:** `camel-processor` (recipient_list, cache, do_try),
`camel-api` (PipelineOutcome contract doc), `camel-test` (integration tests),
`docs/adr/` (ADR-0058).

## Acceptance criteria

- A timer-driven `cache:{on_miss:[recipient_list:{last_wins}]}` whose single
  recipient 4xx-fails does NOT cache the inbound body and reports `Failed` (not
  `Completed`) through the on_miss boundary.
- Unit test: `RecipientListService` with all-recipients-failed +
  `stop_on_exception=false` returns `Err(last_error)` (zero successes), not
  `Ok(original)`.
- `cache_peek_stale` read-back after a failed warm returns the previously seeded
  value, never the timer/inbound string.
- rc-n8rc reproducer: a recipient returning 403 with a streaming inbound body
  does NOT emit `Body::Stream already consumed`; the error reply reaches the
  caller with correct status/body.
- rc-65yi reproducer: `cache:{key:k, on_miss:[do_try:{…catch:[cache_peek_stale:{key:k}]}]}`
  serves the stale body (200 + stale body), not an empty 200.
- ADR-0058 exists under `docs/adr/`, states the invariant, enumerates governed
  Segments, specifies the cache write-back trust rule; rc-20yn's fix cites it.
- A reviewer can determine, from ADR-0058 alone, whether a new Segment
  implementation is compliant.

## Risk budget

- **In bounds:** change `recipient_list` all-failed return value; add a new ADR;
  add composed-path integration tests; targeted fixes to rc-n8rc/rc-65yi only
  where the reproducer proves necessary.
- **Out of bounds:** any change to `stop_on_exception` default; any change to
  the `PipelineOutcome` enum shape (ADR-0025 stable); partial-success behavior;
  legitimate no-op `Completed(original)` when no work was attempted.

Bd: rc-65fs (epic). Children: rc-20yn, rc-n8rc, rc-65yi, rc-yy74 (ADR-0058),
rc-fgcu (integration tests). ADR-0058 number reserved via rc-zfov.
