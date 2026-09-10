# mock-testkit delta — env-int-placeholder-typing

## MODIFIED Requirements

### Requirement: LEAN tier resolves env placeholders in route sources via the document env layer with boot parity

The unit-tier runner (`camel test --unit`) MUST load route sources — `routeFiles`,
`routeFilesFromRoot`, and inline `routes:` — after interpolating `${env:NAME:-default}`
placeholders with a lookup that consults the document `env:` map first, then the
inline default, and that MUST NOT read the ambient environment (ADR-0069 §13.1
global-state flake class), using the SAME interpolation strategy and typing
semantics as the route loader (`camel run` boot parity): string-valued fields
interpolate; integer-typed positions carrying a whole-scalar
placeholder load through the loader's typed probe — including when
the document `env:` map supplies the value (the coerced value parses as
the boot path's parser accepts). LEAN
accepts exactly what the boot path accepts, no more. Unresolved no-default
placeholders in value positions — not in the document `env:` map either — MUST fail
the document with an error naming the variable, mirroring the boot path's
`DiscoveryError::Env` wording.

#### Scenario: file route with string-field placeholder loads under LEAN

- **Given** a `.test.yaml` whose `routeFiles` reference a route with a `set_header`
  step value `${env:LEAN_T:-hello}`
- **When** the document's routes load
- **Then** the route loads with the header value `"hello"` and no document error occurs

#### Scenario: document env value steers a route-file string field before the default

- **Given** a `.test.yaml` declaring `env: { LEAN_T: docval }` whose `routeFiles`
  reference a route with a `set_header`
  step value `${env:LEAN_T:-hello}`
- **When** the document's routes load
- **Then** the route loads with the header value `"docval"` — the document layer wins over
  the inline default

#### Scenario: inline routes with string-field placeholder load under LEAN

- **Given** a `.test.yaml` with inline `routes:` containing a `set_header` step value
  `${env:P:-one}` and no `env:` map
- **When** the document's routes load
- **Then** the route loads with the header value `"one"` and no document error occurs

#### Scenario: document env value steers an inline-routes string field

- **Given** a `.test.yaml` declaring `env: { P: two }` with inline `routes:`
  containing a `set_header` step value `${env:P:-one}`
- **When** the document's routes load
- **Then** the route loads with the value `"two"` and no document error occurs

#### Scenario: no-default placeholder resolves from the document env map

- **Given** a `.test.yaml` declaring `env: { LEAN_DEF: supplied }` whose route file
  contains a `set_header` step value `${env:LEAN_DEF}`
- **When** the document's routes load
- **Then** the route loads with the header value `"supplied"` — no unresolved-variable
  error

#### Scenario: file route with integer-field placeholder loads via the typed probe

- **Given** a `.test.yaml` whose `routeFiles` reference a route with
  `circuit_breaker.open_duration_ms: ${env:CB_MS:-750}`
- **When** the document's routes load
- **Then** the route loads with `open_duration_ms == 750` — the loader's
  typed probe coerces the whole-scalar leaf exactly as `camel run`
  does (boot parity: LEAN accepts what the boot path accepts)

#### Scenario: integer-field placeholder loads when the document env supplies the value

- **Given** a `.test.yaml` declaring `env: { CB_MS: "500" }` whose `routeFiles`
  reference a route with `circuit_breaker.open_duration_ms: ${env:CB_MS:-750}`
- **When** the document's routes load
- **Then** the route loads with `open_duration_ms == 500` — the document env value
  flows through the same probe regardless of the value's source (boot parity)

#### Scenario: inline routes with integer-field placeholder load under LEAN

- **Given** a `.test.yaml` with inline `routes:` containing a route step
  `throttle: {max_requests: ${env:CB_MS:-750}}` and no `env:` map
- **When** the document's routes load
- **Then** the route loads with `max_requests == 750` — the inline branch
  routes through the same interpolation seam, so the probe applies
  (boot parity with the file forms)

#### Scenario: file route with integer-field placeholder fails with doc_error (boot parity)

Supersession note: before the typed probe, every integer-field placeholder
failed here. Whole-integer defaults now load — see the preceding scenarios.
The failure surface that remains:

- **Given** a `.test.yaml` whose `routeFiles` reference a route with
  `circuit_breaker.open_duration_ms: ${env:CB_MS:-soon}`
- **When** the document's routes load
- **Then** loading fails with a document error exactly as `camel run` rejects
  the same value; LEAN accepts what the boot path accepts (boot parity)

#### Scenario: integer-field placeholder fails even when the document env supplies the value

Supersession note: numeric document-env values now flow through the typed
probe and load. The failure surface that remains — a non-numeric value
fails regardless of its source:

- **Given** a `.test.yaml` declaring `env: { CB_MS: "soon" }` whose `routeFiles`
  reference a route with `circuit_breaker.open_duration_ms: ${env:CB_MS:-750}`
- **When** the document's routes load
- **Then** loading fails with a document error — the probe never coerces
  non-numeric leaves; the unit tier resolves exactly what the boot tier
  resolves (boot parity)

#### Scenario: unresolved no-default placeholder fails the document naming the variable

- **Given** a route file referenced by a `.test.yaml` containing
  a `set_header` step value `${env:LEAN_UNDEF}`, with no `LEAN_UNDEF` entry in
  the document `env:` map
- **When** the document's routes load
- **Then** the result is an error string containing `LEAN_UNDEF` (boot-parity
  wording), not a serde type error

#### Scenario: LEAN ignores ambient environment

- **Given** ambient `LEAN_T=ambient` set in the process environment and a route
  with a `set_header` step value `${env:LEAN_T:-hello}`, no `env:` map in the document
- **When** the document's routes load with ambient values present
- **Then** the header value is still `"hello"` (document env absent, default applies;
  ambient never consulted; hermetic determinism)
