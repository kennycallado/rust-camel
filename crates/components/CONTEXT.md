# Components

Inbound and outbound adapters that connect the Runtime to external systems. Each Component handles a URI scheme and creates Endpoints that produce Consumers or Producers.

## Language

**Component**:
Factory for Endpoints, identified by a URI scheme (e.g., `timer`, `kafka`, `http`). Registered into CamelContext by scheme.
_Avoid_: adapter, connector, plugin

**Endpoint**:
An instantiated communication point created by a Component from a URI. An Endpoint creates Consumers (inbound) or Producers (outbound).
_Avoid_: URI, address, channel (these describe *where* an Endpoint points, not the Endpoint itself)

**Consumer**:
Inbound adapter started by the Runtime for a Route's `from:` Endpoint. Event-driven (push model): receives data from an external system and submits Exchanges to the Route's Pipeline.
_Avoid_: listener, subscriber, source

**PollingConsumer**:
Pull-based adapter created on demand via `Endpoint::polling_consumer()`, delivering one Exchange per `receive(timeout)` call. Contrasts with the event-driven Consumer that drives a Route; a PollingConsumer never starts a Route. Endpoints opt in; purely event-driven components (HTTP server, Kafka) return `None` by default. Used by the EIP-7 `pollEnrich` verb and the WASM `camel_poll` host function. Established by ADR-0015.
_Avoid_: synchronous consumer, on-demand reader, pull consumer

**Producer**:
Outbound adapter created for a `to:` Endpoint via `Endpoint::create_producer()`, which returns a
`BoxProcessor`. Sends an Exchange to an external system by running it through the returned processor. Producers are strictly write/send (Tower `Service<Exchange>`); reading a resource mid-route uses `PollingConsumer`, not a producer mode.
_Avoid_: sender, publisher, sink

**ExchangeEnvelope**:
The transfer unit from Consumer to Pipeline — carries an Exchange and an optional reply channel for `InOut` exchanges.
_Avoid_: exchange wrapper, message envelope (use Exchange for the content, ExchangeEnvelope for the handoff)

**ConcurrencyModel**:
The Consumer's declared execution strategy: `Sequential` (one Exchange at a time) or `Concurrent { max: Option<usize> }` (parallel with optional bound; `None` means unbounded).
_Avoid_: threading model, parallelism setting

**ComponentBundle**:
A trait for a config-backed bundle that owns one TOML configuration key, deserializes its config,
and registers all related Component schemes through a single `register_all` call.
_Avoid_: plugin bundle, component group

**StaticFileServing** (HTTP component):
HTTP Endpoint behavior that serves files from disk, optionally with SPA fallback and custom error pages. StaticFileServing returns existing file content; it does not render templates or perform server-side rendering.
_Avoid_: SSR, template rendering, asset pipeline

**HTTP Co-hosting**:
HTTP behavior where `http:` API routes and `http-static:` mounts share one server per host/port. Dispatch precedence is API exact path match first, then static mounts by longest prefix, then SPA fallback/error page handling within the winning static mount.
_Avoid_: shared registry (implementation), separate static server

**StreamList** (SQL component):
SQL `outputType` that exposes result rows as a lazy NDJSON byte stream (`application/x-ndjson`) instead of materializing the whole result set before continuing.
_Avoid_: list mode, streaming query (too vague), cursor API

**ComponentBackoff / BackoffConfig**:
Capped exponential delay used inside networked Components for transient reconnect or poll retry loops. Not Route supervision and not ErrorHandler redelivery.
_Avoid_: retry policy (ambiguous), redelivery, ConsumerRestart

**Master**:
Component that runs a delegate Consumer only while this node holds a LeadershipService lock. Losing leadership stops delegate intake and steps down with a drain timeout.
_Avoid_: load balancer, consumer restart, supervision

**ControlBus**:
Producer-only Component that sends Route lifecycle commands (`start`, `stop`, `suspend`, `resume`, `restart`, `status`) from one Route to another through the RuntimeBus. Target Route comes from the URI `routeId` parameter or `CamelRouteId` header.
_Avoid_: command bus, admin API, route controller (unless naming Runtime type)

**SEDA**:
In-memory asynchronous Endpoint for decoupling Routes with a bounded queue. A Producer sends to `seda:name`; a Consumer on the same name processes later from a background task.
_Avoid_: direct route call, external queue, channel (too vague)

**SEDA Fanout**:
SEDA mode enabled by `multipleConsumers=true`; the Producer clones each Exchange to every active subscriber on the same SEDA name. Fanout is fire-and-forget only: waiting for replies is rejected because one request cannot have N valid replies without aggregator semantics.
_Avoid_: load balancing, competing consumers, broadcast queue

**ComponentContext**:
Runtime context passed to Components during Endpoint and Consumer/Producer creation. Provides
access to other components (`resolve_component`), languages (`resolve_language`), metrics
(`metrics`), and platform services (`platform_service`).
_Avoid_: global context, service locator (unqualified)

**ConsumerContext**:
The runtime handle provided to a Consumer when it starts — used to submit ExchangeEnvelopes into the Route's Pipeline and to detect shutdown.
_Avoid_: consumer handle, context (unqualified)

**Supervision**:
Route-level ownership of Consumer failure handling. A Consumer reports failure by returning an error from its running task; the Route records the failure through the RuntimeBus and, when a supervision policy is configured, restarts the whole Route rather than letting the Consumer restart itself.
_Avoid_: self-healing Consumer, hidden retry loop, task watchdog

**ConsumerRestart**:
Recreation of a Consumer as part of a Route restart after Consumer failure. ConsumerRestart is a Route lifecycle action: the old Consumer and Pipeline are stopped, a new Consumer is created from the Route's Endpoint, and Exchanges resume only after the Route is started again.
_Avoid_: reconnect, retry, hot reload

## Example dialogue

> "What is the difference between an Endpoint and a Component?"
> "A Component is the factory — it exists once per scheme. An Endpoint is the instantiated result of calling that factory with a specific URI. One Component creates many Endpoints."
>
> "When does the Consumer start?"
> "The Runtime starts the Consumer when it starts the Route. The Consumer runs independently, submitting ExchangeEnvelopes into the Pipeline."
