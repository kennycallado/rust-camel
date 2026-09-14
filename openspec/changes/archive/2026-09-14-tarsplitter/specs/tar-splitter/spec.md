## ADDED Requirements

### Requirement: TAR entry splitting

The system SHALL provide a TAR stream-split format that emits one Exchange per bounded regular-file entry.

#### Scenario: regular entries preserve archive order

- **GIVEN** a TAR containing multiple regular files in a known header order
- **WHEN** a route uses TAR stream splitting
- **THEN** one fragment is emitted per regular file in header order and each fragment has a zero-based TAR entry index

#### Scenario: non-regular entries are skipped

- **GIVEN** a TAR containing directories, symlinks, hard links, or device entries
- **WHEN** the archive is split
- **THEN** only regular files are emitted and no link target is followed

#### Scenario: empty archive emits no fragments

- **GIVEN** an empty or directory-only TAR with empty archives allowed
- **WHEN** the archive is split
- **THEN** the splitter emits zero fragments without touching the filesystem

### Requirement: TAR.GZ entry splitting

The system SHALL provide a TAR.GZ stream-split format that applies TAR entry splitting after bounded single-member GZIP decoding, with the decode bounded by the total decoded payload limit plus a dual-bounded TAR framing allowance: an entry-count-derived term capped at an absolute fail-closed ceiling, plus a constant base covering GNU longname and PAX extension blocks and end-of-archive blocks.

#### Scenario: compressed regular entries split

- **GIVEN** a valid single-member TAR.GZ containing regular files
- **WHEN** the route uses TAR.GZ stream splitting
- **THEN** the splitter emits the same bounded fragments and metadata as TAR splitting

#### Scenario: compressed input respects the input cap

- **GIVEN** a TAR.GZ whose compressed input exceeds the configured compressed-input limit
- **WHEN** the archive is split
- **THEN** splitting fails before decompression is accepted

### Requirement: archive split bounds and configuration

The system SHALL enforce configured entry-count, per-entry, total decoded, compressed-input, and entry-path length limits, and SHALL reject unknown splitter configuration fields. The total decoded limit SHALL account for aggregate regular-entry payload bytes and exclude TAR framing (headers and padding). The system SHALL apply the shared archive duplicate-name policy to TAR entries; this requirement deliberately does not claim ZIP parity, because the ZIP splitter's historical reader-level duplicate collapse is pinned existing behavior that this change does not modify.

#### Scenario: oversized entry is rejected

- **GIVEN** a regular TAR entry larger than the configured per-entry limit
- **WHEN** the entry is read
- **THEN** splitting fails with a bounded-size error and does not allocate the full entry

#### Scenario: total decoded counts payload bytes, not framing

- **GIVEN** a TAR.GZ whose aggregate regular-entry payload bytes stay within the configured total decoded limit while its TAR framing exceeds that limit
- **WHEN** the archive is split
- **THEN** the split succeeds and emits every regular entry, and an archive whose payload alone exceeds the limit fails with the total decoded error

#### Scenario: duplicate names follow the shared archive policy

- **GIVEN** duplicate TAR entry names and a configured duplicate policy
- **WHEN** the archive is split
- **THEN** the reject policy fails the split on the first duplicate, and the allow-with-index policy emits every entry with deterministic collision-free indexed names that never reuse an already-emitted name

### Requirement: entry-path confinement boundary

The system SHALL validate TAR entry names for length, absolute paths, and traversal, and SHALL not write or extract entries to disk in v1.

#### Scenario: unsafe path is rejected

- **GIVEN** an entry name containing an absolute path or a parent traversal component
- **WHEN** the archive is split
- **THEN** splitting fails closed with a path-validation error and no filesystem path is accessed

#### Scenario: future extraction remains confined

- **GIVEN** a future implementation maps an entry name to a filesystem destination
- **WHEN** it resolves that destination
- **THEN** it uses the nearest-existing-ancestor confinement pattern from rc-0ks57 rather than trusting the archive name

### Requirement: split aggregation contract

The system SHALL document that TAR splitting and ordinary marshal are not inverse operations.

#### Scenario: original aggregation preserves source exchange

- **GIVEN** a route splits a TAR and uses `AggregationStrategy::Original`
- **WHEN** fragments complete
- **THEN** the original parent Exchange is retained; the system does not claim to reassemble a TAR archive

## ADDED Requirements

### Requirement: stream split format selection

The system SHALL expose TAR and TAR.GZ in the existing stream-split format contract and compiler while preserving ZIP behavior.

#### Scenario: compiler selects TAR format

- **GIVEN** a stream split configuration naming TAR
- **WHEN** the route is compiled
- **THEN** the compiler constructs the TAR splitter without changing ZIP selection or semantics
