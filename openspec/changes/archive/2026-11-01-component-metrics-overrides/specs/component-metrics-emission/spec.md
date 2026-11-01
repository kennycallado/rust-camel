## ADDED Requirements

### Requirement: HTTP consumer operations are observed

The HTTP component SHALL emit
`camel_component_operations_total{component="http",operation="consume"}`
once per handled inbound request on a `from: http:` route, with outcome
`success` or `failure`. The outcome SHALL be decided at response
completion or abort, not at envelope handoff to the route channel, and
SHALL be recorded exactly once per request:

- success is a reply not classified as failed whose response body is
  exhausted under the HTTP body contract; a body that is
  end-of-stream at construction, or a response whose body is
  suppressed by protocol (`HEAD`, `204`, `304`, `1xx`), counts as
  exhausted;
- failure is a reply classified as failed by the consumer (pipeline
  error, authentication denial, cancellation reply, unconsumable
  response body), a response body error, a response body dropped
  before exhaustion, a reply whose headers cannot build a valid
  response, the dispatcher's oversized-response replacement, or a
  request that never produces a completed
  route round-trip (body over the limit, inflight exhaustion, route
  channel closed, reply channel closed, handler cancellation while
  awaiting, including the dispatch timeout);
- a classified failed reply keeps its failure classification through
  body finalization: exhausting or dropping its error-rendering body
  SHALL NOT turn it into a success;
- a route-selected HTTP status code (including 4xx/5xx chosen by the
  route) SHALL NOT by itself be a failure: the route completed and
  produced a well-formed response;
- completion is observed through the HTTP body contract. A transport
  write error that occurs after the body is exhausted is outside this
  requirement.

Failures SHALL also forward the non-disableable error family as
`e:http:consume`. The emission SHALL carry no URL, host, header, or
body label. Static mounts served by the `http-static` endpoint are
outside this requirement.

#### Scenario: successful inbound request

- **GIVEN** an `http` consumer route that answers a request successfully
  and the effective component switch for `http` is on
- **WHEN** the response body is exhausted
- **THEN** exactly one `("http", "consume", "success")` component
  operation is recorded and no `e:http:consume` error is emitted

#### Scenario: route-selected error status is a success

- **GIVEN** a route that sets `CamelHttpResponseCode = 503` and completes
  normally
- **WHEN** the response body is exhausted
- **THEN** the component operation is `("http", "consume", "success")`
  and no `e:http:consume` error is emitted

#### Scenario: body-less success responses

- **WHEN** a successful route answers with an empty body, a `HEAD`
  request, or a `204`/`304` response
- **THEN** exactly one `("http", "consume", "success")` component
  operation is recorded, never a failure from a body that is never
  polled

#### Scenario: failed inbound request

- **GIVEN** the same wiring where the route pipeline returns an error
- **WHEN** the failed reply is classified and its error response body is
  finalized
- **THEN** one `("http", "consume", "failure")` component operation is
  recorded, plus `increment_errors("http", "e:http:consume")`, and no
  success is recorded when the error body is exhausted

#### Scenario: streaming reply completes

- **GIVEN** a successful route whose reply body is a stream
- **WHEN** the stream reaches its final frame
- **THEN** one `("http", "consume", "success")` component operation is
  recorded

#### Scenario: final frame signals end-of-stream

- **GIVEN** a non-empty reply body whose last frame also sets the inner
  body to end-of-stream
- **WHEN** that frame is polled and the body is dropped without another
  poll
- **THEN** exactly one `("http", "consume", "success")` component
  operation is recorded and no failure is recorded at drop

#### Scenario: dispatcher replaces an oversized response

- **GIVEN** a route reply whose buffered body exceeds
  `maxResponseBody`
- **WHEN** the dispatcher substitutes its limit-exceeded response
- **THEN** exactly one `("http", "consume", "failure")` component
  operation and one `e:http:consume` error are recorded, even after the
  replacement body is exhausted

#### Scenario: two contexts share one listener

- **GIVEN** two consumers registered on the same host/port from
  different contexts with distinct collectors and distinct `http`
  effective switches
- **WHEN** one request per consumer is handled
- **THEN** each request's `consume` emission reaches its own consumer's
  collector and observes its own effective switch

#### Scenario: streaming reply aborted early

- **GIVEN** a successful route whose reply body is a stream
- **WHEN** the reply body is dropped before end-of-stream (client abort)
- **THEN** one `("http", "consume", "failure")` component operation is
  recorded plus `e:http:consume`

#### Scenario: streaming reply error is a failure, once

