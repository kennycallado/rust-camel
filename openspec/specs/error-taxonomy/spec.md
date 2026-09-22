# error-taxonomy Specification

## Purpose
TBD - created by archiving change sedamatch. Update Purpose after archive.
## Requirements
### Requirement: source-preserving endpoint-creation failure variant

The error taxonomy SHALL provide a source-preserving form of the
endpoint-creation failure: `EndpointCreationFailedWithSource(detail,
source)`, where `detail` is the user-facing diagnostic text and `source`
is an opaque owned handle (`OpaqueErrorSource`) carrying the typed cause.
The variant SHALL render exactly like the plain `EndpointCreationFailed`
(top-level Display `Endpoint creation failed: {detail}` remains
unchanged; chain-aware diagnostics that walk `source()` additionally gain
the marker's explicitly non-canonical source text — an intended,
documented addition, not drift), `variant_name()` SHALL
alias it to `EndpointCreationFailed` (doTry catch-by-variant
compatibility, same aliasing pattern as `ProcessorErrorWithSource`), and
`classify()` SHALL report `endpoint`. Producers that need downstream typed
classification of an endpoint-creation failure SHALL use this variant and
put their typed marker in the source chain instead of encoding meaning in
message text.

The source handle SHALL be opaque: `Error::source()` SHALL expose only the
pointee (the producer's marker as `&dyn Error`), the inner owned handle
SHALL be inaccessible outside camel-api, and the handle SHALL NOT be
publicly cloneable — so a marker carried by one error cannot be extracted
and replayed into a fabricated error by external code. Cloning the
top-level error SHALL preserve the marker (the clone carries another
handle to the same pointee).

#### Scenario: aliases match the plain variant

- **GIVEN** an `EndpointCreationFailedWithSource` with detail `d` and any
  source
- **WHEN** Display, `variant_name()`, and `classify()` are evaluated
- **THEN** rendering equals `Endpoint creation failed: d`,
  `variant_name()` reports `EndpointCreationFailed`, and `classify()`
  reports `endpoint`

#### Scenario: doTry catch-by-variant still catches

- **GIVEN** a doTry catch handler selecting the `EndpointCreationFailed`
  variant name
- **WHEN** an `EndpointCreationFailedWithSource` flows into the handler
- **THEN** the handler matches, because `variant_name()` aliases to
  `EndpointCreationFailed`

#### Scenario: source chain remains inspectable without extraction

- **GIVEN** an `EndpointCreationFailedWithSource` whose source is a typed
  marker error
- **WHEN** `source()` is walked and the marker probed by downcast
- **THEN** the marker is reachable as the pointee (no `Arc` wrapper hop on
  this variant), so ownership of the typed cause stays with the producing
  component; external code cannot clone the handle or name the marker

#### Scenario: cloning the error preserves provenance

- **GIVEN** a source-preserving endpoint-creation failure carrying a typed
  marker
- **WHEN** the error is cloned
- **THEN** the clone still carries the marker, so classification of the
  clone is identical to the original

