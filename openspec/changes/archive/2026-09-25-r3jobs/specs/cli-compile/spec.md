## ADDED Requirements

### Requirement: Compile and boot multi-entry job artifacts

A job entry document whose file-form route source (`routeFiles` or `routeFilesFromRoot`) resolves to one or more route documents SHALL compile into a v2 virtual-store artifact embedding the job document, every resolved route document, and the selected configuration chain; the artifact SHALL boot all indexed route entries through the existing job outcome lifecycle with no source tree, no extraction, and no filesystem route discovery. The job source plan SHALL contain exactly the job document and its own file-form expansions: configuration-declared `routes` patterns SHALL NOT add entries to a job artifact's source plan, mirroring the `camel job` exactly-one-source rule, and overlapping configuration patterns SHALL NOT fail a job compile that `camel job` accepts. A file-form job route source that resolves zero route documents SHALL fail compilation with a diagnostic naming the document and the resolved-zero rule, before any output byte is created. Route-entry consumption order SHALL be the source-plan order — declared pattern order, each pattern's matches sorted by normalized logical path — and identical input sets SHALL produce byte-identical artifacts. A job artifact boot in which any embedded route entry fails parse or validation SHALL exit 2 with a diagnostic naming that entry's `compiled://<logical-path>` identity before any route starts; no partial boot SHALL occur. The operational manifest SHALL list every route entry of a multi-entry job artifact with its canonical logical path, kind, byte length, and digest, alongside the job and configuration entries.

#### Scenario: Multi-entry job artifact boots every entry

- **GIVEN** a compiled job artifact embedding a job document and three or more route documents forming a direct-endpoint chain across files, deployed without its source tree or configuration
- **WHEN** the artifact starts with `--report <path>`
- **THEN** it exits 0, every embedded route entry participates in the boot, the one-shot send traverses the chain across all files, and the report records outcome `Completed` with the composed reply body and the `compiled://<job-path>` document identity

#### Scenario: Entry order follows the source plan

- **GIVEN** a job document declaring route-file patterns in non-sorted order where one pattern matches multiple files
- **WHEN** the document is compiled
- **THEN** the source plan preserves declared pattern order with each pattern's matches sorted by normalized logical path, the store packs entries in canonical path order, and two compiles of the identical input set produce byte-identical artifacts

#### Scenario: Failing entry is named at boot with no partial boot

- **GIVEN** a compiled job artifact in which one embedded route document among several is structurally invalid, which compilation deliberately allows
- **WHEN** the artifact starts
- **THEN** it exits 2 with a diagnostic naming the invalid entry's `compiled://<logical-path>` identity before any route starts, and no job report or outcome is produced

#### Scenario: Manifest lists every route entry

- **GIVEN** a compiled multi-entry job artifact embedding a job document and several route documents
- **WHEN** the artifact runs `--manifest`
- **THEN** it exits 0 without booting and lists every route entry with canonical logical path, `route` kind, byte length, and digest, alongside the `job` entry and configuration entries, under `artifact_kind: "job"`

#### Scenario: Configuration route patterns do not widen the job plan

- **GIVEN** a job document declaring `routeFiles` entries that a configuration `routes` pattern in the selected `Camel.toml` also matches, a document set `camel job` accepts and boots
- **WHEN** the operator compiles the job document with that configuration
- **THEN** compilation succeeds, and the source plan contains exactly the job document and its own declared route-file expansions — the configuration pattern adds no entry and the overlap is not a duplicate-source rejection

#### Scenario: Zero-route-entry job source rejected at compile

- **GIVEN** a job document whose file-form route source patterns resolve zero route documents, a document `camel job` rejects as a job-safety violation
- **WHEN** the operator compiles the job document
- **THEN** compilation fails with exit 2, a diagnostic names the document and the resolved-zero rule, and no artifact is created

## MODIFIED Requirements

### Requirement: Resolve and confine compile-time sources

The compiler SHALL normalize source names to UTF-8 relative paths using `/`, anchor resolution to the explicitly selected `Camel.toml` root (or the primary document directory when no configuration root is required), and reject absolute paths, empty or `.` components, `..` traversal, non-UTF-8 names, symlink escapes, missing sources, and references outside the root. Asset references — document fields, TLS URI parameters, endpoint-URI operands, and `Camel.toml` declarations — SHALL obey the same confinement as route sources. Static directories SHALL expand to individual regular-file entries through a deterministic sorted walk; a symlink encountered inside a static tree SHALL fail compilation with a named confinement diagnostic rather than being ignored, and non-regular files other than symlinks are skipped. Two references resolving to one canonical target SHALL embed one shared asset entry rather than failing, and every alias spelling SHALL receive a substitution-table entry mapping to that single entry. Route-source duplicate canonical targets keep failing closed. For job entry documents, configuration-declared `routes` patterns are not route sources: they add no source-plan entries, and the duplicate rule applies within the job document's own declared route-file family — an overlap between a configuration pattern and a job document's declared route files is not a duplicate-source rejection (the `camel job` exactly-one-source parity). Resolution SHALL be deterministic and SHALL not depend on ambient current-directory discovery.

#### Scenario: Path escape fails before output

- **GIVEN** a route source pattern, include, or asset reference that resolves outside the selected root, including through a symlink
- **WHEN** compilation runs
- **THEN** it exits 2 with a confinement diagnostic and does not create a usable artifact

#### Scenario: Symlink inside a static tree fails closed

- **GIVEN** a `static_dir` tree containing an entry that is a symbolic link
- **WHEN** compilation runs
- **THEN** it exits 2 naming the symlink path and does not silently skip or follow it

#### Scenario: Overlapping sources fail closed

- **GIVEN** two route-file patterns within one route-source family that resolve the same canonical source, or two names that normalize to the same logical path
- **WHEN** compilation runs
- **THEN** it exits 2 naming the duplicate and does not silently embed two copies; for job entry documents a configuration `routes` pattern is outside the job's route-source family and never triggers this rejection

#### Scenario: Shared asset targets deduplicate through the substitution table

- **GIVEN** two TLS blocks referencing the same client-CA file through different relative spellings
- **WHEN** compilation runs
- **THEN** the store contains one asset entry for the canonical target and the index substitution table maps both alias spellings to that single entry

#### Scenario: Static directory expansion is deterministic

- **GIVEN** a `static_dir` tree whose filesystem enumeration order varies
- **WHEN** compilation runs twice
- **THEN** both runs embed the same regular-file asset entries in the same canonical order and contain no entries for non-regular files

#### Scenario: Deterministic ordering is stable

- **GIVEN** the same source tree presented with filesystem enumeration in different orders
- **WHEN** compilation runs twice
- **THEN** the store index, source plan, embedded bytes, and artifact digest are identical

#### Scenario: Store offsets and schema are validated

- **GIVEN** a marked artifact with an unknown `store_schema`, an out-of-bounds or overlapping content range, a missing source-plan target, noncanonical index ordering, unreferenced content, or a substitution entry naming a nonexistent document or asset
- **WHEN** the artifact starts
- **THEN** it exits 2 before boot with a format diagnostic and does not reinterpret the bytes as a different store schema
