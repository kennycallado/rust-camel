## ADDED Requirements

### Requirement: Permanent v1 non-goals

The compiled artifact SHALL remain a sealed deployment unit and SHALL NOT grow development-loop or ambient-configuration capabilities. Permanent v1 non-goals are: watch or hot-reload; runtime file discovery or globbing; ambient `Camel.toml` loading (no external configuration is loaded); a wide argument surface (the artifact remains limited to `--report`, `--help`, `--version`, and `--manifest`, with R4 signature verification as the single sanctioned future surface extension); and compile-time `CAMEL_*` configuration overrides. Deployment-time `${env:NAME}` interpolation in the embedded document remains permitted and is distinct from compile-time `CAMEL_*` overrides.

#### Scenario: Permanent non-goals remain outside the artifact contract

- **GIVEN** an operator or roadmap proposal requests watch/hot-reload, runtime file discovery/globbing, ambient `Camel.toml`, a command argument beyond `--report`/`--help`/`--version`/`--manifest` other than the sanctioned R4 signature-verification surface, or a compile-time `CAMEL_*` configuration override
- **WHEN** the proposal is evaluated against the v1 compiled-artifact contract
- **THEN** the capability is rejected as a permanent non-goal rather than added to the artifact surface

#### Scenario: Embedded configuration does not become ambient configuration

- **GIVEN** a compiled artifact is deployed without its source tree or an ambient `Camel.toml`
- **WHEN** the artifact starts
- **THEN** it loads no external configuration, resolves only permitted deployment-time `${env:NAME}` expressions from the embedded document, and performs no runtime discovery, globbing, watch, or hot-reload behavior

#### Scenario: Artifact arguments stay narrow

- **GIVEN** a valid compiled artifact
- **WHEN** the operator supplies an argument other than `--report`, `--help`, `--version`, `--manifest`, or the sanctioned R4 signature-verification surface
- **THEN** the artifact rejects the argument with exit 2 and does not expand its command surface
