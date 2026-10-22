## Producer envelope and SOAP version

The producer dispatch runs `Service.Mode.PAYLOAD` and passes the Exchange body as
the bare operation element. The bridge emits exactly one SOAP `Envelope` with one
`Body`; the operation element is a direct child of `Body`. A body that is already
a SOAP `Envelope` is rejected before dispatch, so no nested envelope reaches the
wire.

The SOAP version derives from the selected WSDL port binding
(`dispatch.getBinding().getBindingID()`), not from a `soap-version` request
header; that header is ignored. An unrecognized binding ID fails the invocation
instead of defaulting to SOAP 1.1. Envelope parsing stays in the bridge, which
owns the XML boundary (ADR-0032).

## One-way SOAP operations

A **One-way SOAP operation** is a WSDL operation in the selected service/port
binding with an input and no output. The selected binding is the operation
authority for both directions. An operation with WSDL output is request-reply.

### Producer

The producer resolves the operation against the selected binding: an explicit
`operation` name, or, when the name is absent, the bare payload root QName. An
unknown, ambiguous, or missing selection fails before any wire transmission. The
resolved dispatch sets the SOAPAction and the exchange shape together.

For an input-only operation the bridge calls `Dispatch.invokeOneWay` exactly once
with the bare payload. Empty HTTP 200 and 202 acknowledgments complete
successfully with empty output bytes; the bridge does not deserialize an absent
response body. Resolved WSDL dispatches are cached per profile in the bounded LRU,
with the non-robust flag fixed at creation.

One-way producer dispatch is non-robust and there is no public `oneway.robust`
option: `org.apache.cxf.oneway.robust` is fixed to false. Successful send
completion does not assert remote business success and does not guarantee
observation of a remote SOAP fault. A local bridge or transport failure remains an
error; it is never converted into an empty success. A local transport failure
after a one-way send does not trigger a replay or resend. Recovery is a subsequent
explicit send, a separate action.

An operation with WSDL output keeps one `Dispatch.invoke`. An empty response is an
error: the producer never falls back to one-way, fabricates a payload, or resends.

### Consumer listener

Startup builds a per-profile operation catalog from the selected WSDL binding
before the HTTP listener binds. Misconfigured or unloadable WSDL metadata fails
listener startup before a ready port is announced. Every request resolves the SOAP
`Body` root QName against the profile's startup catalog. A supplied SOAPAction must
agree with the resolved operation.

An unknown, conflicting, or ambiguous operation faults Sender (SOAP 1.1 HTTP 500,
SOAP 1.2 HTTP 400) with zero route dispatch. The WSS-only `fault_status` override
never applies to these faults.

For a resolved input-only operation the listener waits for the correlated,
non-fault `ConsumerResponse`, including successful Rust response marshalling. It
then returns HTTP 202 with a zero-length body, discards the marshalled payload, and
writes no outbound ACK envelope and no ACK signature. Route success alone does not
bypass marshalling: a route returning `Body::Stream` marshals to nothing, and that
marshalling failure is a Receiver fault (HTTP 500, never 202). Request-reply
success remains SOAP HTTP 200, wrapped and signed once. Stream unavailability
remains HTTP 503 with `Retry-After` and a plain body. The transport availability,
XML, WSS, signer-identity, capacity, and timeout gates are unchanged.

## Security posture

### XML parsing delegation

The Rust Component treats SOAP and WSDL XML as opaque bytes. It does not run an
XML parser. The supervised Java CXF bridge owns XML parsing, DTD handling, and
entity resolution. This places the XXE boundary in the sidecar, not in this
crate. Audit parser hardening in the bridge process. See ADR-0032 for the trust
boundary.

### Credential channel separation

The bridge process receives keystore, truststore, and signature passwords
through per-profile environment variables. `CxfProfileEnvVars` wraps these
values in `Redacted` while it builds the child-process configuration. gRPC
requests carry only the configured `security_profile` selector. They do not
carry password bytes. See ADR-0051 for credential redaction requirements.

### Transport TLS and message-layer separation

Producer transport TLS comes from the nested `[profiles.security.tls]` table.
`CxfTlsConfig::to_env` serializes it as `CXF_PROFILE_<NAME>_TLS_*` environment
variables. The bridge builds `TLSClientParameters` from the `TLS_KEYSTORE_*`
(key managers) and `TLS_TRUSTSTORE_*` (trust managers) pairs and attaches them
to the CXF `HTTPConduit` of each dispatch. `CxfClientManager.initTlsStores()`
loads and validates every profile's stores during Quarkus startup, before the
`ready` line; a store that a password does not open aborts startup.

This is the HTTP transport layer, not the message layer. It is separate from
the WS-Security `keystore`/`truststore` fields, which sign and encrypt SOAP
envelopes. The two pairs are independent: transport TLS encrypts and
authenticates the connection, WS-Security protects the envelope. Hostname
verification is always on, and one profile's TLS material never applies to
another profile.

