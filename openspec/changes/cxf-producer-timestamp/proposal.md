# Proposal: cxf-producer-timestamp

## Why

The bridge-side CXF producer drops a `Timestamp` token from
`securityActionsOut` on the floor: `SecurityProfile.createOutInterceptor()`
honors only `Signature` and `Encrypt`, so an operator who requests
`"Signature Timestamp"` gets a signed body with no wsu:Timestamp — while our
own consumer (`WssSecurityProcessor`) requires a signature-covered Timestamp
for replay defense (rc-gevh invariant). The WSS contract is asymmetric: the
consumer demands what the producer cannot emit. Interop with any peer that
follows the same invariant (including another rust-camel deployment) fails.

Second defect in the same method: security out-actions without keystore
material degrade to a **silent unsigned no-op** (`createOutInterceptor()`
returns `null` before any log line; `SecurityProfileStore` only validates
keystore files that were actually configured). An operator who writes
actions but omits the keystore gets no security and no error — dead-config
policy violation in its most dangerous form.

bd: rc-u97s.

## What Changes

- `SecurityProfile.createOutInterceptor()` (bridges/cxf, Java-only):
  - Honor `Timestamp` in out-actions → add `WSHandlerConstants.TIMESTAMP`
    to the WSS4J action list.
  - When Timestamp + Signature are both active and no explicit
    `SIGNATURE_PARTS` is configured, default the signature coverage to
    Body + Timestamp (peer-consistent with the consumer invariant).
    Explicit parts are applied verbatim — operator interop responsibility,
    documented in README.
  - `Builder` validation: out-actions containing `Timestamp` without
    `Signature` are rejected (an unsigned Timestamp is strippable —
    decorative security); out-actions containing any of
    Signature/Encrypt/Timestamp without a keystore are rejected
    fail-loud (closes the silent unsigned no-op).
- README: producer WSS section documents the Timestamp action, the
  coverage default, and the explicit-parts responsibility.
- Tests: `SecurityProfileTest` property-level additions following the
  existing extraction pattern, plus a new `SecurityProfileWireTest`
  running the real interceptor chain in-process (no server, no network).

Affected crates: none (Java bridge only, `bridges/cxf`). Rust surface
unchanged — no new env vars beyond the existing profile action variables.

## Non-goals

- Inbound (`createInInterceptor`) action-coverage validation — different
  path, filed separately if confirmed.
- TTL configuration for outbound Timestamps (WSS4J default applies).
- SIGNATURE_PARTS enforcement of Timestamp coverage when the operator sets
  explicit parts — verbatim application is the contract.

## Acceptance criteria

- Out-actions `"Signature Timestamp"` + keystore → outbound interceptor
  whose action list contains both tokens, Timestamp first.
- No explicit parts + Timestamp active → signed output XML carries
  wsu:Timestamp with ds:Reference coverage of Body and Timestamp
  (message-level DOM assertion, in-process).
- Explicit `SIGNATURE_PARTS` → signature references match the configured
  parts exactly, no implicit injection.
- `actions="Timestamp"` (no Signature) → `IllegalArgumentException` at
  build naming both tokens (composition check fires first).
- `actions="Signature"`, `actions="Encrypt"`, or
  `actions="Signature Timestamp"` without keystore →
  `IllegalArgumentException` naming the missing keystore.
- Blank out-actions without keystore → builds unchanged (default action
  resolution exempt from the material check).
- Full Java suite green (117 existing + new); README updated.

## Risk budget

- WSS4J constant/property names verified against the shipped jar before
  implementation (empirical discipline from rc-0xze: the
  `signatureC14nAlgorithm` memory-vs-jar lesson).
- Behavioral risk: profiles that today silently skip security now fail at
  build. That is the intended fail-loud contract; release notes must say
  so — the release-notes carrier is the next bridge tag ritual (0.6.1,
  human-owned decision), not a file this change owns. No wire-format
  change for existing valid configurations (TS absent
  by default — `resolveActionsOrDefault` still returns `"Signature"`).
