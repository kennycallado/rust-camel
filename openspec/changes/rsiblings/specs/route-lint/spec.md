# Delta Spec: rsiblings

## ADDED Requirements

### Requirement: R-SCHEMA anyOf de-collapse surfaces co-located sibling defects

When the pattern de-collapse arm of R-SCHEMA's `AnyOf` handling
reports nested pattern leaves from a failed `anyOf` (rc-n3t73), any
non-pattern defect co-located in the SAME failed `anyOf` MUST also be
reported as its own leaf diagnostic — the sibling defect MUST NOT be
dropped. Sibling diagnostics SHALL anchor on their own authored span
per the existing per-keyword anchoring rules (`additionalProperties`
per unexpected key; others on the resolved instance node) and SHALL
render through the deterministic message path (`diagnostic_message`),
preserving the d0d88267 byte-exactness guarantee. The null-branch
whole-node type mismatch (`{"type": "null"}` failing because the
instance is an object of the intended shape) is branch noise, not a
defect, and MUST NOT be reported. When NO nested pattern error
surfaces from a failed `anyOf`, the single collapsed diagnostic MUST
remain byte-identical to the pre-change shape.

#### Scenario: Pattern violation plus unknown key both reported

- **Given** an `mcp:` block whose `server.tls` carries a blank
  `cert_path` (pattern violation) and an unknown `tls` key
  (schema `additionalProperties: false`)
- **When** R-SCHEMA analyzes the document
- **Then** exactly two R-SCHEMA Errors are emitted: one for the blank
  `cert_path` (pattern message, anchored on the blank value) and one
  for the unknown key (anchored per the `additionalProperties`
  anchoring rule), each on its own span, and NO container-anchored
  type diagnostic is emitted (the null-branch whole-node mismatch is
  excluded)

#### Scenario: Pattern violation plus non-string sibling both reported

- **Given** an `mcp:` block whose `server.tls` carries a blank
  `cert_path` and a non-string `key_path` (e.g. an empty sequence)
- **When** R-SCHEMA analyzes the document
- **Then** exactly two R-SCHEMA Errors are emitted: one for the blank
  `cert_path` (pattern message, anchored on the blank value) and one
  for the non-string `key_path` (type message, anchored on the
  offending value), and NO container-anchored type diagnostic is
  emitted (the null-branch whole-node mismatch is excluded)

#### Scenario: Pure-pattern cases stay byte-identical

- **Given** an `mcp:` block whose only defect is a blank `cert_path`
  or `key_path` (pattern violations alone)
- **When** R-SCHEMA analyzes the document
- **Then** the diagnostics are byte-identical to the pre-change
  behavior: one pattern Error per blank field, no additional
  diagnostics (the null-branch type mismatch is not reported)

#### Scenario: Pure-non-pattern collapses stay byte-identical

- **Given** a failed `anyOf` with no nested pattern error (e.g. an
  unknown `tls` key alone, or `disposition: bogus`, or
  `binding: bogus`)
- **When** R-SCHEMA analyzes the document
- **Then** the single collapsed anyOf diagnostic keeps its byte-exact
  generic message and anchor (the
  `rschema_exception_disposition_oneof_unchanged` and
  `rschema_rest_binding_oneof_unchanged` pins stay green)

#### Scenario: Corpus co-occurrence fixture is baselined as failing

- **Given** the corpus fixture
  `crates/camel-cli/tests/fixtures/lint-corpus/mcp-tls-sibling-defects.yaml`
  carrying a blank `cert_path` plus an unknown `tls` key
- **When** the `corpus_zero_false_positives` gate runs
- **Then** the fixture emits `("R-SCHEMA", "error")` exactly as
  recorded in the baseline with a justification, and the gate passes