Producer transport TLS covers the producer. The consumer listener (Phase 2,
operational) has its own FLAT pool keys beside `bind_address`
(`soap_tls_cert_path`/`soap_tls_key_path` XOR `soap_tls_pkcs12_path`/
`soap_tls_pkcs12_password`, plus the always-required `soap_tls_client_ca_path`).
They are pool-level, never per-profile, and independent of the WSS stores.
Client auth is required (mutual TLS): a missing or foreign client certificate
is rejected at the handshake. The bridge binds the configured address (port `0`
= OS-assigned, JVM-held) and announces the ACTUAL bound port in the
`soap-ready` line; `BridgeProcess` awaits `ready` + `soap-ready` (any order)
under one startup budget and exposes the announced SOAP port read-only
(`CxfBridgePool::listener_info`). Before the consumer stream of the addressed profile is open, requests to that profile
take the pre-stream gate: HTTP 503 + `Retry-After` + plaintext, never a SOAP
fault. Plaintext `http` ingress remains an allowed interim form. Consumer streams are per profile: `CxfConsumer` announces its profile as `x-cxf-profile` metadata, the bridge dispatches by the profile in the request path with no cross-profile fallback and answers an unknown or missing profile with `INVALID_ARGUMENT` (terminal for the consumer, no reconnect), and the pool-level `ConsumerProfileGuard` rejects a second consumer route on one profile at start (rc-gsy1v).

## Consumer fault matrix

The listener classifies every SOAP-processing failure as `Sender` or `Receiver`
and maps the class to an HTTP status. Malformed XML and a missing SOAP `Body`
are detected before WS-Security, so a profile's security override can never
rewrite them. SOAP 1.1 returns 500 for every fault; SOAP 1.2 returns 400 for
`Sender` and 500 for `Receiver`. A fault never returns 200.

Transport rejections are not SOAP faults and stay outside this matrix: the body
cap (`CXF_MAX_BODY_BYTES`, HTTP 413 plaintext, enforced during body accumulation
before XML, WS-Security, and the availability gate) and the availability gate
(HTTP 503 + `Retry-After` plaintext, pre-stream or lost stream). A
`fault_status` override never applies to either.

`[profiles.security] fault_status` is an optional integer in 400–599, validated at
startup. It overrides the status for that profile's security (WSS) faults only, on
both SOAP versions. It does not apply to the signer descriptor, route errors,
timeouts, malformed input, a missing `Body`, or Rust `soap:Client`/`soap:Server`
faults, and there is no global override. The override is a response-shaping value,
not an authentication decision: it writes no principal and no `camel.auth.*`
property.

`[profiles.security] timestamp_ttl_seconds` (default `300`) and
`timestamp_future_tolerance_seconds` (default `60`) are optional integers from
1 to 2147483647 s (about 68 years, the WSS4J `int` limit) that widen the
consumer inbound WS-Security `Timestamp` window; larger values are rejected at
startup (ADR-0088). They are emitted as
`CXF_PROFILE_<NAME>_TIMESTAMP_TTL_SECONDS` and
`CXF_PROFILE_<NAME>_TIMESTAMP_FUTURE_TOLERANCE_SECONDS`. `CxfPoolConfig::validate`
returns `CamelError::Config` for `0` and for a set value on a profile with
neither a keystore nor a truststore, where the window would be inert. Profile
loading validates these values regardless of endpoint role; the producer
in-interceptor does not apply them.

## Consumer signer identity

The listener exports the request signer as the `SignerIdentity` field of the
`ConsumerRequest`: `subject` and `issuer` are RFC 2253 distinguished names,
`serial` is the lowercase-hex certificate serial number, and
`fingerprint_sha256` is 64 lowercase hex characters of the SHA-256 digest of the
DER certificate. No DER bytes cross the wire. The certificate must come from a
validated, trusted `SIGN` engine result that covers the SOAP `Body`; the bridge
never reads identity from headers, a `UsernameToken`, or an unchecked
certificate. A certificate whose DER encoding exceeds 64 KiB is rejected, never
truncated. At most 8 `SIGN` results are inspected and a 9th `SIGN` result rejects
the request on the budget alone; one certificate in several results counts as one
signer; more than one distinct Body-covering fingerprint rejects the request as
ambiguous; a signature that does not cover the `Body` never yields a signer. When no signature is present the field stays absent,
meaning unknown.

The descriptor is signer identity information, not authentication. It is never a
principal and never writes `camel.auth.*`; the Rust side stores it only under the
dedicated `camel.cxf.signer.*` exchange properties.

**Intended breaking change:** a profile whose inbound actions include
`Signature` now requires a Body-covering signature. Timestamp-only and
header-only signers that previously passed such profiles are now rejected.
Profiles without a `Signature` inbound action are not affected: no signer
inspection or signer policy runs on them.

## Log-level policy

Per ADR-0012:

- **(b′) outside-contract** (consumer.rs L~272): response-marshalling failure after handler returns (post-handler side-effect). `runtime.metrics().increment_errors(route_id, "b-prime:cxf:response-marshalling")` BEFORE `error!`. Keeps `error!`.

- **(a) handler-owned** (consumer.rs L~285): route handler invocation returned Err to CXF consumer. Downgraded to `warn!`. No metric — route ErrorHandler owns ERROR.

- **(c) system-broken** (pool.rs L~441): pool restart max-attempts exhausted → "staying degraded" (lifecycle termination). Keeps `error!` with `system-broken` annotation. No metric — operator action required (fix CXF service config + restart).
