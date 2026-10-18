## ADDED Requirements

### Requirement: Inline schema cache is bounded

The xml-bridge compiled-schema cache SHALL evict evictable (unpinned) entries so that
inserting more distinct inline schemas than the configured bound does not grow the cache
beyond the bound, exclusive of entries pinned by registered schema sets.

#### Scenario: Inline inserts beyond the bound stay bounded

- **GIVEN** a cache bound of N entries and no registered schema sets
- **WHEN** more than N distinct inline schemas are compiled into the cache through the
  inline validate path
- **THEN** the cache size stays at or below N and eviction is observable through the
  eviction counter and log

### Requirement: Registered schema-set entries are pinned against eviction

The cache SHALL NOT evict a compiled schema whose digest is still referenced by a
registered schema-set id, even under eviction pressure; the digest becomes evictable only
after its last registered id is unregistered or remapped.

#### Scenario: Registered digest survives eviction pressure

- **GIVEN** a schema set registered under a public id and a cache bound of N entries
- **WHEN** more than N distinct inline schemas are inserted
- **THEN** the registered digest's compiled schema is still retrievable and a
  validate-with request for that id still succeeds

#### Scenario: Unregistered digest becomes evictable

- **GIVEN** a registered digest that survived earlier eviction pressure
- **WHEN** the id is unregistered and further inserts apply eviction pressure
- **THEN** the digest is evictable like any inline entry and the cache remains within the
  bound over the unpinned population

### Requirement: Cache bound is configurable

The maximum number of cache entries SHALL be configurable with a documented default of
256, and a non-positive configured bound SHALL fail startup rather than silently
disabling the bound.

#### Scenario: Configured bound is honored

- **GIVEN** a configuration that sets the bound to a small positive value
- **WHEN** the bridge compiles more distinct schemas than that value
- **THEN** eviction enforces exactly that bound

#### Scenario: Non-positive bound fails startup

- **GIVEN** a configuration that sets the bound to zero or a negative value
- **WHEN** the bridge starts
- **THEN** startup fails with a configuration error naming the invalid setting

### Requirement: Evictions are observable

Each eviction SHALL be logged and counted in a monotonically increasing eviction counter
that is readable by tests and operations.

#### Scenario: Eviction produces a log line and increments the counter

- **GIVEN** a cache at its configured bound
- **WHEN** a new distinct schema is inserted
- **THEN** at least one eviction is logged and the eviction counter increases
