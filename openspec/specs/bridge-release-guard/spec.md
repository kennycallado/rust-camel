# bridge-release-guard Specification

## Purpose
TBD - created by archiving change bridge-release-guard. Update Purpose after archive.
## Requirements
### Requirement: Release existence is checked against the registry

The system SHALL provide `cargo xtask lint-bridge-versions --check-releases`,
which for every entry in `bridges/versions.toml` requires a published,
non-draft GitHub release named `{bridge}-bridge-v{version}` and the assets
declared by that bridge's release matrix: `linux-musl-x86_64.tar.gz`,
`macos-aarch64.tar.gz`, `windows-x86_64.zip`, and
`{bridge}-bridge-{version}-SHA256SUMS`. It SHALL report per bridge and include
the exact URL that failed.

#### Scenario: Published release with every matrix asset passes

- **GIVEN** `bridges/versions.toml` names a version whose release is published and not a draft
- **WHEN** `cargo xtask lint-bridge-versions --check-releases` runs
- **THEN** the release check passes for that bridge and exits zero when every bridge passes

#### Scenario: Declared but never released version fails with the exact URL

- **GIVEN** `bridges/versions.toml` declares `jms = "0.9.1"` and no `jms-bridge-v0.9.1` release exists
- **WHEN** `cargo xtask lint-bridge-versions --check-releases` runs
- **THEN** it fails, names `jms-bridge-v0.9.1`, and prints
  `https://api.github.com/repos/kennycallado/rust-camel/releases/tags/jms-bridge-v0.9.1`

#### Scenario: Published release missing a platform asset fails

- **GIVEN** the release tag exists but its asset list omits `windows-x86_64.zip` or `SHA256SUMS`
- **WHEN** `cargo xtask lint-bridge-versions --check-releases` runs
- **THEN** it fails for that bridge and names the missing asset and the release download URL

#### Scenario: Registry outage fails closed

- **GIVEN** the release API returns a non-404 error, times out, or returns invalid JSON
- **WHEN** `cargo xtask lint-bridge-versions --check-releases` runs
- **THEN** the check fails, names the exact requested URL, and does not report a pass

### Requirement: Offline lint and tag validation remain unchanged

The system SHALL keep `cargo xtask lint-bridge-versions` and
`cargo xtask lint-bridge-versions --tag <tag>` byte-identical when
`--check-releases` is absent. Release findings SHALL NOT suppress offline
constant, live-set, or tag findings; both result sets SHALL be reported and the
command SHALL exit non-zero when either fails.

#### Scenario: Default invocation has no network effect

- **GIVEN** a checkout with no network access
- **WHEN** `cargo xtask lint-bridge-versions` runs without `--check-releases`
- **THEN** output and exit status match the pre-change behavior and no HTTP request is made

#### Scenario: Tag validation is unchanged

- **GIVEN** the registry has `jms = "0.8.8"`
- **WHEN** `cargo xtask lint-bridge-versions --tag jms-bridge-v0.9.1` runs
- **THEN** it reports the same tag-mismatch diagnostic as before this change

#### Scenario: Offline drift does not suppress release checks

- **GIVEN** a scratch registry disagrees with the checked-in `BRIDGE_VERSION` constants
- **WHEN** `cargo xtask lint-bridge-versions --check-releases` runs
- **THEN** it reports the offline drift and still executes and reports the release check for every registry entry

### Requirement: The release check executes in CI

The system SHALL run `cargo xtask lint-bridge-versions --check-releases` as a
step in the `quality` job of `.github/workflows/ci.yml`, SHALL list the same
command in the `## QUALITY GATES` block of `AGENTS.md` under the name
`lint-bridge-releases`, and SHALL include that name in the default skip list of
`scripts/gates-local.sh` with prose explaining that it needs the network.

#### Scenario: CI executes the gate

