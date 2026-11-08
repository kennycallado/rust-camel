# Proposal: diagnose-premature-explicit-consumer-exit

## Why

An Explicit consumer can call `mark_ready()`, return `Ok(())` without a background handle, and immediately be stopped by the runtime without any warning. The existing fallback warns only when readiness was never signalled. This hid the lifecycle mistake observed in RabbitMQ (rc-2vgej). Issue: rc-4uuy8.

## What Changes

- In camel-core, warn when an Explicit consumer returns successfully without a background handle before cancellation, whether or not it already signalled readiness.
- Explain that `stop()` follows immediately. Consumers intended to remain active must keep `start()` running or return a background handle; finite completion remains permitted.
- Clarify this existing lifetime contract in camel-component-api and its CONTEXT.md.
- Add deterministic coverage for readiness, cancellation, background-handle, and Immediate cases. Preserve cleanup, readiness fallback, and failure-publication behavior.

## Acceptance criteria

- A ready Explicit consumer returning without a handle before cancellation emits a route-scoped warning before cleanup.
- Missing readiness still receives the defensive fallback and an actionable diagnostic.
- Cancellation-owned no-handle completion, Immediate completion, and Explicit consumers returning a handle do not emit the new warning.
- Existing outer-task supervision and stop behavior remain intact.

## Risk budget

This change adds diagnostics only. Naturally finite Explicit consumers completing before cancellation can also receive the warning; the message describes the observed lifetime rather than claiming a crash. No new runtime failure, startup rejection, waiting policy, API signature, dependency, or component-specific workaround is introduced.
