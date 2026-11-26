# Design: dispatch-cancellation

## Approach

### Carrier: a token on the Exchange, triggered by wait-future drop

Two carriers were allowed by the mission. Both were probed against the code on
`main` (`e1b81ed86`).

- **A public token field on `ExchangeEnvelope`**
  (`crates/components/camel-component-api/src/consumer.rs:27`) is rejected. The
  struct has all-public fields in a published crate and 50 in-tree
  struct-literal constructors; adding a field is a source-breaking change for
  downstream crates. `#[non_exhaustive]` would be a break too. The mission
  forbids a public API break.
- **A reply-closed observer wired into `run_steps` alone** is rejected. Tokio
  1.54's `oneshot::Sender::closed(&mut self)` is awaitable, but it needs a
  mutable borrow of the sender held by the pipeline task. Reaching it from
  `run_steps` and from the transacted block race means threading `&mut` through
  the compiled pipeline call or sharing the sender behind a lock — neither is a
  step-boundary signal and both tax every step. The token is the same event in
  a shared, awaitable, `Send + Sync` form.

**Chosen carrier:** a `CancellationToken` stamped on the `Exchange` as an
**enqueue-only marker** under `CamelDispatchCancel`, exactly the
`CamelOriginalMessage` extension pattern (`crates/camel-api/src/exchange.rs:15`).
`camel-api` gains an additive RAII helper:

- `Exchange::begin_dispatch_cancel(&mut self) -> DispatchCancellation` mints a
  **fresh independent** token, stamps the marker, and returns a handle holding
  the token and a `drop_guard`.
- `DispatchCancellation::{token, disarm}`: the guard cancels on drop; `disarm`
  defuses it after a successful reply.
- `Exchange::take_dispatch_cancel(&mut self) -> Option<CancellationToken>`
  removes the marker; `Exchange::dispatch_cancel(&self)` is the read-only peek
  used by tests.

The trigger is **wait-future drop**, the only way a dispatcher gives up.
`send_and_wait` calls `begin_dispatch_cancel`, attaches the consumer-lifetime
token as the handle's `drain_owner`, pushes, and disarms on a normal reply. A
drop is therefore shutdown-aware: if the consumer-lifetime token is already
cancelled (JMS, RabbitMQ, SEDA, and Redis stream drop their waits during route
stop), the guard is defused and the existing ADR-0043 drain grace owns the
exchange; only a drop on a live route cancels. The token is never derived from
or linked to another token, and the fresh token is minted in `camel-api`, which
is outside the `crates/components/*/src/**` scope of `xtask lint-cancel-tokens`,
so the ratchet is not raised.

The marker is **taken, not read**, at every pipeline entry:
`let dispatch_cancel = exchange.take_dispatch_cancel();`. That keeps replies,
stash emissions, and any later redispatch free of a stale cancellation. The
inline dispatcher takes the marker too and scopes it explicitly, so an
independent inline entry shadows any outer dispatch scope with `None` instead
of inheriting it.

### Enforcement in camel-core

- `route_compiler.rs`: a second task-local `DISPATCH_CANCEL` (typed
  `Option<CancellationToken>`) is scoped around `pipe.call(exchange)` at the
  three pipeline entry points that already scope route-stop cancellation
  (concurrent envelope task, sequential/default envelope task, inline
  dispatcher). Scoping `None` is allocation-free. The aggregate-split loop in
  `route_controller.rs` (destructure at line 1224) takes and discards the
  marker so its replies carry no stale signal, but does not observe the
  signal: aggregate-split remains out of scope, and one abandoned contributor
  must never cancel work shared by the completed bucket.
