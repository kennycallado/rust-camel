## ADDED Requirements

### Requirement: Step span names identify the DSL step

Step and segment spans SHALL be named `{route_id}:{label}`, where `label` is
derived at compile time from the originating `BuilderStep`: the kebab-case EIP
name for structural steps (`log`, `filter`, `split`, …) or the endpoint
component scheme for endpoint steps (`to:http`, `to:direct`, …). When no
label is derivable (anonymous processors), the name SHALL fall back to
`{route_id}:step-{index}`. Labels SHALL NOT include full URIs, headers, or
exchange payload data.

#### Scenario: Endpoint step is named by component scheme

- **GIVEN** a traced route with a `.to("direct:tree-sub")` step at index 1
- **WHEN** the pipeline processes an exchange
- **THEN** that step's span is named `{route_id}:to:direct`

#### Scenario: Structural step is named by EIP

- **GIVEN** a traced route with a splitter step
- **WHEN** the segment span opens
- **THEN** the segment span is named `{route_id}:split`

#### Scenario: Anonymous processor falls back to positional name

- **GIVEN** a traced route whose step 0 is an opaque `.process(closure)`
- **WHEN** the pipeline processes an exchange
- **THEN** step 0's span is named `{route_id}:step-0`

### Requirement: step_id attribute removed from step spans

Step and segment spans SHALL NOT carry a `step_id` attribute. The `step_index`
attribute SHALL remain as the positional attribute; the span name carries the
step identity.

#### Scenario: Attribute set excludes step_id

- **GIVEN** a traced step span
- **WHEN** its attributes are inspected
- **THEN** `step_index` is present and `step_id` is absent
