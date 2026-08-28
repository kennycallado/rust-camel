# Design: cxf-inbound-crypto-wiring

## Approach

Single-method Java fix plus wire tests — mirrors the outbound crypto
ref-id pattern already merged (f1448d8d).

### D1: Ref-id wiring

`createInInterceptor()`:

- Signature branch: `props.put(SIG_PROP_REF_ID, SIG_CRYPTO_REF_ID)`
  (existing constant, L48) + `props.put(SIG_CRYPTO_REF_ID,
  createCryptoProperties(truststorePath, truststorePassword))`.
- Encrypt branch: new constant `DEC_CRYPTO_REF_ID = "decCryptoProperties"`
  beside `ENC_CRYPTO_REF_ID` (L50); same two-line shape with keystore
  crypto. `PW_CALLBACK_REF` stays `callbackWithPassword(keystorePassword)`
  (unchanged — DECRYPT usage semantics are a non-goal).

### D2: Wire-level inbound proof

New `SecurityProfileInWireTest`: build a secured document with the
producer's own outbound interceptor chain (from `cxf-producer-timestamp`,
verified working), then run it through the profile's
`WSS4JInInterceptor` on a real in-phase `PhaseInterceptorChain`
(`PhaseManagerImpl.getInPhases()` + `SAAJInInterceptor`),
`setInterceptorChain` before `doIntercept`, assert on the resulting SAAJ
document. Two cases: Signature (verify; Body content intact) and
Encrypt (decrypt; content round-trips). Fixture reuse:
`TestKeystoreHelper` split-password overload where needed.

### D3: Property-level shape tests

Extend `SecurityProfileTest`: assert `SIG_PROP_REF_ID`/`DEC_PROP_REF_ID`
values are Strings naming keys whose values are `Properties` — the
shape WSS4J `getString()` requires.

## Affected crates

- None (Rust). `bridges/cxf` (Java): `SecurityProfile.java`,
  `SecurityProfileTest.java`, `SecurityProfileInWireTest.java` (new),
  `README.md`.

## Architecture boundaries

Bridge-internal; no gRPC contract change, no Rust surface, no new env
vars. Same-file symmetry with the outbound path keeps the security
profile single-sourced.

## Phases

Single phase — one coherent slice.
