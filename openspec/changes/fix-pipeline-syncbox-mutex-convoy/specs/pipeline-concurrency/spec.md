## ADDED Requirements

### Requirement: Lock-free per-exchange pipeline acquisition

The runtime SHALL acquire the per-exchange pipeline handle without acquiring any blocking
mutex shared between exchanges of the same route.

#### Scenario: Concurrent exchanges do not serialize on pipeline acquisition

- **GIVEN** a route pipeline snapshot held in a shared `Arc<ArcSwap<PipelineAssembly>>` and 64 concurrent tasks each repeatedly acquiring the pipeline handle via `clone_inner()` with non-trivial hold work between acquisitions
- **WHEN** all tasks complete their acquisitions and work
- **THEN** the total wall-clock stays under a ceiling that a serializing (single-mutex) acquisition path would exceed, demonstrating acquisitions proceed in parallel

#### Scenario: Erased pipeline type is shareable across threads

- **GIVEN** the type-erased pipeline type `BoxProcessor`
- **WHEN** a static (compile-time) assertion checks the trait bound
- **THEN** `BoxProcessor` implements `Send + Sync`, so no wrapper mutex is ever needed to share a pipeline snapshot between threads

### Requirement: Per-exchange snapshot isolation during hot reload

The runtime SHALL preserve ADR-0004 snapshot isolation: each in-flight exchange runs to
completion on the pipeline snapshot it acquired, even while new snapshots are stored, and
Sequential-topology loops SHALL observe the latest stored snapshot per envelope.

#### Scenario: Pipeline swap during concurrent acquisition yields coherent snapshots

- **GIVEN** concurrent tasks acquiring pipeline handles via `clone_inner()` from a shared `Arc<ArcSwap<PipelineAssembly>>`
- **WHEN** a writer repeatedly stores new `PipelineAssembly` snapshots into the ArcSwap
- **THEN** every acquired handle is a coherent pipeline (no torn state) and every acquisition completes without error

#### Scenario: Sequential topology picks up the latest snapshot per envelope

- **GIVEN** a Sequential-topology route loop that acquires the pipeline handle once per envelope
- **WHEN** a hot reload stores a new pipeline snapshot between two envelopes
- **THEN** the next envelope is processed by the newly stored snapshot

### Requirement: Pipeline handle clone cost stays bounded

The runtime's per-exchange pipeline handle clone SHALL stay cheap enough that per-exchange
cloning remains viable: a representative multi-step pipeline clone SHALL complete under a
generous fixed ceiling, acting as a regression tripwire against accidental deep-clone
reintroduction.

#### Scenario: Multi-step pipeline clone completes under the tripwire ceiling

- **GIVEN** a representative multi-step pipeline stack (identity steps composed like a real route)
- **WHEN** `clone_inner()` is timed over many iterations
- **THEN** the mean per-clone cost is below the tripwire ceiling, failing the test if a future change makes clones deep-copy the pipeline work
