# Design: rootorder

## Approach

Single-function reorder inside `camel-component-http` plus test
strengthening. No new types, no API change.

1. `fallback_root_store` (crates/components/camel-http/src/lib.rs)
   currently initializes `rustls::RootCertStore` with
   `webpki_roots::TLS_SERVER_ROOTS.to_vec()` and later appends the
   custom CA via `add_parsable_certificates`. Change to:
   start from `RootCertStore::empty()`, parse/add the custom CA
   (strict policy unchanged: unreadable / zero-PEM / zero-accepted
   still fail closed under strict, warn-and-degrade otherwise),
   then `store.roots.extend_from_slice(webpki_roots::TLS_SERVER_ROOTS)`
   on the success path. Degrade paths return a fresh Mozilla-only
   store (`TLS_SERVER_ROOTS.to_vec()`), so their contents are
   unchanged by this fix.
2. Strengthen `test_fallback_root_store_union_custom_and_mozilla`:
   keep the count assertion, add (a) expected custom anchor at
   index 0 (rebuild the expected anchor the same way the store
   does), (b) tail equality with `TLS_SERVER_ROOTS`, (c) the
   explicit order assertion: position of first custom root <
   position of first Mozilla anchor.
3. Cite, in the test's doc comment, the two-source evidence:
   - Order-insensitive verification: rustls 0.23.45
     `src/webpki/verify.rs:245-263` passes `&roots.roots` in store
     order to rustls-webpki; rustls-webpki 0.103.15
     `src/verify_cert.rs:66-73` + `loop_while_non_fatal_error`
     (`:757-774`) returns on the first anchor that VALIDATES
     (`Ok(anchor) => return Ok(anchor)`) and continues past
     non-fatal misses — every anchor is tried, so accept/reject
     cannot depend on anchor order.
   - Parity target: rustls-platform-verifier 0.7.0
     `src/verification/others.rs:61-93` adds `extra_roots` to an
     empty store before appending native certs.

## Affected crates

- `camel-component-http` (one function, one test).

## Architecture boundaries

Component layer only. No Runtime/DSL/Services changes; no
contract-api surface touched (private fn).

## Phases (optional)

Single phase — one coherent task; splitting reorder from its
order-pinning test would let the parity regress again between
tasks.

## Alternatives considered

- Leave order as-is, document only: rejected — the fallback's job
  is to stand in for the primary path; silent shape drift is the
  defect class castrict-rpt flagged, and the count-only test is
  the coverage gap that hid it.
- Sort or dedup the union: rejected — adds behavior beyond the
  mission; rustls treats the store as an iteration list, and the
  primary path does not sort either.
