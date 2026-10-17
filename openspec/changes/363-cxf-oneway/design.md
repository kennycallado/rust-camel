# Design: 363-cxf-oneway

## Approach

Use the selected WSDL service/port binding as the operation authority. Reuse CXF binding metadata in `CxfClientManager`; resolve explicit producer operation names there. Without an explicit operation, resolve the bare payload root QName against input message parts. Reject zero or multiple matches before transmission. Derive SOAPAction and input-only shape from the same resolved operation. Do not mutate cached dispatch contexts per call or infer shape from an empty reply.

For input-only operations call `Dispatch.invokeOneWay` exactly once and return an empty successful gRPC `SoapResponse`. CXF retains ownership of envelope creation and transport TLS. Do not serialize a response Source. Set CXF's `org.apache.cxf.oneway.robust` context property to false for this path. No public robust-mode option is added: robust SOAP fault observation is unsupported for producer one-way sends. Locally detected transport failures remain errors; remote business faults are not guaranteed observable. Request-reply continues to call `invoke` once; null/empty results and parser failures from empty HTTP 200/202 remain errors. Never recover an invocation failure by resending as one-way.

The Rust producer already maps empty successful gRPC bytes to an empty body. Retain that behavior and verify it. Its existing automatic bridge-transport retry must not replay a potentially transmitted SOAP invocation: remove the retry for invoke, retaining bridge recovery for subsequent calls. This prevents a lost gRPC reply from causing a second remote delivery.

For consumers, resolve input shape from the startup-selected WSDL binding using the SOAP body QName and SOAPAction. A supplied action must agree with the resolved input; unknown or ambiguous input fails before route dispatch. Preserve WSS verification and signer export before dispatch. Keep the existing correlated completion future. An input-only ACK requires a correlated, non-fault `ConsumerResponse`, including successful Rust response marshalling. Discard its successfully marshalled payload and return status 202 with no body and no SOAP envelope/signature. Do not ACK at receipt or dispatch time. Errors retain the 361 fault/status matrix, including route faults, marshalling failures, timeout, and stream-unavailable 503. A successful route returning `Body::Stream` still fails response marshalling and returns Receiver HTTP 500, not 202. A one-way producer's non-robust mode does not promise to observe those remote faults.

No protobuf field is needed: Java owns input shape and suppresses output after the existing completion ACK. Rust route execution and error marshalling remain unchanged.

## Affected crates

- `camel-component-cxf`: replay safety, empty-success regression tests, public documentation.
- `camel-test`: native mTLS empty-response test and consumer wire coverage.
- `bridges/cxf`: operation metadata, one-way dispatch, and consumer ACK rendering.

## Architecture boundaries

These are Components and bridge transport changes. Runtime/DSL/Services/Languages/Functions gain no new behavior or configuration. Preserve data/control separation (ADR-0001), the exchange-data trust boundary (ADR-0032), and bridge IPC mTLS (ADR-0036), as referenced by `CONTEXT-MAP.md`. Document one-way completion semantics in the CXF domain context and glossary without changing 361 security semantics.

## Phases

### Phase 1: Producer one-way delivery
- **Goal:** Select WSDL input-only shape and send once without parsing an ACK body.
- **Dependencies:** Landed mission 361; current `0.8.0` bridge interfaces.
- **Externally-visible types/interfaces:** Existing producer invocation returns empty bytes for one-way; request-reply empty responses are errors. No new public configuration.
- **Deliverable:** Producer implementation, Java wire/unit tests, Rust replay/empty tests.
- **Exit-criteria:** Java test/format gates and affected Rust tests/fmt/clippy pass; phase review approves; commit and inbox evidence recorded. Park before Phase 2.

### Phase 2: Consumer ACK and native verification
- **Goal:** Deliver correlated empty 202 ACKs and prove native mTLS behavior.
- **Dependencies:** Phase 1 approved.
- **Externally-visible types/interfaces:** A correlated non-fault input-only response, after Rust marshalling, changes from SOAP 200 to empty 202.
- **Deliverable:** Consumer implementation/tests, operator docs, rebuilt native `0.8.0` evidence.
- **Exit-criteria:** Required exact-name acceptance and 361 native regressions pass; repository quality gates and holistic review recorded; final commit and inbox handoff. Master owns landing/archive/closure.

## Alternatives considered

Treating every empty reply as success hides request-reply protocol failures. Retrying after EOF risks duplicate delivery. A robust-mode switch adds an unrequested fault-observation contract; this change documents the fixed non-robust producer mode instead. Early consumer ACK discards route completion failures. Rebuilding mTLS or envelope machinery duplicates mission 361 and increases risk.
