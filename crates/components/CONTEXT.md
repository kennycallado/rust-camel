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
Outbound adapter created for a `to:` Endpoint. Sends an Exchange to an external system.
_Avoid_: sender, publisher, sink

**ExchangeEnvelope**:
The transfer unit from Consumer to Pipeline — carries an Exchange and an optional reply channel for `InOut` exchanges.
_Avoid_: exchange wrapper, message envelope (use Exchange for the content, ExchangeEnvelope for the handoff)

**ConcurrencyModel**:
The Consumer's declared execution strategy: `Sequential` (one Exchange at a time) or `Concurrent { max: Option<usize> }` (parallel with optional bound; `None` means unbounded).
_Avoid_: threading model, parallelism setting

**ComponentBundle**:
A config-owned grouping of related Components (multiple URI schemes) registered together into CamelContext via a single call.
_Avoid_: plugin bundle, component group

**ConsumerContext**:
The runtime handle provided to a Consumer when it starts — used to submit ExchangeEnvelopes into the Route's Pipeline and to detect shutdown.
_Avoid_: consumer handle, context (unqualified)

## Example dialogue

> "What is the difference between an Endpoint and a Component?"
> "A Component is the factory — it exists once per scheme. An Endpoint is the instantiated result of calling that factory with a specific URI. One Component creates many Endpoints."
>
> "When does the Consumer start?"
> "The Runtime starts the Consumer when it starts the Route. The Consumer runs independently, submitting ExchangeEnvelopes into the Pipeline."
