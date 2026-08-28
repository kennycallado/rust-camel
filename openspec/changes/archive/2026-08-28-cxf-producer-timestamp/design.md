# Design: cxf-producer-timestamp

## Approach

Single-file Java change in `bridges/cxf` — `SecurityProfile.java` — plus
tests and README. No Rust changes, no new configuration surface.

### D1: Timestamp emission and action ordering

In `createOutInterceptor()`, add `WSHandlerConstants.TIMESTAMP` to the
action list when `containsAction(outActions, "Timestamp")`. Ordering:
Timestamp is inserted BEFORE Signature in the action list — signature
part resolution must see an already-materialized Timestamp element, the
same order the manual consumer path uses (`WssSecurityProcessor`
L78–83: timestamp built, then signed). Constant name and the
`ConfigurationConstants` action-string semantics are verified against the
shipped wss4j jar (gradle cache) BEFORE implementation — the rc-0xze
`signatureC14nAlgorithm` lesson (memory said long form, jar said
`signatureC14nAlgorithm`) makes jar verification mandatory, not optional.

### D2: Coverage default + wire-level proof

WSS4J's default signature parts cover the SOAP Body only — an emitted
Timestamp would be unsigned and rejected by any peer enforcing the
Body+Timestamp coverage invariant (including our own consumer). When
Timestamp and Signature are both active and the operator did not set
`SIGNATURE_PARTS`, we apply the same coverage the manual consumer path
applies (`WssSecurityProcessor` L92–98): Body + Timestamp. Exact parts
grammar (separator, keyword forms) is jar-verified during implementation;
candidates: `";"` separator with the `Body` / `Timestamp` keywords.

Property extraction alone cannot prove WSS4J parsed our parts grammar or
signed the Timestamp. The test suite therefore includes message-level
(DOM) tests on real WSS4J output: signed message contains wsu:Timestamp;
ds:Reference set covers Body and Timestamp; with explicit parts the
references match exactly. In-process only — no network, no container.

Explicit `SIGNATURE_PARTS` is applied verbatim — no implicit Timestamp
injection. Operator interop responsibility, documented in README.

### D3: Fail-loud validation (explicit actions only, deterministic order)

Two rules at `Builder.build()`, checked in this order:

1. **Composition first**: explicitly configured out-actions contain
   `Timestamp` but not `Signature` → `IllegalArgumentException`. An
   unsigned wsu:Timestamp is strippable by any intermediary — decorative
   security, dead on arrival.
2. **Material second**: explicitly configured out-actions contain any of
   `Signature`/`Encrypt`/`Timestamp` but no keystore →
   `IllegalArgumentException` naming the missing material. Closes the
   silent-unsigned-no-op hole: today `createOutInterceptor()` returns
   null before its log line and the request leaves unsigned while the
   operator asked for protection.

Explicit-only scope: the check reads the RAW `securityActionsOut` field,
not `resolveActionsOrDefault()` output — profiles that never configured
out-actions (blank → default "Signature" resolution) and
truststore-only profiles build unchanged. Both rules extend
`validateSignatureKnobs()` philosophy (knobs-requiring-context are
rejected at build) from knobs to the action tokens that gate them; the
validation lives beside it in the Builder.

### D4: Scope fences

- `createInInterceptor()` (producer verifying peer responses) untouched —
  non-goal; file separately if its action coverage needs the same
  treatment.
- No TTL knobs: WSS4J default time-to-live applies.
- `resolveActionsOrDefault()` unchanged — default out-actions remain
  `"Signature"`; existing valid configurations emit identical wire
  format.

## Affected crates

- None (Rust). `bridges/cxf` (Java): `SecurityProfile.java`,
  `SecurityProfileTest.java`, `SecurityProfileWireTest.java` (new),
  `README.md`.

## Architecture boundaries

Bridge-internal security policy — below the gRPC contract, no changes to
`SoapRequest`/`SoapResponse`, no changes to `camel-cxf` component options.
The Dispatch cache key (`DispatchKey`) is unaffected: security profile is
already a key dimension and actions are profile-scoped, not per-exchange.
Control-plane data-plane split respected: actions come from profile
configuration only.

## Phases

Single phase — one coherent slice, ~120 lines including tests.
