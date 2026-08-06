# Validator Component

Schema validation component for the `validator:` URI scheme. It validates XML with an XSD
bridge and validates JSON and YAML schemas in-process.

## Glossary

**ValidatorComponent**:
Component factory for the `validator:` scheme. `ValidatorComponent::new()` owns one shared
`XsdBridgeBackend` and uses it for all XSD Endpoints that it creates.
_Avoid_: validator service, schema component

**ValidatorProducer**:
Producer that validates an Exchange body or configured header against the Endpoint's schema.
Validation failure returns a `CamelError` to the Route error handler.
_Avoid_: filter, consumer

**XsdBridgeBackend**:
Owner of the `xml-bridge` child process, gRPC channel, XSD schema cache, and reconnect re-seed.
The backend starts the child process lazily when the first XSD validation needs it.
_Avoid_: validator lifecycle service, embedded XML validator

## XSD bridge lifecycle

The validator crate does not implement `Lifecycle` for `XsdBridgeBackend`. Therefore,
`CamelContext::stop()` does not clean up the `xml-bridge` child process by itself.

`camel run` registers the CLI-side `BridgeCleanup` lifecycle service. Its `stop()` method calls
`XsdBridgeBackend::shutdown()`. Library embedders must retain the value returned by
`ValidatorComponent::xsd_bridge_backend()` and call `shutdown().await` when it is `Some`.
This requirement applies after an XSD route starts the bridge. JSON and YAML validation do not
start the bridge.

## `non_exhaustive` posture

ADR-0049 limits the workspace `#[non_exhaustive]` policy to contract crates. This component is
not a contract crate, so the policy does not apply to `SchemaType`, `ValidatorError`, or
`BridgeState`. These public enums remain exhaustive under the current policy. Reassess this
posture if an enum becomes a cross-crate contract, such as an operator-facing health state.

## Log-level policy

Per ADR-0012, this component's `error!` sites are categorized as:

### Bridge reconnect (xsd_bridge)

- **(e) outside-contract** (`XsdBridgeBackend::on_reconnect`, xsd_bridge.rs L372): re-seed schema failed after bridge reconnect
  (transient recovery, NOT validation failure). Calls
  `metrics().increment_errors(route_id, "e:validator:reconnect-reseed")` BEFORE the `error!`.
  The metric is the operator signal; `error!` provides loud log visibility. Both stay.