- **GIVEN** a pull request reaches the `quality` job
- **WHEN** the workflow steps run
- **THEN** a `lint-bridge-releases` step invokes `cargo xtask lint-bridge-versions --check-releases` and fails the job on a non-zero exit

#### Scenario: Local sweep skips the gate by name

- **GIVEN** `scripts/gates-local.sh` runs with default settings
- **WHEN** it parses the `AGENTS.md` registry
- **THEN** `lint-bridge-releases` appears in the skipped summary and is not executed, while the offline `lint-bridge-versions` gate still runs

### Requirement: JMS published artifact smoke

The system SHALL provide a JMS published-bridge smoke workflow triggered by
`workflow_run` on `JMS Bridge Release` and by `workflow_dispatch` with a
`bridge_version` input. It SHALL verify the published tarball against its
SHA256SUMS and run the existing `jms_consumer_receives_from_activemq` test with
`CAMEL_JMS_BRIDGE_BINARY_PATH` pointing at the downloaded binary. It SHALL
independently require the tag derived from `bridges/versions.toml` to exist even
when a dispatch overrides the downloaded version. The CXF smoke SHALL remain
unchanged.

#### Scenario: Post-release smoke runs the published JMS binary

- **GIVEN** `JMS Bridge Release` completes successfully for a `jms-bridge-v*` tag
- **WHEN** the JMS smoke workflow runs
- **THEN** it downloads the published tarball and checksum, exports `CAMEL_JMS_BRIDGE_BINARY_PATH`, and executes `jms_consumer_receives_from_activemq` successfully

#### Scenario: Dispatch override cannot hide an unreleased registry version

- **GIVEN** `bridges/versions.toml` says `jms = "0.9.1"` and a dispatch passes an older published `bridge_version`
- **WHEN** the JMS smoke resolves the download version
- **THEN** it still fails because `jms-bridge-v0.9.1` does not exist

#### Scenario: Timeout kills the process group

- **GIVEN** the cargo test phase exceeds its deadline
- **WHEN** the smoke's deadline expires
- **THEN** it terminates the entire process group, reports a timeout failure, and leaves no `jms-bridge` process running

### Requirement: Bridge bump procedure follows the registry

The system SHALL document `bridges/versions.toml` as the single version source,
`cargo xtask bump-bridge` as the atomic edit for the registry and its registered
`BRIDGE_VERSION` constants (both `xml` constants), and the `{bridge}-bridge-v*`
tag as the release trigger. It SHALL require post-tag verification and state that
only a human pushes the tag.

#### Scenario: Bump uses the registry path

- **GIVEN** an operator bumps `jms` from `0.8.8` to `0.9.1`
- **WHEN** they follow the procedure
- **THEN** the registry and `camel_jms::BRIDGE_VERSION` change together, no Gradle literal is edited, and the workflow instructs the human to push the tag

#### Scenario: Post-tag verification catches an unpublished version

- **GIVEN** the tag was pushed but the release is missing or incomplete
- **WHEN** the operator runs `cargo xtask lint-bridge-versions --check-releases`
- **THEN** it fails and names the missing release or asset URL, and the procedure blocks declaring the bump complete

### Requirement: Scratch registry proof

The system SHALL allow the guard to be demonstrated against a scratch registry
copy whose version exists, without committing that copy. The committed registry
and bridge constants SHALL remain unchanged after the proof.

#### Scenario: Published scratch version demonstrates the pass path

- **GIVEN** a scratch copy of `bridges/versions.toml` sets `jms = "0.8.8"` while the checked-in constants still say `0.9.1`
- **WHEN** `cargo xtask lint-bridge-versions --check-releases` runs
- **THEN** the release check reports `jms-bridge-v0.8.8` as passing even though the offline check reports the expected drift

#### Scenario: Scratch change is reverted

- **GIVEN** the scratch registry was used for the proof
- **WHEN** the proof finishes
- **THEN** `bridges/versions.toml` is restored byte-for-byte to `jms = "0.9.1"` and `git status` shows no registry change

