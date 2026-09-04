# runtime-boot Specification

## Purpose
TBD - created by archiving change integration-tier-contract. Update Purpose after archive.
## Requirements
### Requirement: Bundle cascade parity

The system SHALL register the same component bundles from the same
`Camel.toml` whether boot flows through `camel run` or through the embedded
harness, through a single cascade owned by `camel-bundles`.

#### Scenario: identical registration through both boots

- **GIVEN** a `Camel.toml` configuring http, file, container, and template
- **WHEN** the runtime boots through `camel run` and through the harness boot
- **THEN** both CamelContexts hold the same registered component set and the
  same per-bundle configuration

#### Scenario: feature forwarding

- **GIVEN** a consumer that enables the `kafka` bundle feature
- **WHEN** the boot registers bundles
- **THEN** the forwarded feature selection decides bundle availability,
  identical for CLI and harness consumers

### Requirement: BootHandle lifecycle

The system SHALL own bridge cleanup and pool teardown in a `BootHandle`
returned by the shared boot, with an explicit `shutdown()`, and the CLI
SHALL keep the watcher, signal handling, exec guard, and operator logging.

#### Scenario: explicit teardown

- **GIVEN** a boot that started jms and cxf pools
- **WHEN** `BootHandle::shutdown()` completes
- **THEN** pools and bridge cleanup are closed and no leaked tasks remain

#### Scenario: CLI ownership unchanged

- **GIVEN** a `camel run` process under the extracted boot
- **WHEN** the file watcher fires or the user sends Ctrl+C twice
- **THEN** behavior is identical to the pre-extraction path

