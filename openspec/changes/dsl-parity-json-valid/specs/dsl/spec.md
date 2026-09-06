## ADDED Requirements

### Requirement: Non-printable character rejection parity

The DSL front-ends SHALL keep format-correct behavior for route documents
whose raw text contains characters outside the YAML printable set: the JSON
front-end SHALL accept such a document when it is JSON-valid, and the YAML
front-end SHALL reject it with a format-annotated error.

#### Scenario: raw DEL byte in a JSON-valid document

- Given a routes document that serde_json accepts and whose raw text contains
  U+007F
- When the document is parsed through the JSON front-end
- Then parsing succeeds
- When the document is parsed through the YAML front-end
- Then parsing fails with an error prefixed `YAML DSL error:`

#### Scenario: escaped DEL stays strict-parity

- Given a routes document that carries DEL only in escaped form (the six ASCII
  bytes `\u007f`) and contains no raw non-printable character
- When the document is parsed through both front-ends
- Then both front-ends succeed and produce equal route steps

### Requirement: dsl_parity oracle carve-out for the non-printable class

The dsl_parity fuzz oracle SHALL treat a YAML rejection of a JSON-valid
document as expected behavior when the raw document contains characters
outside the YAML printable set, and SHALL panic on every other YAML rejection.

#### Scenario: minimized assurance input does not panic

- Given the minimized document from assurance run 33984285881 containing a raw
  U+007F inside a JSON string
- When the dsl_parity harness consumes it
- Then the harness asserts the YAML front-end rejection and returns without
  panic

#### Scenario: printable rejection still panics

- Given a document that serde_json accepts, whose raw text is fully printable,
  and that the YAML front-end rejects
- When the dsl_parity harness consumes it
- Then the harness panics with `parity divergence: yaml rejects json-valid
  document`
