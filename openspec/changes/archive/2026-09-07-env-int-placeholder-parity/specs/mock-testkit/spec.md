# mock-testkit Delta — env-int-placeholder-parity (rev 2: wave-E canon)

## ADDED Requirements

### Requirement: LEAN tier resolves env placeholders in route sources via default-only lookup with boot parity

The unit-tier runner (`camel test --unit`) MUST load route sources — `routeFiles`,
`routeFilesFromRoot`, and inline `routes:` — after interpolating `${env:NAME:-default}`
placeholders with a default-only lookup that MUST NOT read the ambient environment
(ADR-0069 §13.1 global-state flake class), using the SAME interpolation strategy and
typing semantics as the route loader (`camel run` boot parity): string-typed fields
interpolate; integer-typed fields carrying a placeholder fail to load. Unresolved
no-default placeholders in value positions MUST fail the document with an error naming
the variable, mirroring the boot path's `DiscoveryError::Env` wording.

#### Scenario: file route with string-field placeholder loads under LEAN

- **Given** a `.test.yaml` whose `routeFiles` reference a route with a string-typed
  field `title: ${env:LEAN_T:-hello}`
- **When** the document's routes load
- **Then** the route loads with `title == "hello"` and no document error occurs

#### Scenario: file route with integer-field placeholder fails with doc_error (boot parity)

- **Given** a `.test.yaml` whose `routeFiles` reference a route with
  `circuit_breaker.open_duration_ms: ${env:CB_MS:-750}`
- **When** the document's routes load
- **Then** loading fails with a document error — the substituted leaf keeps string
  typing exactly as `camel run` would reject the same file; LEAN never accepts a route
  the boot path rejects

#### Scenario: inline routes with string-field placeholder load under LEAN

- **Given** a `.test.yaml` with inline `routes:` containing a string-typed field with
  `${env:P:-one}`
- **When** the document's routes load
- **Then** the route loads with the interpolated value and no document error occurs

#### Scenario: unresolved no-default placeholder fails the document naming the variable

- **Given** a route file referenced by a `.test.yaml` containing
  `title: ${env:LEAN_UNDEF}` in a value position with no ambient value
- **When** the document's routes load
- **Then** the result is an error string containing `LEAN_UNDEF` (boot-parity
  wording), not a serde type error

#### Scenario: LEAN ignores ambient environment

- **Given** ambient `LEAN_T=ambient` set in the process environment and a route with
  `title: ${env:LEAN_T:-hello}`
- **When** the document's routes load with ambient values present
- **Then** the field is still `"hello"` (default-only lookup; hermetic determinism)
