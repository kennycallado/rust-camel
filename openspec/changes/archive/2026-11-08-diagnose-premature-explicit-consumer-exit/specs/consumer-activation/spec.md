## ADDED Requirements

### Requirement: Explicit no-handle completion is diagnosed before cancellation

When an Explicit consumer's `start()` returns `Ok(())` and `background_task_handle()` returns `None` while its consumer context is not cancelled, camel-core SHALL emit one route-scoped warning explaining that `stop()` follows immediately and that the consumer can keep `start()` alive until cancellation or return a background handle. The warning SHALL include whether the defensive readiness fallback resolved readiness. It SHALL apply to initial start and resume, including consumers that already called `mark_ready()`.

The Runtime SHALL retain the idempotent no-handle readiness fallback and existing cleanup and supervision behavior. This warning SHALL NOT reject startup or publish a route failure. Successful finite completion without cancellation remains permitted and is diagnosed by the same warning.

#### Scenario: Ready consumer returns without a handle

- **GIVEN** an Explicit consumer that calls `mark_ready()` and returns successfully before cancellation
- **WHEN** it supplies no background handle on initial start or resume
- **THEN** the Runtime emits one warning with its route ID and `readiness_fallback = false` before calling `stop()`
- **AND** startup remains successful

#### Scenario: Consumer returns without signalling readiness

- **GIVEN** an Explicit consumer that returns successfully before cancellation without calling `mark_ready()`
- **WHEN** it supplies no background handle on initial start or resume
- **THEN** the Runtime applies the readiness fallback and emits one warning with `readiness_fallback = true` before calling `stop()`
- **AND** startup remains successful

#### Scenario: Cancellation owns no-handle completion

- **GIVEN** an Explicit consumer whose context is cancelled before `start()` returns successfully
- **WHEN** it supplies no background handle
- **THEN** cleanup runs without the no-handle completion warning

#### Scenario: Background handle owns the remaining lifetime

- **GIVEN** an Explicit consumer that signals readiness and returns a background handle
- **WHEN** `start()` returns successfully
- **THEN** the Runtime retains background-task monitoring without the no-handle completion warning

#### Scenario: Immediate completion stays unchanged

- **GIVEN** an Immediate consumer that returns successfully without a background handle before cancellation
- **WHEN** the Runtime performs cleanup
- **THEN** it does not emit the Explicit no-handle completion warning
