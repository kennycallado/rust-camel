# Proposal: 363-cxf-oneway

## Why

Bd: `rc-nceky`. The camel-112 CyL protocol uses input-only WSDL operations and empty HTTP 200/202 acknowledgments. The bridge currently invokes every operation as request-reply and attempts to parse a SOAP response that does not exist.

## What Changes

- Resolve operation exchange shape from the selected WSDL binding, not from response content or Rust exchange defaults.
- Send input-only operations through `Dispatch.invokeOneWay`. Return successful empty bytes for empty HTTP 200/202 acknowledgments without a second send or a second SOAP envelope.
- Keep request-reply strict: empty HTTP 200/202 is an error, not an implicit acknowledgment or a retry trigger.
- Define non-robust one-way producer semantics explicitly. Preserve listener validation and fault handling before acknowledgment.
- Return empty HTTP 202 for an input-only consumer operation after a correlated, non-fault response, including successful Rust response marshalling.
- Update operator documentation and add Java, Rust, and native wire coverage.

Affected crates: `camel-component-cxf` and `camel-test`; Java bridge: `bridges/cxf`. Mission 361 is already landed. Its mTLS, envelope fidelity, WSS identity, fault matrix, listener TLS, and confined XSD contracts remain mandatory regression coverage. This change does not redesign those contracts. The bridge pin remains `0.8.0`.

## Acceptance criteria

- `cxf_oneway_empty_200_and_202_complete_ok_single_envelope_over_mtls` passes against the rebuilt native bridge: each status variant yields `Ok`, one POST, one SOAP 1.1 Envelope, and empty output.
- Input-only consumer success yields empty HTTP 202 only after correlated non-fault completion and successful Rust response marshalling; request-reply behavior and all 361 security checks remain intact.
- Unknown or ambiguous operations fail before sending. Empty request-reply responses never succeed silently or cause a duplicate send.
- Spec and full plan receive separate fresh `e_gpt` blessings; task, phase, and holistic reviews use `r_gpt`.

## Risk budget

One-way transport completion does not prove remote business processing. That limitation is explicit and acceptable. Duplicate delivery, TLS downgrade, SOAP double-wrapping, swallowed transport failures, and weakened pre-ACK security checks are not acceptable. Native verification must use the rebuilt `0.8.0` artifact, not a downloaded older binary.
