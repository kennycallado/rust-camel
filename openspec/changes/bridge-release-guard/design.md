# Design: bridge-release-guard

## Approach

Three ordered, independently verifiable phases.

### Phase 1: network guard + CI wiring

**Goal:** make a declared-but-never-released bridge version fail CI.

Add `scripts/xtask/src/bridge_versions/releases.rs` with pure builders and one
bounded network probe:

- `release_tag(bridge, version) -> "{bridge}-bridge-v{version}"`.
- `release_api_url(tag) -> "https://api.github.com/repos/kennycallado/rust-camel/releases/tags/{tag}"`.
- `expected_assets(bridge, version) -> Vec<String>`: `{bridge}-bridge-{version}-linux-musl-x86_64.tar.gz`,
  `{bridge}-bridge-{version}-macos-aarch64.tar.gz`,
  `{bridge}-bridge-{version}-windows-x86_64.zip`,
  `{bridge}-bridge-{version}-SHA256SUMS`. Mirrors all three release matrices.
- `check_releases(registry)`: for each bridge, one `GET` via a `ureq` 3
  `Agent` built with `timeout_global(Some(20s))` (same pattern as
  `lint_publish_registration.rs`). `200` parses `draft` and `assets[].name`;
  `draft == true` is a finding; each missing expected name is a finding.
  `404` is a missing-release finding naming the API URL. Every other status,
  transport error, timeout, or JSON parse failure is a fail-closed finding
  naming the URL. No credentials; three requests per run, so the public rate
  limit is not a concern.

CLI surface: `LintBridgeVersions { tag, check_releases }` with
`#[arg(long)] check_releases: bool`. `bridge_versions::lint` keeps its current
signature and delegates to a new `lint_with(root, tag, check_releases)`, so the
offline path is byte-identical. Add `release_findings` to `LintOutcome`
(empty offline). Aggregation is deliberate: offline findings do not stop release
checks; both print and exit 1. `--tag` validation is unchanged.

**Scratch-registry proof without a false pass:** a scratch registry
(`jms = "0.8.8"`) intentionally disagrees with the checked-in const, so offline
drift is reported. Release checks are aggregated, not short-circuited, so the
network verdict still executes and reports the published `jms-bridge-v0.8.8` as
passing. The scratch edit is transient, restored byte-for-byte, never committed;
the committed registry stays at `0.9.1`. The failure proof uses `0.9.1` (or
`0.0.0-does-not-exist` if it has since published).

**CI wiring — execution, not prose:** add a step to the real
`.github/workflows/ci.yml` `quality` job immediately after `lint-bridge-versions`:

```yaml
- name: lint-bridge-releases
  run: cargo xtask lint-bridge-versions --check-releases
```

Add the same gate name and command to the `## QUALITY GATES` block in
`AGENTS.md`. Add `lint-bridge-releases` to the default `SKIP` list in
`scripts/gates-local.sh` and extend the comment block: it needs the GitHub
release API, so CI owns it. `gates-local.sh` parses `AGENTS.md`, so the local
run reports it as skipped by name while the offline `lint-bridge-versions` still
runs. Why CI-only: the registry is external; local reproducibility stays intact,
and the merge gate is where the published fact exists.

### Phase 2: JMS published smoke

**Goal:** exercise the artifact users download.

Add `.github/workflows/published-jms-bridge-smoke.yml` as a sibling of
`published-bridge-smoke.yml`; the CXF file is not modified. A separate workflow
avoids shared `workflow_run` filters and keeps the CXF trigger byte-identical.
It lives beside CXF because it is the same failure class and reuses the same
download/checksum rules.

Triggers: `workflow_run` on `["JMS Bridge Release"]` with
`branches: ['jms-bridge-v*']`, plus `workflow_dispatch` input
`bridge_version`. Steps:

1. Resolve the download version from dispatch input or `workflow_run.head_branch`,
   falling back to the newest `jms-bridge-v*` release.
2. **Independent registry assertion:** read `jms = "..."` from
   `bridges/versions.toml` and require `gh release view jms-bridge-v{registry}`
   to succeed, even when dispatch overrides the download version.
3. Require the resolved tag to exist, download the tarball and SHA256SUMS,
   verify with `sha256sum -c`, extract, check the binary is executable, and
   export `CAMEL_JMS_BRIDGE_BINARY_PATH`.
4. Run `cargo test -p camel-test --features integration-tests --test jms_test --
   jms_consumer_receives_from_activemq --nocapture --test-threads=1`. The test
   uses the ActiveMQ testcontainer, so setup-java and Docker are available.
   `integration-tests` gates `jms_test.rs`; no crate change is needed.
5. Timeout protection copies the CXF pattern: `setsid` cargo, poll a deadline,
   `kill -TERM/-KILL` the whole group, redirect output to a file and `tail
   --pid` it (never pipe cargo, because spawned children hold the pipe).
   `always()` cleanup runs `pkill -f 'jms-bridge'`. Timeout 90 minutes.

### Phase 3: bump procedure

**Goal:** remove the stale literal-edit instruction.

Rewrite `.opencode/prompts/bump-bridge.md`: table keyed by bridge with
`bridges/versions.toml` as the single source, `cargo xtask bump-bridge <bridge>
<version>` as the atomic edit (registry plus registered consts, both `xml`
consts), and the tag as release trigger. Order: bump, offline lint, commit, then
the **human** pushes the tag; the agent never tags or pushes. Post-tag
verification: `--check-releases` green, release carries the three matrix assets
plus SHA256SUMS, tag matches the registry. Document that `--check-releases`
fails before the release exists; that is expected, not a bump failure.

## Affected crates

- `scripts/xtask`: release module, aggregate lint mode, CLI flag.
- `crates/camel-test`: no code change; its existing JMS test is invoked by the
  workflow.
- Workflows, `AGENTS.md`, `scripts/gates-local.sh`, bump prompt: wiring and
  procedure.

## Architecture boundaries

Tooling and CI only. No Runtime, DSL, Components, Services, Languages,
or Functions change; no wire-contract change. Bridge literals and `bridges/**`
stay untouched. It observes the registry, never publishes.

## Alternatives considered

- Pinning `jms` back to `0.8.8`: rejected; deletes the durable subscriber
  feature shipped in 0.66.0.
- Checking releases in the offline lint: rejected; network failures would block
  every local commit.
- A second job in `published-bridge-smoke.yml`: rejected; it changes the CXF
  workflow's triggers and couples unrelated release streams.
- Narrow `integration-tests-jms` feature: rejected as crate-scope creep.
