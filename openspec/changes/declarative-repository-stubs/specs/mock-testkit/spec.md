## ADDED Requirements

### Requirement: Declarative repository stubs

`camel test` SHALL support a `repositories:` block in the test document with
three optional maps — `cache`, `idempotent`, `claimCheck` — each mapping a
repository name to the stub target `memory`. The runner SHALL register a
`MemoryCacheRepository`, `MemoryIdempotentRepository`, or
`MemoryClaimCheckRepository` under each declared name before routes are
added and compiled, so routes whose steps reference named repositories
(`cache:`, `cache_invalidate:`, `cache_clear:`, `cache_peek_stale:`,
`cache_stats:`, idempotent consumer, claim check) compile and run against
in-memory stubs. Validation SHALL be eager at parse time: unknown registry
kinds, unknown stub targets, blank repository names, and use of the
built-in name `memory` as a stub name SHALL be document errors (exit 2).
Only the literal target `memory` SHALL be valid. Repository names that the
document does not declare SHALL continue to fail route compilation with the
same unknown-repository error production uses.

A stub is lossy by design: a green run under `memory` proves
backend-agnostic decision logic only, and it bypasses production
`Camel.toml` repository registration, so it cannot validate missing or
invalid backend configuration. Surfaces a stub does NOT exercise, by
registry: for cache stubs, prefix purge (`invalidate_prefix` — memory
fails closed, redb range-deletes, redis SCAN+UNLINK), backend-specific
TTL/stale-retention timing fidelity, disk-offload decorator behavior,
backend-specific `stats` fidelity, and backend-failure error paths; for
idempotent and claim-check stubs, persistence semantics and backend-failure
error paths. Coverage of those surfaces belongs to the integration tier.

#### Scenario: cache stub compiles a named-repository route

- **Given** a route `from("direct:in").cache(repository: "persistent",
  key: "k").to("mock:out")` and a test document declaring
  `repositories: { cache: { persistent: memory } }` with two inputs for
  the same key
- **When** `camel test` runs the document
- **Then** the route compiles, the first input takes the miss path, the
  second takes the hit path, and the mock expectations pass

#### Scenario: idempotent stub filters duplicate inputs

- **Given** a route with an idempotent consumer referencing repository
  `redis` and a test document declaring
  `repositories: { idempotent: { redis: memory } }`, with two duplicate
  inputs (same message id) delivered
- **When** `camel test` runs the document
- **Then** the route compiles and the downstream mock endpoint receives
  exactly one exchange — the duplicate is filtered by the in-memory
  idempotent repository

#### Scenario: claimCheck stub round-trips content

- **Given** a test document declaring
  `repositories: { claimCheck: { redb: memory } }` for a route that
  claim-checks a body to the repository `redb` and later restores it
- **When** `camel test` runs the document
- **Then** the route compiles and the restored body equals the checked-in
  body, asserted through the mock expectation

#### Scenario: undeclared repository name still fails route load

- **Given** a route referencing repository `persistant` (typo) and a test
  document declaring `repositories: { cache: { persistent: memory } }`
- **When** `camel test` runs the document
- **Then** route loading fails with the unknown-repository error naming the
  step and the repository, as a document error (exit 2) — identical to a
  run without the `repositories:` block

#### Scenario: unknown registry kind is a document error

- **Given** a test document with `repositories: { blob: { x: memory } }`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 rejecting the unknown field and
  listing the supported registry kinds

#### Scenario: unknown stub target is a document error

- **Given** a test document with
  `repositories: { cache: { persistent: rocksdb } }`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 naming the unsupported target and
  listing the supported target `memory`

#### Scenario: blank repository name is a document error

- **Given** a test document with `repositories: { cache: { "  ": memory } }`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating repository names must be
  non-blank

#### Scenario: stubbing the built-in memory name is a document error

- **Given** a test document with `repositories: { cache: { memory: memory } }`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating `memory` is a built-in
  repository name and cannot be stubbed

#### Scenario: stubbing emits a per-run warning

- **Given** a test document declaring any repository stub
- **When** `camel test` runs the document
- **Then** the run emits a stderr warning with the code
  `R-REPOSITORY-STUB` naming each stubbed registry and repository name, and
  stating that backend semantics (for cache: prefix purge, TTL/stale
  timing, disk offload, stats; for idempotent/claim-check: persistence) and
  backend-failure paths are not exercised and belong to the integration
  tier

#### Scenario: camel run ignores the repositories block

- **Given** a project whose route files are loaded by `camel run` and whose
  test documents declare `repositories:` blocks
- **When** `camel run` boots from the same project
- **Then** runtime repository registration is driven solely by
  `Camel.toml` as before — the block lives only in test documents, which
  `camel run` never parses
