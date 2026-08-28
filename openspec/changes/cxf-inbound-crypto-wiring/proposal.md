# Proposal: cxf-inbound-crypto-wiring

## Why

`SecurityProfile.createInInterceptor()` (bridges/cxf, L304-312) puts the
crypto `Properties` object directly as the `SIG_PROP_REF_ID` /
`DEC_PROP_REF_ID` property value. WSS4J's `WSHandler.getString()` casts
that value to `String` — any producer profile configured to verify
signed or decrypt encrypted peer responses throws
`ClassCastException` at the first secured response. The producer path
can now sign (fixed in d8bd125b) but cannot verify what comes back:
silent until first use, broken since the interceptor existed.

Identical defect class as the outbound fix in `cxf-producer-timestamp`
(f1448d8d), fenced there by design D4 and filed as bd rc-13zd (P1).

## What Changes

- `SecurityProfile.createInInterceptor()` (Java-only):
  - Add `DEC_CRYPTO_REF_ID` constant; put `SIG_PROP_REF_ID` =
    `SIG_CRYPTO_REF_ID` string + Properties under that key (truststore
    crypto), `DEC_PROP_REF_ID` = `DEC_CRYPTO_REF_ID` string +
    Properties under that key (keystore crypto) — mirroring the
    outbound pattern at L238-239/L270-271.
- Wire-level inbound tests: sign/encrypt a document, then process it
  through the real `WSS4JInInterceptor` on an in-phase
  `PhaseInterceptorChain`; assert verification succeeds and the
  content survives.
- README: note that inbound verification/decryption is functional and
  how actions.in gates it.

Non-goals: decrypt-callback password-usage semantics (DECRYPT usage
path — separate concern, observed-adjacent); action-token validation on
the IN path (out-path discipline from rc-u97s does not transfer
one-to-one; separate decision); TTL/replay knobs.

bd: rc-13zd.

## Acceptance criteria

- Profile with truststore + in-actions "Signature": inbound processing
  of a document signed by the outbound path verifies without exception.
- Profile with keystore + in-actions "Encrypt": encrypted document
  decrypts through the in-interceptor, content round-trips.
- Interceptor property shape: `SIG_PROP_REF_ID`/`DEC_PROP_REF_ID` values
  are String ref-ids; the `Properties` objects live under those keys.
- Full Java suite green (131 existing + new); spotlessCheck green.

## Risk budget

- Fix shape is already proven on the outbound path in production-bound
  code; wire tests prove it on the in-path. No configuration surface
  changes; no wire-format changes for valid profiles (they were broken
  before — this only makes the documented behavior real).
