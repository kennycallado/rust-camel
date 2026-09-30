## ADDED Requirements

### Requirement: Executable emission proof for artifact-gated components

The wasm `invoke` and cxf `consume` facade wiring SHALL be backed by
executable in-crate emission proof that requires no external artifacts
(no native bridge binary; only committed test fixtures), driven through
the real production paths (endpoint creation and producer call for wasm;
consumer task and response handling for cxf) against a recording
`RuntimeObservability` double published in camel-component-api test
support. The `AUDIT` const in `crates/camel-test/tests/component_emission_test.rs`
SHALL remain the authoritative swept-operations table with every entry
backed by executable proof.

#### Scenario: wasm invoke success leg

- **GIVEN** a recording RuntimeObservability double with the components
  lever on, and the committed echo guest copied into a temp base dir
- **WHEN** a wasm endpoint created through `WasmComponent::create_endpoint`
  produces an exchange whose producer call completes successfully
- **THEN** the double records the component operation
  `("wasm", "invoke", "success")` and no error-family increment

#### Scenario: wasm invoke failure leg

- **GIVEN** the same wiring pointed at a file that is not a valid WASM
  module, with the components lever on
- **WHEN** the producer call fails
- **THEN** the double records `increment_errors("wasm", "e:wasm:invoke")`
  on the never-gated error family, and the component operation
  `("wasm", "invoke", "failure")`

#### Scenario: wasm invoke failure with the lever off

- **GIVEN** the same failing wiring with the components lever off
- **WHEN** the producer call fails
- **THEN** the double still records
  `increment_errors("wasm", "e:wasm:invoke")` and records no component
  operation at all

#### Scenario: cxf consume success leg via mock bridge

- **GIVEN** a recording double with the lever on and a real
  `CxfConsumer` started against the in-test mock bridge with a ready
  bridge slot
- **WHEN** the route answers a consumed request without fault
- **THEN** the double records `("cxf", "consume", "success")` and no
  error-family increment

#### Scenario: cxf consume route-error leg

- **GIVEN** the same wiring with a route handler that rejects the
  exchange, with the components lever on
- **WHEN** the consumer emits the fault response
- **THEN** the double records `increment_errors("cxf", "e:cxf:consume")`
  and the component operation `("cxf", "consume", "failure")`

#### Scenario: cxf consume failure with the lever off

- **GIVEN** the same route-error wiring with the components lever off
- **WHEN** the consumer emits the fault response
- **THEN** the double still records
  `increment_errors("cxf", "e:cxf:consume")` and records no component
  operation at all

#### Scenario: cxf marshalling failure retains the b-prime label

- **GIVEN** the same wiring (lever on) with a consumer route id distinct
  from the component name (for example `emission-marshalling-leg`) and a
  route handler that returns a stream output body (unmarshallable by the
  consumer)
- **WHEN** the response marshalling fails
- **THEN** the double records exactly one route-scoped error
  `increment_errors("<route-id>", "b-prime:cxf:response-marshalling")`
  under the consumer's route id, plus the component-scoped
  `increment_errors("cxf", "e:cxf:consume")` and one component operation
  `("cxf", "consume", "failure")` — the intended design-D5 double-count
  where the two error labels differ and land under different first
  labels (route id vs component)

#### Scenario: AUDIT table stays authoritative

- **WHEN** the camel-test component emission suite runs
- **THEN** the `AUDIT` const still lists exactly the swept
  component-operation pairs including wasm `invoke` and cxf `consume`,
  and the module documentation points to the crate-internal proofs for
  both entries instead of claiming no honest harness exists
