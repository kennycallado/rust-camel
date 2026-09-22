# Proposal: rootorder

## Why

The CA-less-platform TLS fallback (`fallback_root_store`,
crates/components/camel-http/src/lib.rs) assembles its root store
Mozilla-anchors-FIRST and appends the configured custom CA afterward.
The primary path (`add_root_certificate` through
rustls-platform-verifier 0.7.0, `verification/others.rs:61-93`)
inserts configured extra roots BEFORE platform roots. The fallback
silently inverts that order, and the union unit test asserts only
the root COUNT, so the parity drift is invisible to the suite
(castrict-rpt finding, bd rc-d7rc3, P2).

rustls-webpki 0.103.15 (`verify_cert.rs:66-73`, `loop_while_non_fatal_error`
at `:757-774`) tries every anchor until one validates, so
accept/reject is order-insensitive — the drift is hygiene, not a
live trust failure. It is still a real parity defect: the fallback
store should not differ in shape from the path it stands in for,
and the test gap let it land.

## What Changes

- `fallback_root_store` builds custom-CA anchors FIRST (empty
  store, parse/add per strict policy), then extends with the
  bundled Mozilla anchors. Mozilla-only degrade paths (no CA,
  non-strict failures) keep returning a Mozilla-only store.
- The union unit test asserts ORDER, not just count: index of the
  first custom root < index of the first Mozilla anchor, plus
  exact-shape checks (custom anchor at position 0, tail equal to
  `TLS_SERVER_ROOTS`).
- A doc comment on the test cites the rustls/rustls-webpki source
  that makes order hygiene-only (verification semantics), and the
  rpv source that makes custom-first the parity target.
- Affected crate: `camel-component-http`. No API change; private
  function, same signature.

Excluded: any change to the primary verifier path, mTLS handling,
strict fail-closed logic, and `webpki_root_client_config`.

## Acceptance criteria

- Custom-root index 0 < first Mozilla-anchor index in the union
  store; union size unchanged (`TLS_SERVER_ROOTS.len() + 1`).
- No-CA and non-strict degrade results remain Mozilla-only with
  unchanged count and order (all-Mozilla).
- Strict fail-closed behavior and all existing fallback tests
  unchanged.
- Gate set green: `cargo fmt --check`, `cargo clippy -p
  camel-component-http -- -D warnings`, camel-http tests.

## Risk budget

Low. The reorder is provably observably neutral for handshake
accept/reject (anchor iteration tries all anchors). Acceptable
risk: none beyond test churn. Out of bounds: touching primary-path
TLS code, dependency upgrades, behavior changes beyond store
assembly order.
