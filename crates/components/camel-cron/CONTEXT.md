# cron Component

Calendar-triggered scheduling for rust-camel Routes. Fires Exchanges on Unix
5-field cron schedules, backed by a pluggable `CronService` SPI.

## URI

```
cron:<name>?schedule=<5-field-expr>[&timeZone=<IANA>&includeMetadata=true]
```

## Day-of-week semantics

The `schedule` parameter takes Unix day-of-week numbers. `0` and `7` mean
Sunday. `1` means Monday. `6` means Saturday.

Config normalization translates the Unix day-of-week field to the ordinals of
the `cron` crate, where Sunday is `1` and Saturday is `7`. A numeric value maps
with `v % 7 + 1`. A range, list, or step expands over the source values first,
then each value maps. A single value with a step expands from that value to the
field maximum 7. `*`, stepped wildcards, and day names pass through
unchanged because both spaces agree on them.

Endpoint creation rejects four forms: a value outside `0-7`, a descending
range, a zero step, and a mixed numeric/name endpoint.

Scope note: the `cron` crate intersects a restricted day-of-month field with a
restricted day-of-week field. Unix cron takes the union (OR) instead. An
expression that restricts both fields can therefore fire on fewer days than a
Unix cron implementation would.

## Language

**CronSchedule**:
Validated cron trigger handed to a `CronService`: expression, timezone,
trigger identity, and owning route. Lives in `camel-component-api`.
_Avoid_: cron config, trigger config

**CronService**:
SPI trait for cron scheduling backends. The service decides *when* to fire;
the `CronConsumer`'s callback submits the Exchange. Default: `TokioCronService`.
Lives in `camel-component-api`.
_Avoid_: scheduler, cron engine, timer service

**CronFire**:
Metadata passed to the callback on each fire: `scheduled_at`, `fired_at`,
`counter`. `scheduled_at` differs from `fired_at` under load.
_Avoid_: tick event, fire event

**CronCallback**:
Async, fallible closure built by `CronConsumer` from its `ConsumerContext`.
Calls `context.send(exchange).await`. On `Err`, the error propagates to Route
supervision (ADR-0007).
_Avoid_: handler, listener

**Misfire-skip**:
When the process was down during a scheduled fire, the missed fire is NOT
replayed. The next fire is computed from `now()`. Rationale (D4): automatic
catch-up of batch jobs is dangerous.
_Avoid_: catch-up, fire-once-now, misfire policy

## Relationships

- Implements `camel_component_api::CronService` (SPI in component-api)
- Mirrors `camel-timer` structure (Component → Endpoint → Consumer)
- `CronConsumer` delegates to `Arc<dyn CronService>` — scheduling is pluggable
- Error propagation follows ADR-0007 (Route-supervised Consumer failure)
