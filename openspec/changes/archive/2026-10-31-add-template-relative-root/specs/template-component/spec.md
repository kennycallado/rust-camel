## ADDED Requirements

### Requirement: Template root configuration and confinement

The system SHALL accept an optional `[components.template]` `root` path
(absolute or relative to the process CWD). When `root` is configured, the
system SHALL canonicalize both the configured root and the entry candidate
at startup, SHALL fail with a clear error on a missing root directory, a
missing entry file, or a canonical entry outside the canonical root, and
SHALL anchor the dependency closure at the canonical root. Entry opening
SHALL use the entry's original root-relative path components under the
same no-symlink confinement as closure targets; canonicalization SHALL NOT
replace confined opening. When `root` is not configured, the system SHALL
keep the existing entry-parent anchoring for absolute URIs unchanged.

#### Scenario: relative entry under root loads and renders

- **GIVEN** `[components.template] root = "<tempdir>"` and a route declaring
  `template:file:templates/page.html` with `page.html` present under
  `<tempdir>/templates/`
- **WHEN** the route starts and a request is processed
- **THEN** the nested `<tempdir>/templates/page.html` renders — not a
  different `<tempdir>/page.html` namesake

#### Scenario: dot-dot segment in relative URI rejected

- **GIVEN** `[components.template] root` configured and a route declaring
  `template:file:../escape.html`
- **WHEN** the endpoint is constructed
- **THEN** construction fails with `CamelError::Config`

#### Scenario: leaf symlink entry inside the root rejected

- **GIVEN** `[components.template] root` configured and a symlink
  `link.html → page.html` inside the root with a route declaring the
  `link.html` entry
- **WHEN** the route starts
- **THEN** startup fails with a path-escape error and the route does not
  serve requests

#### Scenario: intermediate symlink inside the root rejected

- **GIVEN** `[components.template] root` configured, a directory symlink
  `partials → <dir-inside-root>` inside the root, and a route declaring the
  `partials/page.html` entry
- **WHEN** the route starts
- **THEN** startup fails with a path-escape error and the route does not
  serve requests

#### Scenario: symlink escaping the root rejected

- **GIVEN** `[components.template] root` configured, an entry inside the
  root, and a closure target that is a symlink pointing outside the
  canonical root to an existing file
- **WHEN** the route starts
- **THEN** startup fails with a path-escape error and the route does not
  serve requests

#### Scenario: absolute entry outside configured root rejected

- **GIVEN** `[components.template] root = "<tempdir-a>"` and a route
  declaring `template:file:///<tempdir-b>/page.html` where `<tempdir-b>` is
  outside the canonical root and `page.html` exists there
- **WHEN** the route starts
- **THEN** startup fails with a clear escape error naming the entry and the
  canonical root

#### Scenario: missing root directory fails with clear error

- **GIVEN** `[components.template] root = "<path-that-does-not-exist>"`
- **WHEN** the route starts
- **THEN** startup fails with an error naming the configured root

#### Scenario: include outside root rejected

- **GIVEN** `[components.template] root` configured, the entry inside the
  root including `../outside.html`, and `outside.html` existing outside the
  root
- **WHEN** the route starts
- **THEN** the closure acquisition fails and the route does not serve
  requests

## MODIFIED Requirements

### Requirement: External template URI resolution

The system SHALL parse templates from route-declared `template:file:///abs/path`
URIs and, when a component `root` is configured, from route-declared relative
`template:file:<rel-path>` URIs, at endpoint construction. The system SHALL
reject bare-path (`template:/abs/path`) and non-`file:` schemes at
construction with `CamelError::Config`. A relative URI without a configured
component `root` SHALL fail construction with `CamelError::Config`. The
relative form SHALL contain no empty path component, no `.` component, and
no `..` component. File acquisition SHALL occur during awaited startup, not
at construction.

#### Scenario: valid file URI parses at construction

- **GIVEN** a route declaring `template:file:///srv/templates/page.html`
- **WHEN** the endpoint is constructed
- **THEN** the URI parses and construction succeeds without filesystem access

#### Scenario: valid relative URI parses at construction when root configured

- **GIVEN** `[components.template] root` configured and a route declaring
  `template:file:templates/page.html`
- **WHEN** the endpoint is constructed
- **THEN** the URI parses and construction succeeds without filesystem access

#### Scenario: relative URI without configured root rejected

- **GIVEN** no `[components.template]` `root` configured and a route
  declaring `template:file:templates/page.html`
- **WHEN** the endpoint is constructed
- **THEN** construction fails with `CamelError::Config`

#### Scenario: bare-path URI rejected

- **GIVEN** a route declaring `template:/srv/templates/page.html`
- **WHEN** the endpoint is constructed
- **THEN** construction fails with `CamelError::Config`

#### Scenario: non-file scheme rejected

- **GIVEN** a route declaring `template:http://host/page.html`
- **WHEN** the endpoint is constructed
- **THEN** construction fails with `CamelError::Config`

#### Scenario: dot component in relative URI rejected

- **GIVEN** `[components.template] root` configured and a route declaring
  `template:file:./templates/page.html`
- **WHEN** the endpoint is constructed
- **THEN** construction fails with `CamelError::Config`

### Requirement: Dependency-closure contract

The system SHALL resolve `include`, `extends`, `import`, and `from` targets
openat-relative to the configured canonical root when a component `root` is
set, and openat-relative to the entry template's parent directory otherwise,
through a statically discovered closure. The system SHALL reject dynamic
(render-time-computed) template names, symlinks, `..` segments, absolute paths,
cycles, and duplicate file identities.

#### Scenario: include within root accepted

- **GIVEN** a configured root and a template that includes a file under the
  same canonical root
- **WHEN** the closure is acquired
- **THEN** the include resolves relative to the canonical root and is
  compiled

#### Scenario: include escape rejected

- **GIVEN** a template that includes a path escaping the configured root
- **WHEN** the closure is acquired
- **THEN** acquisition rejects the include

#### Scenario: dynamic template name rejected

- **GIVEN** a template whose include target is a render-time variable
- **WHEN** the closure is acquired
- **THEN** acquisition rejects the closure as not statically discoverable

#### Scenario: cycle rejected

- **GIVEN** template A includes template B and B includes A
- **WHEN** the closure is acquired
- **THEN** acquisition rejects the cycle

#### Scenario: existing anchoring preserved without a configured root

- **GIVEN** no component `root` configured and an absolute entry URI
- **WHEN** the closure is acquired
- **THEN** the closure resolves relative to the entry's parent directory
  with all rejection behaviors (dynamic names, symlinks, `..`, absolute
  paths, cycles, duplicate identities) unchanged