- **GIVEN** a successful route whose reply stream yields an error frame
- **WHEN** the body is polled, and afterwards either ends or is dropped
- **THEN** exactly one `("http", "consume", "failure")` component
  operation is recorded and no late success is recorded

#### Scenario: request rejected before the route

- **GIVEN** an inbound request rejected by the dispatcher (request body
  over the limit, inflight exhaustion, or a closed route channel)
- **WHEN** the rejection response is produced
- **THEN** exactly one `("http", "consume", "failure")` component
  operation and one `e:http:consume` error are recorded

#### Scenario: handler cancelled before the response

- **GIVEN** an inbound request whose dispatch future is dropped while
  awaiting the route channel or the reply (for example the dispatch
  timeout fires)
- **WHEN** the future is dropped
- **THEN** exactly one `("http", "consume", "failure")` component
  operation and one `e:http:consume` error are recorded, and no success
  is recorded later

#### Scenario: failure with the component switch off

- **GIVEN** the effective component switch for `http` is off
- **WHEN** an inbound request fails
- **THEN** `increment_errors("http", "e:http:consume")` is still
  recorded and no component operation is recorded

#### Scenario: no request data becomes a label

- **WHEN** any inbound request completes or aborts
- **THEN** the emitted labels are exactly `component="http"` and
  `operation="consume"` (plus the two-value outcome), and no path,
  query, host, header, or body value is emitted by this requirement

## MODIFIED Requirements

### Requirement: Uniform component operations family

Components SHALL emit
`camel_component_operations_total{component,operation,outcome}` with
outcome ∈ {success, failure} for their principal operations, through a
component-scoped `ComponentMetrics` handle that fixes the component
identity at construction and exposes `observe(operation, failed)`.
Emission SHALL be gated by the effective per-component switch
(`enabled && component_overrides.get(component).unwrap_or(components)`)
resolved from `[observability.metrics]`; error-family forwarding SHALL
NOT be gated by any lever or override. Components audited as
holding-but-not-emitting (wasm, opensearch, seda, surrealdb remainder,
cxf remainder) SHALL emit this family for errors at minimum.

#### Scenario: healthy route observable

- **WHEN** a route using a swept component processes exchanges
  successfully with the effective switch on for that component
- **THEN** `camel_component_operations_total{...,outcome="success"}`
  increases for the component's principal operation

#### Scenario: dead-observability component now emits errors

- **WHEN** a swept component's operation fails with the effective switch
  off
- **THEN** the failure is observable on the component's error-family
  emission despite the switch being off

#### Scenario: scoped handle fixes the identity

- **GIVEN** a `ComponentMetrics` built with component `redis` and a
  recording collector
- **WHEN** `observe("command", false)` is called twice
- **THEN** both component-operation records carry `component="redis"`
  and no other identity can be supplied at the call site

#### Scenario: registry context honors the components lever

- **GIVEN** a `RegistryComponentContext` constructed with the lever
  snapshot on and a wired collector
- **WHEN** `component_metrics_enabled()` is queried and the facade is
  built through `component_metrics()`
- **THEN** the lever reports on and the facade emits the
  component-operations family through the wired collector
- **AND** constructed with the lever off, the family is suppressed
  while the error family still flows

#### Scenario: registry context honors the effective switch

- **GIVEN** a context whose levers enable component operations for
  `wasm` only
- **WHEN** `component_metrics("wasm")` is built and observes a
  successful invocation
- **THEN** the component-operations family is emitted for `wasm`, and a
  handle built for another component emits nothing while still
  forwarding its errors

#### Scenario: handle preserves late binding and NoOp safety

- **GIVEN** a handle built over a late-bound collector slot before any
  real collector is registered
- **WHEN** `observe` runs before registration and again after a
  collector is registered
- **THEN** the pre-registration call is a safe no-op and the
  post-registration call reaches the registered collector

#### Scenario: composite collectors receive the scoped labels exactly once

- **GIVEN** two collectors registered on one late-bound handle, and a
  scoped `ComponentMetrics` for `http`
- **WHEN** `observe("consume", true)` runs once
- **THEN** each collector receives exactly one
  `("http", "consume", "failure")` component operation and exactly one
  `e:http:consume` error
- **AND** a subsequent `observe("consume", false)` records one
  `("http", "consume", "success")` per collector and no error

#### Scenario: existing producer labels are unchanged

- **GIVEN** the HTTP producer's request boundary
- **WHEN** a request fails
- **THEN** the emitter still records `("http", "request", "failure")`
  and `e:http:request`, and the consumer `consume` series is not
  touched by producer activity
