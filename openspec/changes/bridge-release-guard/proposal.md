# Proposal: bridge-release-guard

## Why

v0.66.0 shipped `camel_jms::BRIDGE_VERSION = "0.9.1"` and
`bridges/versions.toml: jms = "0.9.1"`, but the newest published JMS bridge
release is `jms-bridge-v0.8.8`. `crates/services/camel-bridge/src/download.rs`
turns those values into
`https://github.com/kennycallado/rust-camel/releases/download/jms-bridge-v{version}`,
so every installed `jms:`/`activemq:`/`artemis:` route 404s when the bridge
starts. Three independent guards were missing and any one would have caught it:

1. `lint-bridge-versions` never asked the release registry whether the declared
   version exists; `--tag` validates a tag only when supplied.
2. The published-bridge smoke covers CXF only, chained off `cxf-bridge-release`.
3. `.opencode/prompts/bump-bridge.md` still points at a Gradle version literal
   and buries the tag push that actually triggers the release.

Bd: `rc-bxjzq`, `rc-o60f5`, `rc-mjvub`.

## What Changes

- Add `cargo xtask lint-bridge-versions --check-releases`. For every
  `bridges/versions.toml` entry it requires a published, non-draft GitHub release
  `{bridge}-bridge-v{version}`, carrying the three platform assets and the
  `{bridge}-bridge-{version}-SHA256SUMS` file expected by that bridge's release
  matrix. Failures name the exact URL. The HTTP path is bounded and fail-closed.
- Wire the new check into the real `.github/workflows/ci.yml` `quality` job and
  into the `## QUALITY GATES` block in `AGENTS.md`. Add the gate name to the
  network skip list in `scripts/gates-local.sh` with prose explaining the skip.
- Add a JMS published-bridge smoke beside the CXF one: `workflow_run` on
  `JMS Bridge Release`, plus `workflow_dispatch` with `bridge_version`. It
  downloads the published tarball, verifies its checksum, and runs the existing
  `jms_consumer_receives_from_activemq` test against
  `CAMEL_JMS_BRIDGE_BINARY_PATH`. It independently requires the registry tag
  even when a dispatch overrides the downloaded version.
- Rewrite `.opencode/prompts/bump-bridge.md` around `bridges/versions.toml` as
  the single source, the two Rust consts for `xml`, and the tag as release
  trigger; add post-tag release verification and state that the human pushes.

**Excluded:** no changes to `bridges/versions.toml`, any `bridges/**` source,
`camel_jms::BRIDGE_VERSION`, `crates/**/src/**`, or the CXF smoke job. No tagging,
release triggering, or push in this change.

## Acceptance criteria

- `cargo xtask lint-bridge-versions` and `--tag jms-bridge-v0.9.1` output and exit
  status are unchanged when `--check-releases` is absent.
- `--check-releases` fails on the current unreleased `jms = "0.9.1"` and names
  `jms-bridge-v0.9.1` with the exact failed URL.
- `--check-releases` passes for a scratch registry copy pointing at a published
  version (for example `0.8.8`); the committed registry is unchanged.
- CI's `quality` job executes the new command; `scripts/gates-local.sh` skips it
  by name and exits 0.
- The JMS smoke workflow is valid YAML, preserves the CXF workflow unchanged,
  and fails when the registry tag is absent.

## Risk budget

Accepted: public GitHub API rate limits (three requests per run) and the smoke's
runtime cost. Out of bounds: weakening the offline lint, changing bridge
versions, editing bridge sources, touching the CXF smoke, or making tagging
automatic. A false negative from an API outage is unacceptable: transport and
non-404 failures must fail the gate, never pass it.
