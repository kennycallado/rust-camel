# Design: sedaretry

## Approach

Follow the rc-3px7o typed-provenance doctrine exactly as the
no-active-consumers gate does it (seda lib.rs, `NoActiveConsumersGate`):

1. New crate-private marker `TerminalConfigError` (enum, first variant
   `MultipleConsumersWaitConflict`). `Display` is deliberately
   non-canonical diagnostic text; it never equals a canonical message
   and never participates in classification.
2. New in-crate constructor `terminal_config_rejection()` building
   `CamelError::EndpointCreationFailedWithSource(detail, marker)` — the
   ONLY way this rejection is built. Outer detail stays byte-identical
   to today's wording (`multipleConsumers=true with waitForTaskToComplete
   != Never is not supported — a single request cannot have N valid
   replies without aggregator semantics`), so job reports and the
   pinned seda wording test are unchanged.
3. New pub predicate `is_seda_terminal_config_error(&CamelError) ->
   bool`: same bounded walk as `gate_from_error` (8 source hops,
   `unwrap_arc_dyn_error` for std's `Arc<dyn Error>` wrapper).
4. `is_direct_startup_race` narrows its `EndpointCreationFailedWithSource`
   arm to `!(is_no_active_consumers_gate(err) ||
   is_seda_terminal_config_error(err))`. The plain
   `EndpointCreationFailed(_)` arm is UNTOUCHED — the direct
   registration race and the queue-full residual keep their variant.
5. The reject site moves: `SedaProducer::call`'s multipleConsumers+wait
   check (currently a plain `EndpointCreationFailed` string literal)
   returns the typed rejection. The check fires only with an ACTIVE
   consumer (the gate check precedes it), i.e. fanout endpoints with a
   started consumer.

`camel-cli` needs no behavior change: `is_retryable_startup_failure`
delegates to `is_direct_startup_race`; the narrowed classification
flows through. Doc comments updated to record the new exclusion.

## Affected crates

- `camel-component-seda`: marker, constructor, predicate, classifier
  arm, reject-site conversion, unit tests.
- `camel-cli`: doc comments; classification characterization tests
  (behavioral config-reject fail-fast, foreign-imitation retryability);
  subprocess e2e regression (exit 1, `Failed`, pinned text, elapsed far
  below the 3 s window).

## Architecture boundaries

Components own their classification (camel-component-seda owns the
wording, the marker, and both predicates — no CALLER-side Display
sniffing; rc-fr20u). camel-api's `EndpointCreationFailedWithSource`
(see `error-taxonomy` spec: source-preserving variant) is the carrier;
`OpaqueErrorSource` keeps the marker unforgeable outside the crate.
camel-cli's job loop consumes only the pub predicates. No Runtime,
DSL, Services, or Functions boundary is crossed (ADR-0012 error family
unchanged: same `e:seda:produce` family, same Display). ADR-0024
verdict fidelity: outcome stays `Failed`, exit 1 (2 > 1 > 0 map).

## Alternatives considered

- **Fix `seda_send_uri` to skip the Always injection when
  multipleConsumers=true** (bd's second option): rejected — the
  synchronous-verdict rule is a pinned cli-jobs requirement
  ("seda send is synchronous"); skipping injection would silently
  degrade fanout sends to fire-and-forget and change report outcomes.
- **String-matching the config wording in the classifier**: rejected —
  violates the typed-provenance doctrine (foreign imitations would
  fail-fast wrongly).
- **Generic `is_terminal_config_error` across all components**: out of
  scope — each component must own its marker; foreign components keep
  the retryable default until they add one.

## Sibling inventory (mission scope item)

- Queue-full, bounded enqueue/fanout timeout (`EndpointCreationFailed`,
  plain): stay retryable — they can clear; documented rc-ucemm residual.
- Send loop OUTER transport arm (`attempt_send`'s `Err(String)`: component
  not registered, endpoint-creation failures incl. seda's
  `is_compatible_with` incompatibility): retries deterministically for
  the full window and never consults any classifier — DIFFERENT seam
  (error erased to String); file follow-up bd, not fixed here.
- Consumer-start errors ("already has a registered consumer" etc.):
  unreachable from the send loop (fire at route boot). Unchanged.
- Auth/Keycloak: endpoint and role configuration errors
  (`camel-component-keycloak` `create_endpoint`/`create_consumer`/
  `create_producer` — HTTP client build, URL validation, host
  resolution, admin/events role mismatch) DO ride
  `EndpointCreationFailed` and are deterministic, but they surface
  through `attempt_send`'s outer transport arm (erased to String
  before any classifier) — covered by the outer-arm follow-up bd.
  Runtime admin-auth failures use `ProcessorError` — already
  non-retryable at the inner classifier.
