# Proposal: schemamatch

## Why

A contract-first team cannot assert that a payload conforms to a schema in
either test tier. The unit tier offers `equals`, `regex`, `contains`,
`startsWith`, `endsWith`, `exists`, `jsonSubset`; the last is a structural
subset, not a schema. Teams approximate a schema check with `jsonSubset` or
by placing a `validator:` step inside the route under test — which tests the
route, not the payload contract, and cannot assert a reply or a dead-letter
body (bd `rc-0lvd3`, p2).

`schemaconf` (`428b80220`, ADR-0100) landed `camel-json-schema`: one confined
compiler (`compile`, `SchemaSource::{File,Inline}`, `SchemaSyntax::{Auto,Json,Yaml}`,
`Violation { instance_path, message }`). This change is the consumer that
crate was built for.

## What Changes

- **In scope** (unit tier only, per the 2026-11-15 e_opus ruling):
  - `BodyMatcher::JsonSchema` in `camel-component-mock`: the body validates
    against a compiled JSON Schema; the compiled schema is held behind an
    `Arc` (it is not `Clone`) and compiled once at document load.
  - A `jsonSchema` body-matcher key in the `camel-cli` unit-tier document
    grammar: object form `{ "path": "<file>", "type": "json|yaml" }`, `type`
    optional, default `auto` (extension-based: `.yaml`/`.yml` are YAML).
  - Fail-closed load: missing file, unreadable file, unresolvable `$ref`,
    invalid schema, and unsupported `type` (`xsd`, `protobuf`) are document
    errors naming the schema path. Paths resolve relative to the unit
    document's directory.
  - Failure messages carry the schema path and the first violation in the
    established shape `{message} at {instance_path}` (same as
    `camel-processor`'s JSON Schema validation).
  - A `jsonSchema` section on the testkit docs page with a runnable example.
  - Component-lane rows for `camel-cli` (deliberate declaration the lane
    sweep's fail-safe asks for) and `camel-component-mock`.
- **Out of scope**: the scenario/integration-tier `validate` target
  (bd `rc-v0bxl`, deferred by the same ruling); xsd/protobuf engines (absent
  from the tree — rejected at load, recorded as a deferral); any change to
  `camel-component-validator`, the `validator:` step, `camel-sql`, `camel-api`,
  or the route model; schema caches or watch modes.

## Acceptance criteria

- A unit-tier document can assert a received or replied body against a JSON
  or YAML schema file, failing with the schema path and the first violation.
- Relative schema paths resolve against the document's directory (pinned by
  a test that runs from a different CWD).
- Load failures fail closed naming the path; unsupported types name the
  supported set (json, yaml).
- A non-JSON body is a non-match, not an error; `jsonSubset` behavior is
  unchanged.
- Spec delta extends `mock-testkit` "Declarative test document parsing";
  confinement references `json-schema-validation` (ADR-0100) rather than
  restating it.

## Risk budget

Parser-grammar churn is the main risk; it is contained to the strict
body-matcher grammar and its pinned tests. No new dependency direction is
introduced beyond `camel-component-mock → camel-json-schema` (a leaf crate).
Compilation cost is paid once per document load, never per assertion.

Bd: rc-0lvd3
