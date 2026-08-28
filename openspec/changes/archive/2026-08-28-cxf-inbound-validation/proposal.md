# Proposal: cxf-inbound-validation

## What

Fail-loud build-time validation for inbound WS-Security actions, mirroring
`validateOutboundActions` (landed in cxf-producer-timestamp). Two
degradations today: explicit `actions.in=Signature` without a truststore
builds a profile whose in-interceptor is `null` — peer responses pass
unverified, silently; explicit `actions.in=Encrypt` without a keystore (with
a truststore present) builds an interceptor wired to decryption material
from a null path — failing at message time.

## Why

- bd rc-es1t (P1). Two distinct degradations. (1) `actions.in=Signature`
  without a truststore: `createInInterceptor` skips the Signature branch and
  returns `null` — responses pass unverified, silently. (2)
  `actions.in=Encrypt` without a keystore: with a truststore present the
  interceptor is still created, wired to decryption material built from a
  null keystore path — misconfigured, failing at message time at best.
- This is the silent-security-bypass class the dead-config-policy canon
  forbids: a knob that requests protection and silently delivers none.
- Completes the fail-loud trilogy: out-actions (d8bd125b), signature knobs
  (d8bd125b), now in-actions.

## What Changes

- `SecurityProfile.Builder.build()` calls a new `validateInboundActions()`
  before construction (same shape as `validateOutboundActions`):
  - explicit `actions.in` containing Signature without `cxf.truststore.path`
    → `IllegalArgumentException` naming the knob and the manual-consumer
    fallback distinction;
  - explicit `actions.in` containing Encrypt without a keystore → throw.
  - blank `actions.in` stays raw-exempt (no interceptor requested).
- Property tests for both rejections plus the blank-exempt and
  truststore-present acceptance cases.
- README: `actions.in` row and inbound paragraph state the build-time
  contract; the legacy CAT112/Baleares interop example gains
  `cxf.truststore.path` (same JKS, self-anchored) so the example stays
  valid under the new validation.

## Impact

- Affected: `bridges/cxf` (SecurityProfile.java, tests, README).
- `actions.in` is shared by BOTH inbound paths (`createInInterceptor` and
  the manual `WssSecurityProcessor.processInbound`). Breaking surface:
  silently-unprotected interceptor configs now fail at build, AND
  keystore-only manual consumers with explicit `actions.in=Signature` —
  whose manual path previously fell back to the keystore — now require an
  explicit `cxf.truststore.path` (it may point at the same JKS).
- codecs: unaffected. specs: `bridge-transport-security` gains one
  requirement.
