# mock-testkit Delta

## MODIFIED Requirements

### Requirement: Declarative test document parsing

`camel test` SHALL accept one or more test documents (`*.test.yaml`). A test
document SHALL contain exactly one route source: `routeFiles` (paths to
route YAML files, resolved relative to the test document's directory),
`routeFilesFromRoot` (paths resolved against the nearest ancestor
`Camel.toml` directory), or inline `routes` (same schema as route files).
It MAY contain optional `inputs` and SHALL contain a mandatory non-empty
`expects` map keyed by mock endpoint name. Documents declaring two or three
route sources, or none, SHALL be rejected. Unknown fields SHALL be rejected.

#### Scenario: valid document with reference and expectations

- **Given** a test document with `routeFiles: [config/routes.yaml]` and `expects: {mock:result: {count: 3}}`
- **When** `camel test` parses the document
- **Then** parsing succeeds, `config/routes.yaml` resolves relative to the test document's directory, and one expectation set exists for endpoint `result`

#### Scenario: unknown field rejected

- **Given** a test document containing an unknown top-level field
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 and a message naming the unknown field

#### Scenario: empty expects rejected

- **Given** a test document with `expects: {}` or no `expects` key
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating that `expects` is mandatory

#### Scenario: both routeFiles and routes rejected

- **Given** a test document containing both `routeFiles` and `routes`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating the route source keys are mutually exclusive and naming `routeFiles` and `routes`

#### Scenario: routeFilesFromRoot combined with another source rejected

- **Given** a test document containing `routeFilesFromRoot` together with `routeFiles`, with `routes`, or with both of the other keys
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating the route source keys are mutually exclusive and naming every present key

#### Scenario: no route source rejected

- **Given** a test document containing none of `routeFiles`, `routeFilesFromRoot`, or `routes`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating exactly one route source is required

#### Scenario: unsupported body scalar rejected

- **Given** an input whose `body` is a null, boolean, or number scalar
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating only string, object, and array bodies are supported

#### Scenario: settle out of range rejected

- **Given** a test document with `settle: 0ms` or `settle: 10s`
- **When** `camel test` parses the document
- **Then** parsing fails with exit code 2 stating `settle` must satisfy `0 < settle <= 5s`

### Requirement: camel run non-interference

Route YAML files SHALL remain unchanged by this feature: `camel run`'s route
discovery (initial load and watch reload) SHALL skip files whose names end
in `.test.yaml` or `.test.yml` on every pattern path, and test documents
SHALL NOT alter route file schema or runtime behavior. This replaces the
previous contract that honored an explicit `*.test.yaml` glob as a user
override: it is an accepted breaking change (route-shaped files using the
reserved suffix must be renamed). An explicit no-wildcard pattern naming a
test-suffixed file SHALL fail with the reserved-suffix error (see the dsl
specification, "Reserved test suffix in route discovery"). A wildcard
pattern that matches only test documents SHALL yield no routes from those
files.

#### Scenario: run ignores test documents

- **Given** a routes directory containing `routes/demo.yaml` and `routes/demo.test.yaml`
- **When** `camel run` discovers routes with default globs
- **Then** only `demo.yaml` loads as a route and no error occurs

#### Scenario: explicit Camel.toml routes entry ignores test documents

- **Given** `Camel.toml` with explicit `routes = ["routes/*.yaml"]` and a colocated `routes/demo.test.yaml`
- **When** `camel run` starts
- **Then** the test document is skipped, startup succeeds, and a watch reload triggered by saving the test document is a no-op

#### Scenario: explicit glob override honored

- **Given** a user invoking `camel run --routes 'routes/*.test.yaml'`
- **When** discovery runs
- **Then** the explicit glob is processed and every matched test document is skipped (never parsed as a route document); when no other routes match, the command reports that no routes were found

#### Scenario: explicit file naming errors instead of parsing

- **Given** a user invoking `camel run --routes routes/demo.test.yaml`
- **When** discovery runs
- **Then** the run fails with the reserved-suffix error directing the user to `camel test`; the document is never parsed as a route document

## ADDED Requirements

### Requirement: Root-anchored route file references

A test document MAY declare `routeFilesFromRoot`, a list of route file paths
resolved against the project root. The project root SHALL be the nearest
ancestor directory of the test document that contains a `Camel.toml` file,
located by walking up from the test document's directory. Resolution SHALL
be independent of the process working directory. When no ancestor directory
contains `Camel.toml`, the document run SHALL fail with exit code 2 and a
`NoProjectRoot` error that names the test document's directory.

#### Scenario: nested test document resolves from project root

- **GIVEN** a project with `Camel.toml` at its root, a route file `routes/orders.yaml`, and a test document at `tests/integration/orders.test.yaml` containing `routeFilesFromRoot: [routes/orders.yaml]`
- **WHEN** `camel test` runs on that document (given by an absolute or otherwise cwd-valid path) from any working directory
- **THEN** the route file resolves to `<project-root>/routes/orders.yaml` and the document executes

#### Scenario: nearest ancestor Camel.toml wins

- **Given** a monorepo with `Camel.toml` at the repository root and a second `Camel.toml` in `services/orders/`, and a test document at `services/orders/tests/a.test.yaml`
- **When** the document declares `routeFilesFromRoot: [routes/a.yaml]`
- **Then** the path resolves against `services/orders/`, the nearest ancestor

#### Scenario: no ancestor Camel.toml rejected

- **Given** a test document in a directory tree with no `Camel.toml` in any ancestor
- **When** `camel test` runs the document with `routeFilesFromRoot` declared
- **Then** the run fails with exit code 2 and a `NoProjectRoot` error naming the test document's directory
