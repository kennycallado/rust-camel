## MODIFIED Requirements

### Requirement: Run embedded documents without extraction

The artifact SHALL feed the indexed documents through the existing parse, runtime interpolation, lowering, boot, and start paths using an in-memory virtual store. Runtime SHALL build configuration from embedded `Camel.toml`, include, and selected-profile entries. Embedded job configuration SHALL pass through the same job boot projection as argv jobs before context configuration; embedded route configuration SHALL boot unchanged. Runtime SHALL perform no source-tree reads, glob expansion, ambient `Camel.toml` or profile loading, canonicalization, temporary extraction, watch, or hot reload. Source diagnostics SHALL use `compiled://<logical-path>` identities. `${env:NAME}` expressions SHALL resolve from the deployment environment, not the compiler environment. Route `--report <path>` SHALL write JSON object `{ "kind": "route", "status": "completed"|"failed", "error": string|null }`; job reports SHALL use the existing job outcome schema. Boot and report-write failures SHALL exit 2; route pipeline failures SHALL exit 1; completed routes SHALL exit 0.

#### Scenario: Multi-document artifact runs without its source tree

- **GIVEN** a valid compiled artifact whose source files, `Camel.toml`, and working directory are unavailable
- **WHEN** the artifact starts with required deployment environment values
- **THEN** all indexed route documents boot and run from memory without file discovery, extraction, or temporary writes

#### Scenario: Runtime cannot discover an unembedded file

- **GIVEN** a compiled artifact and a new route file placed beside it after compilation
- **WHEN** the artifact starts
- **THEN** the new file is ignored because runtime consumes only indexed store entries

#### Scenario: Unknown store schema fails closed

- **GIVEN** a marked artifact whose index declares a store schema the executable does not support
- **WHEN** the artifact starts
- **THEN** it exits 2 before configuration or route boot

#### Scenario: Read-only deployment

- **GIVEN** a valid multi-document artifact running with a read-only root filesystem
- **WHEN** it boots and receives deployment environment values
- **THEN** it runs without materializing documents or writing files other than an explicitly requested report

#### Scenario: Embedded job artifact binds no diagnostic listeners

- **GIVEN** a compiled job artifact whose embedded configuration enables `[observability.prometheus]` and `[observability.health]` and declares `[runtime_journal]`
- **WHEN** the artifact starts
- **THEN** it projects the configuration through the job boot policy, opens no journal, binds no Prometheus or health listener, and exits with the normal job outcome codes

### Requirement: Restrict artifact arguments and expose manifest

The artifact SHALL accept only `--report <path>`, `--help`, `--version`, and `--manifest` plus the sanctioned R4 signature-verification surface. Duplicate exclusive flags, missing report values, positional arguments, and other arguments SHALL exit 2. The operational manifest SHALL contain a separate `manifest_schema` field and an `embedded_files` list with canonical logical paths, document kinds, byte lengths, and content digests. Manifest schema values SHALL be validated independently from trailer version. `--manifest` SHALL print this metadata without booting. The manifest SHALL not contain compile-time environment values and SHALL list required environment variables without defaults. Listener declarations SHALL report the artifact kind's effective runtime listeners: job artifacts SHALL omit listeners the job boot projection suppresses, and route artifacts SHALL list all configuration-declared listeners.

#### Scenario: Manifest inspection

- **GIVEN** a valid multi-document artifact
- **WHEN** the operator runs `./app --manifest`
- **THEN** the command exits 0 without booting and prints manifest schema, artifact kind, logical embedded-file metadata, components, required environment names, and listener declarations

#### Scenario: Unknown manifest schema fails closed

- **GIVEN** a marked artifact with a manifest schema that the reader does not support
- **WHEN** the artifact starts
- **THEN** it exits 2 before boot and does not reinterpret trailer version as manifest schema

#### Scenario: Unknown artifact argument

- **GIVEN** a valid artifact
- **WHEN** the operator supplies an unknown, positional, duplicate-exclusive, or incomplete report argument
- **THEN** it exits 2, names the rejected argument, and does not boot

#### Scenario: Manifest includes operational version

- **GIVEN** a valid multi-document artifact
- **WHEN** the operator runs `./app --manifest`
- **THEN** output includes runtime version, artifact kind, embedded files, required environment names, and listener declarations without booting

#### Scenario: Job artifact manifest omits suppressed listeners

- **GIVEN** a compiled job artifact whose embedded configuration enables health and Prometheus listeners
- **WHEN** the operator runs `./app --manifest`
- **THEN** the listener declarations list is empty of the suppressed health and Prometheus endpoints, matching the job boot projection's effective runtime

#### Scenario: Route artifact manifest keeps config-declared listeners

- **GIVEN** a compiled route artifact whose embedded configuration enables health and Prometheus listeners
- **WHEN** the operator runs `./app --manifest`
- **THEN** the listener declarations list contains both endpoints, because route artifacts bind them at runtime
