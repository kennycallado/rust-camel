# cli-scaffold Delta

## ADDED Requirements

### Requirement: Basic template scaffolds a testable route with colocated test document

`camel new` with the basic template SHALL emit a `routes/hello.yaml` sample
route with a `direct:` consumer and a `mock:` producer step, and a colocated
`routes/hello.test.yaml` sample test document referencing it through
`routeFiles: [hello.yaml]` with at least one input and one expectation. The
generated README SHALL contain a test section instructing `camel test
routes/hello.test.yaml`, placed before the run section. The scaffolded
project SHALL pass `camel test routes/hello.test.yaml` and SHALL start
under `camel run` without route discovery errors.

#### Scenario: scaffolded project passes its sample test

- **Given** a project created by `camel new` with the basic template
- **When** `camel test routes/hello.test.yaml` runs in the project directory
- **Then** every expectation passes and the exit code is 0

#### Scenario: scaffolded project starts under camel run

- **Given** a project created by `camel new` with the basic template
- **When** `camel run` starts in the project directory
- **Then** route discovery loads `hello.yaml`, skips `hello.test.yaml` via the reserved suffix rule, and startup succeeds

#### Scenario: README teaches test before run

- **Given** a project created by `camel new` with the basic template
- **When** the generated README is read
- **Then** a test section mentioning `camel test routes/hello.test.yaml` appears before the run section
