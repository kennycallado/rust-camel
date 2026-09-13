# camel-timer

Timer component for rust-camel — fires Exchanges on a configurable period, initial delay, and repeat-count schedule. Consumer-only: `timer:name` endpoints create a Consumer; producer creation is rejected.

## Language

**TimerComponent**:
Component for `timer:name` URIs; creates `TimerEndpoint` values from parsed `TimerConfig`.
_Avoid_: scheduler, cron component

**TimerConfig**:
URI-deserialized configuration for `timer:` endpoints. Holds the timer name, period, initial delay, repeat count, fixed-rate flag, and metadata flag. `TimerConfig::validate` rejects an empty name and a zero period before any exchange is produced.
_Avoid_: timer settings, timer options

**TimerEndpoint**:
Endpoint for `timer:name` URIs; creates the `TimerConsumer`. `create_producer` returns `CamelError::EndpointCreationFailed` — the timer is consumer-only.
_Avoid_: timer source, tick endpoint

**TimerConsumer**:
Event-driven Consumer that fires one Exchange per tick. `start` applies the cancellable initial delay, then loops on a tokio interval; `fixedRate=true` uses `MissedTickBehavior::Skip`, otherwise `Burst`. Each tick builds an Exchange with body `timer://<name> tick #<count>` and, when `includeMetadata=true`, the `CamelTimerName`, `CamelTimerCounter`, `CamelTimerFiredTime`, and `CamelMessageTimestamp` headers. A double-start is rejected (TIMER-003); `repeatCount=0` fires zero times; an omitted `repeatCount` fires until cancellation.
_Avoid_: ticker, periodic task

## Log-level policy

Per ADR-0012.

**Outside-contract metric:**
- `b-prime:timer:fire-send` (`fn start` in `impl Consumer for TimerConsumer`, `src/lib.rs:299`): a tick's `context.send(exchange)` failed (route channel closed). The consumer increments the metric and breaks the loop — no log, the metric is the only signal (category b′ per ADR-0012: locally terminal fire send).
