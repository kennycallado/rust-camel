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
Inbound adapter started by the Runtime for a Route's `from:` Endpoint. Receives data from an external system and submits Exchanges to the Route's Pipeline.
_Avoid_: listener, subscriber, source

**Producer**:
Outbound adapter created for a `to:` Endpoint via `Endpoint::create_producer()`, which returns a
`BoxProcessor`. Sends an Exchange to an external system by running it through the returned processor.
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