- `run_steps`: the boundary check becomes route-stop OR dispatch-cancel, both
  mapping to `PipelineOutcome::Failed(CamelError::ConsumerStopping)` per
  ADR-0043. The check runs **before** each step: if a cancelled dispatch has no
  remaining step, it finishes with the last step's outcome (no terminal check).
  When a step returns `Failed(ConsumerStopping)` and the dispatch token is
  cancelled, `run_steps` returns that failure directly — no error-handler
  retry, no `Handled` absorption. The retry loop is gated too:
  `RetryableStep::dispatch_cancelled` (a defaulted-false method added to
  `camel-api`; overridden by camel-core's `OwnedRetryable`) is read by
  camel-processor between backoff attempts, and a cancelled dispatch returns
  `Exhausted` with `ConsumerStopping`, which `run_steps` propagates before
  `handle_step`. Route-stop handling is untouched: neither gate consults the
  route-stop token.
- `transacted_segment.rs`: the body race keeps the frozen `biased` order
  `deadline → cancellation → body`; the cancellation arm now resolves on either
  token. A body cancelled before the commit cutoff takes `BodyEnd::Cancelled`,
  `finalize` skips the R7 commit gate, and runs the session's total `rollback()`
  bounded by the 5 s finalize grace. If the rollback is acknowledged, the block
  ends finalized with no open session. If it is not acknowledged, the armed
  `DiscardOnDrop` guard detaches the connection and the existing
  `RollbackUncertain`/`Detached` path reports it; the cleanup uncertainty is
  logged without replacing the primary error. Once `commit()` has been handed
  to the driver, cancellation is never re-consulted — the post-cutoff rules
  (`Committed`, `CommitUncertain`, `CommitFailed`) are unchanged.
- Envelope drop sites need no new logic: step 0 is checked too, so an exchange
  cancelled while queued never runs a step.

### Decision: stop between steps, not mid-step drop

The primitive stops **between steps**. ADR-0043 already establishes this
granularity, and it is the only choice that does not silently change step
semantics:

- A plain SQL step (auto-commit, outside a block) that is already executing
  runs to completion; its committed effect persists. Cancellation prevents the
  next step, not the current statement. Dropping the statement mid-flight would
  make commit status unknowable without a transaction to roll back.
- A `transacted` block is the one place where the body is dropped mid-flight,
  and only because finalization owns the session: the block-level race is the
  existing route-stop behaviour, and the new token plugs into the same arm. The
  session is never left open — see the finalization path above.
- Route stop keeps its semantics: it stays on `CANCEL_TOKEN`, keeps the drain
  grace (ADR-0043 amendment), and is an independent check. The dispatch handle
  is shutdown-aware (`drain_owner`), so a component that drops its wait because
  the consumer token fired does not convert a graceful drain into a hard cut.
  Per-dispatch cancel is immediate by design only on a live route: it is the
  dispatcher giving up, not a shutdown. Route suspend cancels the same
  consumer-lifetime token while the pipeline keeps running; a wait dropped in
  that window is stood down too, preserving the existing suspend behaviour.

## Affected crates

- `camel-api`: `CamelDispatchCancel` key, `DispatchCancellation` (fresh token +
  drop guard + `drain_on` + shutdown-aware `disarm`), `Exchange::{begin_dispatch_cancel,
  take_dispatch_cancel, dispatch_cancel}`. Additive; docs state the marker is
  enqueue-only and non-serializable.
- `camel-component-api`: `send_and_wait` begins the dispatch cancellation,
  stamps the exchange, and disarms on success; docs state the cancellation
  contract. No fresh-token literal is added to component code.
- `camel-core`: second task-local, `run_steps` boundary + handler bypass, inline
  scope, `TransactedSegment` race and R6 cutoff, aggregate-loop marker take.
  Tests live beside the existing route-controller tests, plus the segment tests.
- `camel-processor`: the retry loop reads the defaulted
  `RetryableStep::dispatch_cancelled` hook between attempts and stops with
  `ConsumerStopping`; no route-stop token is consulted.

## Architecture boundaries

The runtime (camel-core) owns observation and finalization; the component API
mints and carries an opaque token on the exchange. No pipeline type, step, or
session is exposed to components. The token is ordinary exchange metadata, like
the original-message extension, and does not enter the DSL or the data plane
schema.

## Alternatives considered

- **Bridge task per dispatch** (cancel a child of `pipeline_cancel` when the
  dispatch token fires): works but spawns one task per in-flight request and
  makes token lineage load-bearing. Two independent checks are cheaper and
  satisfy the "route stop unchanged" requirement by construction.
- **New `CamelError::DispatchCancelled` variant**: more truthful naming, but it
  would force a parallel mapping through error handlers, UoW hooks, and the
  transacted segment for no observable consumer benefit; `rc-sqpqn` maps the
  outcome at the MCP boundary anyway.
- **Opt-in `send_and_wait_cancellable` only**: smaller per-call overhead, but
  existing timeout wrappers (the `rc-sqpqn` adoption) would stay unable to
  cancel unless every call site switched. Automatic arming makes the primitive
  correct by default; the disarmed path is behaviourally identical.

No phases: this is one coherent slice with one behavioural crate and two
additive surfaces.
