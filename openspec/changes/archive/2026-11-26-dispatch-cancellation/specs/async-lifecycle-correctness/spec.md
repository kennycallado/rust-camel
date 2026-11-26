## ADDED Requirements

### Requirement: Per-dispatch cancellation stops an enqueued exchange between steps

The system SHALL provide a per-dispatch cancellation signal for the
request-reply envelope path. `ConsumerContext::send_and_wait` SHALL mint a
fresh, independent cancellation token and stamp it on the Exchange as an
enqueue-only marker; dropping the wait future before the reply SHALL cancel
exactly that dispatch, except when the consumer-lifetime token is already
cancelled — then the drop is shutdown-driven and the existing route-stop drain
grace SHALL govern the exchange, not the dispatch signal. A completed reply
SHALL disarm the signal. Each pipeline entry SHALL take (remove) the marker
before running any step, so replies, stash emissions, and later redispatches
carry no stale signal. The token SHALL be independent of the consumer-lifetime
and route-stop tokens, and the fresh token SHALL be created by the `camel-api`
dispatch helper, not by component code.

The pipeline SHALL observe the signal before each step, beside the route-stop
check, at the entries that already scope route-stop cancellation (the
concurrent envelope loop, the sequential envelope loop, and inline dispatch),
and resolve with `PipelineOutcome::Failed(CamelError::ConsumerStopping)` when a
step is prevented; an exchange with no remaining step SHALL finish with its
last step's outcome. The aggregate-split loop SHALL take (remove) the marker
without observing it, because that loop scopes no route-stop cancellation
either; its replies SHALL still carry no stale signal. A dispatch-cancelled
`ConsumerStopping` SHALL NOT be retried or absorbed by the error handler. A
non-transactional step that is already executing SHALL run to completion —
cancellation SHALL NOT abort a step mid-`.await`; it SHALL prevent the next
step. Route stop SHALL keep its existing drain-grace semantics and SHALL remain
an independent trigger.

For a `transacted` block the new signal joins the existing block-level
cancellation arm with the frozen precedence: block deadline first, then
cancellation, then body. A body cancelled before the commit cutoff SHALL skip
COMMIT and await the session's total rollback within the 5 s finalize grace; an
acknowledged rollback SHALL leave no checked-out session, and an unacknowledged
rollback SHALL take the existing detach path (`RollbackUncertain`/`Detached`)
with the cleanup uncertainty logged and the primary error standing. A body that
completes successfully SHALL consult both tokens only at the existing cutoff:
after COMMIT is handed to the driver, cancellation SHALL NOT be re-consulted
and the commit resolution SHALL stand. No new field SHALL be added to any
published public struct.

#### Scenario: cancelled dispatch stops before the next step

- **GIVEN** a started route whose first step parks until released and whose
  second step records execution, with a dispatcher waiting through
  `send_and_wait`
- **WHEN** the dispatching wait is dropped and the first step is then released
- **THEN** the first step completes, the second step never runs, the exchange
  resolves with `ConsumerStopping`, and the in-flight bookkeeping returns to
  zero

#### Scenario: cancelled dispatch releases the exchange

- **GIVEN** a `Concurrent { max: Some(1) }` route whose single step parks until
  released, and a dispatcher whose wait is dropped while the step is parked
- **WHEN** the parked step is released and a second dispatch is submitted
- **THEN** the dropped dispatch's reply channel is closed, its in-flight claim
  and its concurrency permit are released, the second dispatch completes, and
  neither reply carries a dispatch marker

#### Scenario: route stop still cancels an in-flight dispatch

- **GIVEN** a started `Concurrent` route with two steps whose first step parks,
  and a live `send_and_wait` waiter
- **WHEN** the route is stopped, the drain grace expires so the route-stop token
  fires, and the first step is then released
- **THEN** the waiter resolves with `ConsumerStopping` at the next step
  boundary, the second step never runs, and no dispatch token was involved

#### Scenario: abandoned wait during route stop drains to completion

- **GIVEN** a started Sequential route whose first step parks and whose second
  step records execution, with a live `send_and_wait` waiter and a shutdown
  budget long enough for the drain grace
- **WHEN** the route is stopped, the consumer-lifetime token is cancelled, the
  waiter is then dropped, and the parked step is released
- **THEN** the drop does not cancel the dispatch, the second step runs, the
  exchange drains to completion, and the stop returns success

#### Scenario: cancelled transacted block finalizes without leaking a session

- **GIVEN** a `transacted` block whose body is parked inside a joined step when
  the dispatching wait is dropped, with cancellation arriving before the commit
  cutoff and a session whose rollback is acknowledged
- **WHEN** the block finalizes
- **THEN** COMMIT is never issued, the bounded rollback completes within the
  5 s grace, the session reaches its finalized state, and no checked-out
  session remains

#### Scenario: uncancelled dispatch is unchanged

- **GIVEN** a started route and a dispatcher that awaits a normal
  `send_and_wait` to completion
- **WHEN** every step succeeds and the reply arrives
- **THEN** all steps run, the reply carries the unchanged exchange with no
  dispatch marker, the dispatch signal is disarmed, and no cancellation is
  observed
