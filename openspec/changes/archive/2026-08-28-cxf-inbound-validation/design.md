# Design: cxf-inbound-validation

## Context

`createInInterceptor()` gates inbound Signature on `hasText(truststorePath)`
(L304) and returns `null` when no action accumulated (L316):
truststore-less + `actions.in=Signature` skips verification silently. The
Encrypt branch (L309) has no keystore guard: with a truststore present the
interceptor is created with decryption material built from a null keystore
path — broken at message time rather than silent. `actions.in` (via
`resolveActionsIn()`) drives BOTH the interceptor and the manual
`WssSecurityProcessor.processInbound()`; only the manual verification crypto
(`getVerificationCrypto`) falls back to the keystore.

## Goals / Non-Goals

- Goals: build-time fail-loud for explicit-but-unmaterialized inbound
  actions; mirror the proven `validateOutboundActions` shape; README states
  the contract.
- Non-Goals: implementing keystore-as-truststore fallback on the in-path
  (role separation is deliberate — verification anchors live in the
  truststore); validating resolved runtime defaults (blank `actions.in` is
  raw-exempt); WSS4J action-token/TTL validation (separate concern).

## Decisions

### D1 — Validate the RAW inbound actions at build time

`build()` gains `validateInboundActions()` before `validateSignatureKnobs()`,
operating on the raw `securityActionsIn` string. Blank/unset → return (no
interceptor was requested; manual path unaffected). Non-blank containing
Signature without truststore → throw. Non-blank containing Encrypt without
keystore → throw. Messages name `cxf.truststore.path` / `cxf.keystore.path`
and state that the keystore fallback exists only on the manual consumer
path.

Alternatives rejected: (a) implement keystore fallback on the in-path —
blurs truststore/keystore roles and diverges from the fail-loud trilogy;
(b) throw inside `createInInterceptor()` — config errors must surface at
build, not at first route wiring.

### D2 — Material rules are per-action, not composition-based

Unlike the out-path (Timestamp-requires-Signature composition), inbound
actions are independent: verify and decrypt have no cross-action dependency.
Each action maps to exactly one material requirement — Signature →
truststore, Encrypt → keystore.

### D3 — Wire-level proof stays out of scope

The interceptor's verify/decrypt wire behavior was proven in
cxf-inbound-crypto-wiring. This change is validation-only; tests are
property-style against the Builder, mirroring the existing
`actionsOut` validation tests in `SecurityProfileTest`.

## Risks / Trade-offs

- Previously-silent configs now throw at build. Intentional (they were
  silently unprotected); the message points at the missing knob.
- `actions.in` is shared by both inbound paths, so a keystore-only manual
  consumer with explicit `actions.in=Signature` — working today via the
  manual keystore fallback — now fails at build. Accepted for unified
  semantics (truststore IS the verification anchor, one knob, one rule);
  the remedy is one line: `cxf.truststore.path` pointing at the same JKS.

## Migration Plan

Add `cxf.truststore.path` (may point at the same JKS as the keystore for
self-anchored topologies). Dropping `actions.in` only suits manual consumers
accepting the default: the runtime default resolves to `Signature`
(`resolveActionsOrDefault`), which the interceptor materializes only with a
truststore — keystore-only interceptor users must configure one, or they get
no in-interceptor (documented in README, not thrown: the manual keystore
fallback keeps blank+keystore-only working for manual consumers).

## Open Questions

None.
