# Delta: eip-splitter

## MODIFIED Requirements

### Requirement: Fragment body typing is element-driven

A split that derives fragments from JSON array elements SHALL type each
fragment body from the array element it derives from: string elements produce
`Body::Text`; number, boolean, object, nested-array, and null elements produce
`Body::Json`. Fragment typing SHALL NOT depend on how many nodes the split
expression matched. The rule holds for the declarative split and for the
programmatic `split_body_json_array` expression alike.

#### Scenario: N-match string nodeset yields raw-text fragments

- **GIVEN** a declarative split whose xpath expression matches 3 string nodes in the input XML
- **WHEN** the route runs and a fragment body is observed
- **THEN** the fragment body is `Body::Text` and `${body}` renders the raw string with no JSON quote characters

#### Scenario: Match-count parity

- **GIVEN** the same xpath split expression applied once to XML where it matches 1 node and once to XML where it matches 3 nodes
- **WHEN** fragments are produced
- **THEN** the per-fragment body type and content are identical for both counts (`Body::Text`, raw string)

#### Scenario: collect_all aggregation is indifferent to the typing change

- **GIVEN** a declarative split with `collect_all` aggregation over string elements
- **WHEN** the split scope closes
- **THEN** the aggregated JSON array is byte-identical to the pre-change output, because `Body::Text` and `Body::Json(String)` aggregate to the same `Value::String`

#### Scenario: jsonpath string-array splits yield text fragments

- **GIVEN** a declarative split whose jsonpath expression evaluates to `["a","b"]`
- **WHEN** fragments are produced
- **THEN** each fragment body is `Body::Text` with the raw string

#### Scenario: Non-string elements keep JSON typing

- **GIVEN** a declarative split whose expression evaluates to an array containing a number and an object
- **WHEN** fragments are produced
- **THEN** those fragment bodies are `Body::Json` with the element value unchanged

#### Scenario: Programmatic body_json_array mirrors element-driven typing

- **GIVEN** the programmatic `split_body_json_array` expression over a JSON array that contains strings (one of them empty), a number, and an object
- **WHEN** the splitter runs
- **THEN** each string element yields a `Body::Text` fragment with the raw string (the empty string yields an empty `Body::Text`, not `""`), and the number and object elements yield `Body::Json` fragments with the element value unchanged — matching the declarative path
