# Delta: release-pipeline

## ADDED Requirements

### Requirement: Release assets are checksummed tarballs

Every uploaded release asset SHALL be a gzip-compressed tar archive named
`<artifact-name>.tar.gz` with the binary at archive root (`camel`, or
`camel.exe` on Windows targets), accompanied by a
`<artifact-name>.tar.gz.sha256` sidecar whose content is
`<64-hex>␣␣<basename>` (sha256sum `-c`-checkable). No raw binary name
SHALL ship after the first tarball release.

#### Scenario: tarball and sidecar per asset

- **GIVEN** a tag run completing with the 12-entry matrix (11 uploading)
- **WHEN** the release is created
- **THEN** it carries exactly 11 `.tar.gz` files (2 slim + 5 full +
  4 regular) and exactly 11 `.sha256` sidecars, each sidecar named for
  exactly one present tarball, and no raw binary file

#### Scenario: sidecars verify at consumption points

- **WHEN** the docker job consumes its flavor-matched tarballs
- **THEN** it runs `sha256sum -c` against each sidecar before extraction,
  and a mismatch or missing sidecar fails the job before any image layer
  is built; `sha256sum -c` with a sidecar next to its tarball passes for
  any end user

#### Scenario: raw names retired loudly

- **WHEN** the first tarball release ships
- **THEN** the breaking change is documented (release notes /
  distribution docs): raw `camel-<target>` names from v0.50.0 are
  replaced by `camel-<target>.tar.gz` — one-time break, same
  loud-break policy as the desktop clean-name retirement

#### Scenario: dev harness exercises packaging

- **GIVEN** a dev-profile run
- **WHEN** its 3 build legs complete
- **THEN** the uploading legs pack and checksum exactly like tag legs,
  and the gnu docker variant verifies and extracts its tarball in the
  dev run
