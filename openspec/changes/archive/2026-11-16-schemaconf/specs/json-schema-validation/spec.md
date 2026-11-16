# Delta: json-schema-validation (change `schemaconf`)

## ADDED Requirements

### Requirement: Schema reference resolution is confined

JSON Schema compilation SHALL resolve `$ref` references only through a confined retriever: a base URI of `file://<canonical root schema path>` combined with per-reference checks performed on the real file path (never on the URI). The retriever SHALL accept only `file`-scheme references (percent-decoded and canonicalized) whose canonical path is contained in the canonical root schema directory, SHALL accept only regular files, SHALL cap each file at 16 MiB, SHALL cap resolution at 256 resources and 32 MiB total per compilation, and SHALL fail closed past any cap. Resource accounting SHALL count the root schema as one resource with its bytes toward the total and SHALL count each distinct external resource once per compilation (repeated references to the same canonical path are free). File reads SHALL be bounded (never metadata-only size checks). A `$id` that rebases resolution outside the root directory or onto a remote scheme SHALL be rejected. Remote references SHALL be rejected with the literal error `remote $ref '<uri>' rejected: schemas resolve from local files only (ADR-0100); vendor the file`, preserved through consumer error wrapping. Inline schemas (no file root) SHALL resolve only same-document (`#`) references; every external reference SHALL fail with a named rejection error. Included files SHALL be parsed by the target file extension (`.yaml`/`.yml` as YAML, otherwise JSON), while the root document's parser SHALL follow an explicit syntax override when one is supplied (`type`/`schemaType`) and the root file extension otherwise. The schema's `$schema` SHALL select the draft, with 2020-12 assumed when absent. With the workspace `resolve-file` feature removed, a bare `jsonschema::validator_for` SHALL fail closed on any external reference.

#### Scenario: sibling file reference resolves without $id

- **GIVEN** a root schema file `root.json` with no `$id`, compiled with the base URI derived from its canonical path, and `$ref: "defs.json#/$defs/x"`
- **WHEN** the schema compiles and validates an instance
- **THEN** compilation succeeds and the referenced `defs.json` definition is enforced (the base URI alone suffices)

#### Scenario: parent-directory reference is rejected naming the path

- **GIVEN** a root schema file whose `$ref: "../outside.json"` targets a file outside the root schema directory
- **WHEN** the schema compiles
- **THEN** compilation fails closed with an error that names the rejected path

#### Scenario: percent-encoded traversal is rejected

- **GIVEN** a root schema file whose `$ref` percent-encodes a traversal (e.g. `%2e%2e%2foutside.json`) targeting a file outside the root schema directory
- **WHEN** the schema compiles
- **THEN** compilation fails closed with a rejection error naming the decoded, canonicalized rejected path

#### Scenario: symlink escape is rejected

- **GIVEN** a root schema directory containing a symlink that resolves outside the root schema directory
- **WHEN** a `$ref` follows that symlink
- **THEN** compilation fails closed with a rejection error

#### Scenario: absolute file reference outside the root is rejected

- **GIVEN** a root schema file whose `$ref: "file:///etc/hostname"` (or any absolute path outside the root schema directory) targets a file the process can read
- **WHEN** the schema compiles
- **THEN** compilation fails closed with a rejection error; the target file is not read

#### Scenario: remote reference is rejected with the local-files-only error

- **GIVEN** a schema containing an `https://` `$ref`, or a relative `$ref` rebased by an `https` `$id`
- **WHEN** the schema compiles
- **THEN** compilation fails closed with the literal error `remote $ref '<uri>' rejected: schemas resolve from local files only (ADR-0100); vendor the file`

#### Scenario: out-of-root file $id rebase is rejected

- **GIVEN** a schema whose `$id` rebases resolution onto a `file:` URI outside the root schema directory
- **WHEN** the schema compiles
- **THEN** compilation fails closed with a rejection error naming the rejected path; the base URI override is not trusted

#### Scenario: oversized file is rejected

- **GIVEN** a root schema directory containing an includable file larger than 16 MiB
- **WHEN** a `$ref` targets that file
- **THEN** compilation fails closed with an error naming the file and the size cap

#### Scenario: non-regular file is rejected without blocking

- **GIVEN** a root schema directory containing a FIFO or other non-regular file
- **WHEN** a `$ref` targets that file
- **THEN** compilation fails closed with a rejection error and does not block on opening the file

#### Scenario: resource budget is enforced deterministically

- **GIVEN** compilations at exactly 256 resources / under 32 MiB total and just beyond each cap (257 distinct resources; total bytes over 32 MiB)
- **WHEN** each schema compiles
- **THEN** the at-cap compilation succeeds and each over-cap compilation fails closed with a named cap error

#### Scenario: inline schema external reference is rejected while local refs work

- **GIVEN** an inline schema (processor `request_schema` or `camel-embedded:`) containing `$ref: "#/$defs/..."` and a second schema containing an external `$ref`
- **WHEN** each schema compiles
- **THEN** the `#` reference resolves and validates; the external reference fails closed with a named rejection error

#### Scenario: JSON root referencing a YAML file works

- **GIVEN** a JSON root schema whose `$ref` targets a `.yaml` (or `.yml`) file, and the reverse arrangement of a YAML root referencing a JSON file
- **WHEN** each schema compiles and validates an instance
- **THEN** the target file is parsed by its own extension and both schemas compile and enforce their referenced definitions

#### Scenario: explicit root syntax override keeps sibling resolution

- **GIVEN** a root schema file whose explicit `type`/`schemaType` selection disagrees with its extension (e.g. a YAML root named `.json`) and which references a sibling file
- **WHEN** the schema compiles with the explicit syntax and validates an instance
- **THEN** the root parses per the explicit syntax, the sibling reference resolves under confinement, and parse failures keep the existing parse-error text

#### Scenario: draft selection follows $schema

- **GIVEN** a draft-07 schema resolving `definitions` through an external ref, a 2020-12 schema using `$defs`, a referenced file declaring a different draft than its root, and a draft-07 schema with keywords beside `$ref`
- **WHEN** each schema compiles and validates
- **THEN** the `$schema` of each document selects its draft semantics (2020-12 when absent), draft-07 ignores keywords beside `$ref` while 2020-12 applies them, and cross-draft includes compile under the referencing behavior

#### Scenario: bare validator_for fails closed on external refs

- **GIVEN** the workspace `jsonschema` dependency compiled without `resolve-file`
- **WHEN** a bare `jsonschema::validator_for` compiles a schema containing an external file `$ref`
- **THEN** compilation fails (external refs have no sanctioned resolution path outside the confined crate)
