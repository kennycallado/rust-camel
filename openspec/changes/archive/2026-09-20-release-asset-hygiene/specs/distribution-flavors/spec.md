# Delta: distribution-flavors

## MODIFIED Requirements

### Requirement: Shipped artifact names are flavor-prefixed

Release artifacts SHALL be named `camel-slim-<target>.tar.gz`,
`camel-<target>.tar.gz` (regular), and `camel-full-<target>.tar.gz`,
each accompanied by its `.tar.gz.sha256` sidecar (format per the
release-pipeline checksummed-tarballs requirement). The clean legacy
name `camel-<target>` SHALL re-alias from the historical full-ish
closure to regular at the first flavored release, announced in the
release notes (one-time semantic break, bd ruling superseding the
verdict's transition-release alternative). All three names SHALL exist
from that release onward; the raw extensionless names shipped by
v0.50.0 are retired at the first tarball release (documented one-time
break).

#### Scenario: all three names ship from the first flavored release

- **WHEN** the first flavored tag release completes
- **THEN** the GitHub release carries `camel-slim-*` (musl ×2),
  `camel-*` (regular, the 4 Linux targets), and `camel-full-*` (gnu ×2,
  macOS ×2, Windows) — 11 tarballs with 11 matching `.sha256` sidecars
  (22 files); on desktop targets the clean regular name is retired
  (documented breaking change, scripts must switch to `camel-full-*`),
  and no asset name 404s relative to the previous release's names
  except by documented retirement or re-alias

### Requirement: Flavor closure contract enforced in tests and release

The feature-closure test suite (`crates/camel-cli/tests/feature_profiles.rs`)
SHALL encode the flavor contracts as prefix sets alongside the existing
`SLIM_FORBIDDEN_PREFIXES`: `REGULAR_REQUIRED`, `REGULAR_FORBIDDEN`, and
`FULL_REQUIRED` (containing kafka). The golden closure fixture SHALL be
regenerated in the same change that re-curates any flavor body. The release
path SHALL run per-flavor capability asserts against the built binary
before packaging, extending the existing symbol-assert pattern.

#### Scenario: contract sets gate the closures

- **WHEN** the feature-closure suite runs
- **THEN** regular satisfies every `REGULAR_REQUIRED` prefix and contains no
  `REGULAR_FORBIDDEN` prefix, full satisfies `FULL_REQUIRED` (kafka), and slim
  contains no `SLIM_FORBIDDEN_PREFIXES` entry

#### Scenario: slim security omission fails closed

- **WHEN** a slim build encounters a configuration requiring security
  features it does not embed
- **THEN** startup rejects the configuration with an explicit error, and a
  test proves the rejection (no silent degradation to an insecure mode)

#### Scenario: release artifacts carry per-flavor capability asserts

- **WHEN** a tag run builds and collects artifacts
- **THEN** each leg's binary is smoke-probed for its flavor suffix in
  `--version` and its flavor capability set (kafka registered on full;
  absence asserts on slim/regular) before packaging into its tarball —
  cross-compiled legs (aarch64 musl/gnu) are tarball-name-asserted at
  release collection instead, since their binaries do not run on the
  build runner
