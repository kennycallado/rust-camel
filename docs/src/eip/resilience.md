# Resilience and control

Resilience and control patterns protect a route from failure, limit throughput, and scope error handling. They map to the *System Management* family in Hohpe and Woolf, adapted to the Tower middleware pipeline.

- [Circuit Breaker](circuit-breaker.md) — trip after repeated failures and recover after a cool-down
- [Do Try](do-try.md) — scoped catch and finally blocks around a group of steps
- [Throttler](throttler.md) — cap the rate of exchanges through a route
- [Idempotent Consumer](idempotent-consumer.md) — reject duplicates by correlation key
- [Delayer](delayer.md) — hold an exchange for a fixed duration
- [Loop](loop.md) — repeat a sub-route a fixed number of times
- [Validator](validator.md) — check exchange content against a schema

For the error-handling contract that governs catch and finally behavior, see [error handling](../concepts/error-handling.md).
