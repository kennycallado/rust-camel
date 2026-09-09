# Proposal: ps97b-receive

## Why

bd rc-ps97b (P2, discovered during the rc-2miu multi-path example work): a
DYNAMIC-reference server-role receive (`from: http://${MOCK}/billing`)
resolves its partner by authority but drains the lane of the REGISTERED
key's path (`/orders`) — `PartnerRouter::receive` passes the registered key
to `await_arrival`, which parses the lane path from it. Wire dials preserve
the path (`wire_target`); arrival lanes key on the strict wire
`path_and_query`; only server-role receive lane selection diverges. The
one-listener/N-paths pattern (mocks.py migration) currently requires a
workaround (path-filtered count validates) and a sibling-path receive times
out while its own lane holds the arrival.

## What Changes

- **integration-tier spec** (delta): MODIFIED requirement "Wire-fidelity
  lane key and mismatch diagnostics" — the server-role receive derives its
  lane from the INTERPOLATED reference's own path (and query), aligning
  receive lane selection with the wire-fidelity canon; bare-authority
  dynamic receives fail apparatus-class naming the declaration; the
  client-role roundtrip parking stays path-blind (documented explicitly;
  path-aware keying deferred to bd rc-cr5yf).
- **Code** (`crates/camel-integration-test`): `PartnerRouter::receive`
  threads the interpolated URI into the adapter receive so
  `await_arrival` parses the lane path from the interpolated reference,
  NOT the registered key (the registered key remains the ADAPTER LOOKUP
  key); doc comments updated.
- **Docs/example flip**: docs/src/testing/index.md:337 passage,
  CONTEXT.md:136 arrival-lane entry, and the
  partner-multi-path example comments/structure upgrade to the canonical
  TWO-dynamic-receives pattern (the originally-blessed rc-2miu shape) —
  the sibling-path limitation text is removed.

## Zones

`crates/camel-integration-test/src/{adapters.rs,adapters/http.rs,runner.rs}`
(doc comments only unless the seam demands), `docs/src/testing/index.md`,
`examples/integration-testing/partner-multi-path.*`,
`crates/camel-integration-test/{CONTEXT.md}`,
`openspec/changes/ps97b-receive/**`.

A parallel fleet member works validate-sql (mock-testkit-adjacent): this
change touches only integration-tier + itest — additive+additive, rebase
discipline applies.

## Out of Scope

- Path-aware client-lane roundtrip keying (bd rc-cr5yf, papal Q2 deferral).
- camel-dsl, camel-http, camel-sql, DatasourceCatalog — untouched.

## bd

Closes rc-ps97b. Filed rc-cr5yf (Q2 deferral, P3).
