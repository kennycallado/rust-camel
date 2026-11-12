## ADDED Requirements

### Requirement: REST error mapping inheritance

A `rest:` block SHALL accept an `error_mapping` field carrying the same
kind-to-status and code-to-status tables as the route-level mapping, validated
by the same load-time rules (exact non-blank keys, 400-599 values only). The
field lowers into the separate route-level `http_error_mapping` field of each
lowered operation route (`RouteDslRoute.http_error_mapping`) — the lowered
routes keep `error_handler: None` exactly as today; the mapping never rides
the error handler and never enables retry, dead-letter, or `on_exceptions`
behavior. Rest lowering SHALL always set `http_error_mapping: Some(_)` on its
lowered routes — with empty tables when the block declares none — marking them
as seam routes where the business dead-letter bypass applies (review C1); a
`rest:` block without `error_mapping` therefore still maps business errors
through the built-in default kind map and the 500 closure. Every operation of the block maps business errors through
the one block-level table, without per-operation branches. Plain-route YAML
authoring of the mapping is deferred to the phase-3 `errors:` catalog; the
rest block is the only document-level authoring path in this scope (owner
decision 2026-11-12).

#### Scenario: rest block mapping reaches every operation route

- **GIVEN** a `rest:` block with `error_mapping: {kinds: {"not-found": 410}}`
  and two operations whose pipelines raise business errors with kind
  `not-found` and kind `conflict`
- **WHEN** each operation runs over HTTP
- **THEN** the `not-found` operation replies 410 from the explicit block table
  and the `conflict` operation replies 409 from the built-in default kind map —
  both from the single block-level configuration

#### Scenario: rest lowering keeps the error handler untouched

- **GIVEN** a `rest:` block with `error_mapping` configured
- **WHEN** the block lowers its operation routes
- **THEN** each lowered route carries `error_handler: None` exactly as today
  and its mapping on the separate route-level `http_error_mapping` field; no
  retry policy, dead-letter channel, or `on_exceptions` configuration is
  introduced by the mapping

#### Scenario: rest block without mapping keeps today's behavior

- **GIVEN** a `rest:` block without `error_mapping`
- **WHEN** the block lowers its operation routes and an operation pipeline
  raises `Business { kind: "failed", .. }`
- **THEN** the lowered routes carry `http_error_mapping: Some(_)` with empty
  tables (seam marking) and `error_handler: None`, and the reply has status
  500 from the built-in default kind map with no per-operation configuration
