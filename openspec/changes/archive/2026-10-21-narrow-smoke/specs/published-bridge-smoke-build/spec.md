## ADDED Requirements

### Requirement: Narrow CXF compilation

camel-test SHALL provide `integration-tests-cxf` that enables only the optional CXF component and compiles the existing CXF target and required support helpers without the full integration feature. The full feature SHALL retain its existing behavior.

#### Scenario: Narrow target

- **GIVEN** only `integration-tests-cxf` is enabled
- **WHEN** the CXF test target is compiled and listed
- **THEN** compilation succeeds and the existing published-smoke tests remain present

#### Scenario: Full target compatibility

- **GIVEN** `integration-tests` is enabled
- **WHEN** the CXF target is compiled
- **THEN** compilation succeeds with its existing tests and support

### Requirement: Published smoke budget

The published bridge workflow SHALL use the narrow feature without changing artifact acquisition, checksum verification, or its three end-to-end assertions. It SHALL lower `timeout-minutes` from 150 to 60 only if a narrow no-run build classified as cold completes in at most 25 minutes. Otherwise it SHALL keep 150, and the mission report SHALL state the reason.

#### Scenario: Workflow selection

- **GIVEN** the published bridge workflow and a cold narrow build completed in at most 25 minutes
- **WHEN** its smoke command runs
- **THEN** it selects `integration-tests-cxf`, sets a 60-minute timeout, and retains the existing filters and assertions

#### Scenario: Timeout retained

- **GIVEN** a narrow measurement that is only empty-target with a warm compiler cache, or takes more than 25 minutes
- **WHEN** the workflow is updated
- **THEN** it selects `integration-tests-cxf`, retains the 150-minute timeout, and the mission report explains why the timeout was not lowered

### Requirement: Measured build evidence

The change SHALL record elapsed time and exit status for a narrow CXF no-run build starting with a dedicated empty target directory, including cache and containment conditions. A cold measurement SHALL disable both compiler wrappers and verify no sccache hits during the run; otherwise the report SHALL label it empty-target, sccache-warm with hit/miss changes. Any full-build comparison SHALL state whether it is cold, incremental, or historical. The mission report and bd notes SHALL record the remaining unconditional dependencies, including kube, sqlx, and bollard, without crediting their removal to the narrow feature.

#### Scenario: Report without an affordable cold baseline

- **GIVEN** a completed narrow measurement and an incremental full-feature compatibility build
- **WHEN** the mission report is written
- **THEN** it distinguishes both measurements from historical CI timeouts and does not claim an unmeasured speedup ratio
