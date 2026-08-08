# camel-xslt

XSLT transformation Component for the `xslt:<stylesheet>` URI scheme. The
Component is a thin gRPC client for the Java/Saxon `xml-bridge` sidecar in
`bridges/xml/`.

## Delegation model

The Rust crate does not parse XML or execute XSLT. It reads the configured
stylesheet, bounds the Exchange body, and forwards both as bytes through
`proto/xml_bridge.proto`. The sidecar owns stylesheet compilation,
transformation, XML parsing, DTD handling, and entity resolution.

The sidecar is therefore the security location for XSLT secure processing,
`document()` restrictions, XXE controls, entity-expansion limits, and XML-bomb
protection. `BridgeError.Kind.SECURITY_VIOLATION` provides a contract for the
sidecar to report a policy rejection. Its presence does not prove that the
sidecar enables each defense. Audit those controls in `bridges/xml/`.

## Trust boundary

ADR-0032 classifies endpoint configuration as trusted operator input and the
Exchange body as untrusted exchange data.

| Input | Source and trust | Handling |
|---|---|---|
| Stylesheet | Endpoint URI, trusted | Read from disk during Endpoint creation, then compiled by the sidecar |
| XML document | Exchange body, untrusted | Bounded and forwarded as bytes without Rust-side XML parsing |
| Parameters and output method | Endpoint query, trusted | Forwarded as string values to the sidecar |

No Exchange field can select or replace the stylesheet.

## Input bound

ADR-0040 governs body materialization. `maxPayloadBytes` sets the limit. When
the option is absent, `XsltProducer::call` uses
`DEFAULT_MATERIALIZE_LIMIT` (10 MiB). It passes the effective limit to
`Body::into_bytes` before it sends the document to the sidecar.

## Log-level policy

Per ADR-0012, this component's `error!` / `warn!` sites are categorized as:

### Client (bridge reconnect)

- **(e) outside-contract** (client.rs L328): failed to re-seed stylesheets after bridge reconnect
  (transient recovery, NOT handler invocation). Calls
  `metrics().increment_errors(route_id, "e:xslt:reconnect-reseed")` BEFORE the `error!`.
  The metric is the operator signal; `error!` provides loud log visibility. Both stay.

- **best-effort reseed** (`XsltTransformBackend::recompile_all`): a per-stylesheet
  `warn!` records a failed compile during reseed. The aggregate failure reaches
  the outside-contract site above.

### Producer (transform pipeline)

- **(a) handler-owned** (producer.rs L160): stylesheet compilation failed inside the pipeline (`warn!`).
  Route ErrorHandler owns the ERROR. Downgraded to `warn!` with
  `// log-policy: handler-owned`. No metric call.

- **(a) handler-owned** (producer.rs L186): XSLT transform failed inside the pipeline (`warn!`).
  Route ErrorHandler owns the ERROR. Downgraded to `warn!` with
  `// log-policy: handler-owned`. No metric call.

- **transient retry** (`XsltBridgeRuntime::transform_with_retry`): `warn!` records
  a transport failure before retry. The operation has not returned an error.

- **degraded or stopped readiness** (`XsltProducer::poll_ready`): `warn!` precedes
  the returned error. The Route ErrorHandler owns that error.

### Component lifecycle

- **shutdown cleanup** (`XsltBridgeRuntime::shutdown`): `warn!` records failure to
  stop the bridge process during context shutdown.
