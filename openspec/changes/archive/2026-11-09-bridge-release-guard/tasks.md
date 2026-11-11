# Tasks: bridge-release-guard

Delivery phases mirror `design.md §3` (Phase 1 network guard + CI wiring, Phase 2
JMS published smoke, Phase 3 bump procedure). Phases execute in order. Each task
is one coherent phase deliverable, per the mission direction.

## Scope check (STAGE 2 start)

**Verdict: PASS.**

- Phase 1 mixes two layers (Rust guard + wiring) but one deliverable: a
  declared-but-unreleased bridge version fails CI. The design records both layers
  in the same phase with a single exit criterion, and the wiring is meaningless
  without the guard. Coherent.
- Phase 2 is one artifact (one new workflow file) reusing the blessed CXF
  pattern. Coherent.
- Phase 3 is one artifact (one prompt rewrite). Coherent.
- No phase contains more than one independent subsystem that could ship alone.
  Boundary matches `design.md §Approach`; no spec-level defect to escalate.

## Execution rules (apply to every task)

- Work only inside `/home/shared/rust-camel-worktrees/bridgereleaseguard` on
  branch `feature/bridgereleaseguard`. Never build in the main checkout.
- Prefix every cargo invocation with:
  `systemd-run --user --scope --collect --unit=fleet-bridgereleaseguard -p MemoryMax=12G env TMPDIR=/tmp`
  and run commands sequentially (the unit name is reused).
- Never run `cargo test --workspace`, `cargo clippy --workspace`, or any full
  workspace build **during task execution**. Run only the package/test targets
  named in each task. This restriction is task-local: after all tasks the
  conductor MUST run the `## QUALITY GATES` sweep (`bash scripts/gates-local.sh`)
  in the feature worktree, never in the main checkout. The mission's prohibition
  on full-workspace `cargo test` and on replicating CI full-test suites remains
  in force before, during, and after the tasks; no task or conductor step in
  this change runs `cargo test --workspace` or an integration/e2e suite. A
  conductor-required `cargo test --workspace --lib` is a library-unit run, not
  the forbidden full test suite; that narrow distinction does not authorize any
  wider workspace test run.
- Do not commit, tag, push, or trigger a release. Do not run `git push`.
- TDD: author the named test for a step before writing that step's
  implementation, run it, and record the red result (compile error, unknown
  flag, missing file, or failed assertion) before making it green.
- Local steps must not call `curl`, `wget`, or any inline HTTP fetch. The only
  release-registry network path is the `xtask` probe (Phase 1) and the CI
  workflow (Phase 2). Nix-package realisation for the Phase 2 YAML parser is a
  toolchain fetch, not a release-registry call.
- Forbidden edits for the whole change: any `bridges/**` source,
  `bridges/versions.toml` committed content, any `BRIDGE_VERSION` constant,
  `crates/**/src/**`, and `.github/workflows/published-bridge-smoke.yml`.
  The single permitted transient write to `bridges/versions.toml` is the
  mission-authorized scratch proof in Task 1.1; it must be restored
  byte-for-byte and never committed.
- Capture command output to files under `/tmp` and print only the assertion
  result (`printf 'ok\n'`), to keep worker context small.

## Phase 1: network guard + CI wiring

### `scripts/xtask` + CI wiring

#### Task 1.1: Add `--check-releases`, aggregate reporting, and wire the gate into CI, AGENTS.md, and gates-local

**Files:**
- `scripts/xtask/src/bridge_versions/releases.rs` (new)
- `scripts/xtask/src/bridge_versions/mod.rs` (modified)
- `scripts/xtask/src/main.rs` (modified)
- `.github/workflows/ci.yml` (modified)
- `AGENTS.md` (modified)
- `scripts/gates-local.sh` (modified)
- `bridges/versions.toml` (transient proof write only in step 15; restored byte-for-byte)

**Steps:**

0. Capture the pre-change baseline before editing anything. Run and keep the
   four files:
   `cargo build -p xtask`, then
   `./target/debug/xtask lint-bridge-versions >/tmp/blv-pre.out 2>/tmp/blv-pre.err; echo $? >/tmp/blv-pre.rc`
   and the same with `--tag cxf-bridge-v0.8.5` into `/tmp/blv-tag-pre.*`.
   The binary path is relative to the worktree root.

1. Create `scripts/xtask/src/bridge_versions/releases.rs`. Add
   `pub mod releases;` to `scripts/xtask/src/bridge_versions/mod.rs` (alphabetical
   after `registry`). Reuse the existing `super::registry::{Bridge, Registry}`;
   no `Cargo.toml` change is needed (`ureq`, `serde_json`, `serde` are already
   dependencies).

2. In `releases.rs` define:
   `pub struct ReleaseFinding { pub tag: String, pub url: String, pub detail: String }`
   and `pub struct ReleasePass { pub tag: String, pub assets: usize }`, both with
   `#[derive(Debug, Clone, PartialEq, Eq)]`.

3. Add pure builders:
   - `pub fn release_tag(bridge: Bridge, version: &str) -> String` returning
     `format!("{}-bridge-v{}", bridge.key(), version)`.
   - `pub fn release_api_url(tag: &str) -> String` returning
     `format!("https://api.github.com/repos/kennycallado/rust-camel/releases/tags/{tag}")`.
   - `pub fn release_download_base(tag: &str) -> String` returning
     `format!("https://github.com/kennycallado/rust-camel/releases/download/{tag}")`.
   - `pub fn expected_assets(bridge: Bridge, version: &str) -> Vec<String>` returning
     exactly four names, with `b = bridge.key()`:
     `format!("{b}-bridge-{version}-linux-musl-x86_64.tar.gz")`,
     `format!("{b}-bridge-{version}-macos-aarch64.tar.gz")`,
     `format!("{b}-bridge-{version}-windows-x86_64.zip")`,
     `format!("{b}-bridge-{version}-SHA256SUMS")` — the union of all three release
     matrices (`jms-bridge-release.yml`, `cxf-bridge-release.yml`,
     `xml-bridge-release.yml`).

4. Add private serde types for the GitHub payload:
   `#[derive(serde::Deserialize)] struct ReleasePayload { draft: bool, assets: Vec<ReleaseAsset> }`
   and `#[derive(serde::Deserialize)] struct ReleaseAsset { name: String }`.

5. Add `pub fn classify_response(bridge: Bridge, version: &str, status: u16, body: Option<&str>) -> Result<ReleasePass, Vec<ReleaseFinding>>`:
   - `tag = release_tag(bridge, version)`; `api = release_api_url(&tag)`.
   - `status == 200`: when `body` is `None`, return `Err(vec![one finding])`
     with `detail = format!("invalid JSON from {api}: empty response body - failing closed")`
     and `url = api`; otherwise `serde_json::from_str::<ReleasePayload>` the body.
     On parse error return `Err(vec![one finding])` with `detail =
     format!("invalid JSON from {api}: {e} - failing closed")` and `url = api`.
     Build `let mut findings = Vec::new();`. If `draft` is true push one finding
     with `detail = format!("release {tag} is a draft - publish it before declaring the bump complete")`
     and `url = api`. Compute `missing: Vec<&str> = expected_assets(bridge, version)`
     names not in `assets[].name`; for each missing expected name push its own
     finding with `detail = format!("release {tag} is missing asset {name} - downloads: {}", release_download_base(&tag))`
     and `url = release_download_base(&tag)` — exactly one finding per missing
     expected name, matching `design.md §Phase 1` ("each missing expected name is
     a finding"). Return `Err(findings)` when non-empty; otherwise return
     `Ok(ReleasePass { tag, assets: expected_assets(bridge, version).len() })`.
   - `status == 404`: `Err(vec![one finding])` with `detail =
     format!("release {tag} not found - publish the release or fix bridges/versions.toml")`
     and `url = api`.
   - Any other status: `Err(vec![one finding])` with `detail =
     format!("release {tag}: unexpected HTTP status {status} at {api} - failing closed")`
     and `url = api`.

6. Add `pub fn classify_transport_error(tag: &str, error: &str) -> ReleaseFinding`
   returning `detail = format!("release {tag}: transport error {error} at {} - failing closed", release_api_url(tag))`
   and `url = release_api_url(tag)`.

7. Add `pub fn check_releases(registry: &Registry) -> (Vec<ReleasePass>, Vec<ReleaseFinding>)`.
   Build the agent exactly like `lint_publish_registration::observation_agent`:
   `ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(20))).build().new_agent()`.
   Iterate `[Bridge::Cxf, Bridge::Jms, Bridge::Xml]`; for each, derive `tag`, `url`,
   call `agent.get(&url).call()`, and classify:
   - `Ok(mut response)` with `status = response.status().as_u16()`: read
     `response.body_mut().read_to_string()`; a read error becomes
     `findings.push(classify_transport_error(&tag, &e.to_string()))`; otherwise
     `match classify_response(bridge, version, status, Some(&body))` and push the
     pass on `Ok`, extend `findings` with every element on `Err`.
   - `Err(ureq::Error::StatusCode(code))`: the same `match` with
     `classify_response(bridge, version, code, None)`.
   - Any other `Err(e)`: `findings.push(classify_transport_error(&tag, &e.to_string()))`.
   Push passes and findings into separate vectors in registry order and return
   both. Never short-circuit: one bridge failure must not skip the next bridge.

8. Add `#[cfg(test)] mod tests` in `releases.rs` with the fourteen cases listed
   under **Tests** below.

9. In `mod.rs` add `use releases::{ReleaseFinding, ReleasePass};` and extend
   `LintOutcome` with
   `pub release_findings: Vec<ReleaseFinding>` and
   `pub release_passed: Vec<ReleasePass>` (imported types, not a
   `super::releases::` path). Update every `LintOutcome` struct literal in
   `mod.rs` with `release_findings: Vec::new(), release_passed: Vec::new()`.

10. Add `impl LintOutcome`:
    - `pub fn failed(&self) -> bool` returns
      `self.tag_error.is_some() || !self.findings.is_empty() || !self.release_findings.is_empty()`.
    - `pub fn lines(&self) -> Vec<String>` returns, in order: every offline
      finding as `format!("{}:{}: {} [{}]", finding.path.display(), finding.line, finding.detail, finding.token)`;
      every release pass as `format!("release OK: {} ({} assets)", pass.tag, pass.assets)`;
      every release finding as `format!("release finding: {} {}: {}", f.tag, f.url, f.detail)`.
      Return an empty `Vec` when clean.

11. In `mod.rs` keep `pub fn lint(root: &Path, tag: Option<&str>) -> Result<LintOutcome, String>`
    with its current signature and body replaced by
    `lint_with(root, tag, false)`. Add
    `pub fn lint_with(root: &Path, tag: Option<&str>, check_releases: bool) -> Result<LintOutcome, String>`
    containing the current `lint` body. Thread `check_releases` into
    `collect_findings(root, registry, check_releases)`; after the offline
    `findings` are collected, when `check_releases` is true call
    `releases::check_releases(registry)` and store both vectors. The `--tag`
    unknown-prefix and mismatch early returns keep empty release vectors and make
    no HTTP request. A successful `--tag` path still runs release checks when the
    flag is set.

12. In `main.rs` extend the CLI variant:
    `LintBridgeVersions { #[arg(long)] tag: Option<String>, #[arg(long)] check_releases: bool }`
    with the doc comment "Check the GitHub release registry for every declared
    bridge version. Network; fail-closed on transport and non-404 errors.".

13. In the `Commands::LintBridgeVersions { tag }` arm destructure
    `{ tag, check_releases }`, call `bridge_versions::lint_with(&workspace_root, tag.as_deref(), check_releases)`,
    keep the `tag_error` early exit unchanged, then:
    `for line in outcome.lines() { println!("{line}"); }`.
    When `outcome.findings` is non-empty print the unchanged trailer
    `eprintln!("\nlint-bridge-versions: FAILED ({} findings)", outcome.findings.len())`.
    When `outcome.release_findings` is non-empty print
    `eprintln!("\nlint-bridge-versions: FAILED ({} release findings)", outcome.release_findings.len())`.
    Exit 1 when either is non-empty; otherwise print
    `lint-bridge-versions: OK (no findings)`. With `--check-releases` absent all
    output, stdout/stderr split, and exit status must match step 0 exactly.

14. Wire the gate:
    - `.github/workflows/ci.yml`: immediately after the `lint-bridge-versions`
      step (currently lines 441-442) insert
      `- name: lint-bridge-releases` / `  run: cargo xtask lint-bridge-versions --check-releases`
      with the same two-space `run:` indentation as its siblings.
    - `AGENTS.md`: immediately after the `lint-bridge-versions` entry in the
      `## QUALITY GATES` block insert the same `- name:` / `  run:` pair.
    - `scripts/gates-local.sh`: add a comment line
      `#   lint-bridge-releases -> GitHub release API (published tag and assets)`
      and append `lint-bridge-releases` to the default `SKIP` assignment, keeping
      the existing names and `GATES_SKIP` override:
      `SKIP="${GATES_SKIP-lint-commits doc-guide-test cargo-audit audit lint-bridge-releases}"`.

15. Run the scratch-registry proof with a mandatory restore trap. Replace
    `bridges/versions.toml` content `jms = "0.9.1"` with `jms = "0.8.8"` only for
    the duration of the proof:
    ```
    cp bridges/versions.toml /tmp/blv-registry.bak
    trap 'cp /tmp/blv-registry.bak bridges/versions.toml' EXIT
    sed -i 's/^jms = "0.9.1"$/jms = "0.8.8"/' bridges/versions.toml
    ./target/debug/xtask lint-bridge-versions --check-releases >/tmp/blv-scratch.out 2>/tmp/blv-scratch.err; printf '%s\n' "$?" >/tmp/blv-scratch.rc
    cp /tmp/blv-registry.bak bridges/versions.toml
    cmp /tmp/blv-registry.bak bridges/versions.toml
    ```
    The committed registry must never contain `0.8.8` after the task. The
    negative proof's published-version branch (falling back to a transient
    `0.0.0-does-not-exist`) is specified by the `network_negative_current_registry`
    test below; do not silently skip the negative proof.

**Tests:** (executable spec — name, setup, action, assert, command, expected)

Unit tests in `scripts/xtask/src/bridge_versions/releases.rs::tests`:

- `release_tag_formats_bridge_and_version` — setup: none; action: call
  `release_tag(Bridge::Jms, "0.9.1")` and `release_tag(Bridge::Xml, "0.8.4")`;
  assert: `"jms-bridge-v0.9.1"` and `"xml-bridge-v0.8.4"`; command:
  `cargo test -p xtask --lib bridge_versions::releases::tests::release_tag_formats_bridge_and_version`;
  expected: red before (module absent, compile error), green after.
- `release_api_url_is_exact` — setup: none; action: `release_api_url("jms-bridge-v0.9.1")`;
  assert: exactly
  `https://api.github.com/repos/kennycallado/rust-camel/releases/tags/jms-bridge-v0.9.1`;
  command: `cargo test -p xtask --lib bridge_versions::releases::tests::release_api_url_is_exact`.
- `release_download_base_is_exact` — setup: none; action:
  `release_download_base("jms-bridge-v0.8.8")`; assert: exactly
  `https://github.com/kennycallado/rust-camel/releases/download/jms-bridge-v0.8.8`;
  command: `cargo test -p xtask --lib bridge_versions::releases::tests::release_download_base_is_exact`.
- `expected_assets_lists_three_platforms_and_sums` — setup: none; action: loop
  over `[Bridge::Cxf, Bridge::Jms, Bridge::Xml]` with version `0.8.8`; assert:
  `len() == 4` and the four names are
  `{b}-bridge-0.8.8-linux-musl-x86_64.tar.gz`,
  `{b}-bridge-0.8.8-macos-aarch64.tar.gz`,
  `{b}-bridge-0.8.8-windows-x86_64.zip`,
  `{b}-bridge-0.8.8-SHA256SUMS`; command:
  `cargo test -p xtask --lib bridge_versions::releases::tests::expected_assets_lists_three_platforms_and_sums`.
- `classify_published_release_passes` — setup: body
  `{"draft":false,"assets":[{"name":"jms-bridge-0.8.8-linux-musl-x86_64.tar.gz"},{"name":"jms-bridge-0.8.8-macos-aarch64.tar.gz"},{"name":"jms-bridge-0.8.8-windows-x86_64.zip"},{"name":"jms-bridge-0.8.8-SHA256SUMS"}]}`;
  action: `classify_response(Bridge::Jms, "0.8.8", 200, Some(body))`; assert:
  `Ok(ReleasePass { tag: "jms-bridge-v0.8.8", assets: 4 })`; command:
  `cargo test -p xtask --lib bridge_versions::releases::tests::classify_published_release_passes`.
- `classify_draft_release_fails` — setup: same body with `"draft":true`; action:
  classify 200; assert: `Err(findings)`, `findings.len() == 1`,
  `findings[0].detail` contains `"is a draft"`, `findings[0].url` equals
  `release_api_url("jms-bridge-v0.8.8")`; command: `cargo test -p xtask --lib
  bridge_versions::releases::tests::classify_draft_release_fails`.
- `classify_missing_windows_asset_fails` — setup: the pass body with the
  `windows-x86_64.zip` asset removed; action: classify 200; assert:
  `Err(findings)`, `findings.len() == 1`, `findings[0].detail` contains
  `"windows-x86_64.zip"` and `release_download_base("jms-bridge-v0.8.8")`,
  `findings[0].url` equals that download base; command:
  `cargo test -p xtask --lib bridge_versions::releases::tests::classify_missing_windows_asset_fails`.
- `classify_missing_checksums_fails` — setup: the pass body with the
  `SHA256SUMS` asset removed; action: classify 200; assert:
  `Err(findings)`, `findings.len() == 1`, `findings[0].detail` contains
  `"SHA256SUMS"`; command: `cargo test -p xtask --lib
  bridge_versions::releases::tests::classify_missing_checksums_fails`.
- `classify_each_missing_asset_is_a_separate_finding` — setup: the pass body
  with both `windows-x86_64.zip` and `SHA256SUMS` removed; action: classify 200;
  assert: `Err(findings)`, `findings.len() == 2`, one detail contains
  `"windows-x86_64.zip"` and the other contains `"SHA256SUMS"`, both `url`
  values equal `release_download_base("jms-bridge-v0.8.8")`; command:
  `cargo test -p xtask --lib
  bridge_versions::releases::tests::classify_each_missing_asset_is_a_separate_finding`.
- `classify_404_names_api_url` — setup: none; action:
  `classify_response(Bridge::Jms, "0.9.1", 404, None)`; assert:
  `Err(findings)`, `findings.len() == 1`, `findings[0].detail` contains
  `"not found"`, `findings[0].url` equals
  `https://api.github.com/repos/kennycallado/rust-camel/releases/tags/jms-bridge-v0.9.1`;
  command: `cargo test -p xtask --lib bridge_versions::releases::tests::classify_404_names_api_url`.
- `classify_non_404_status_fails_closed` — setup: none; action: classify 500
  with `None`; assert: `Err(findings)`, `findings.len() == 1`,
  `findings[0].detail` contains `"500"` and `"failing closed"`,
  `findings[0].url` equals the API URL; command: `cargo test -p xtask --lib
  bridge_versions::releases::tests::classify_non_404_status_fails_closed`.
- `classify_invalid_json_fails_closed` — setup: body `"<html>gateway</html>"`;
  action: classify 200 with that body; assert: `Err(findings)`,
  `findings.len() == 1`, `findings[0].detail` contains `"invalid JSON"` and
  `"failing closed"`; command: `cargo test -p xtask --lib
  bridge_versions::releases::tests::classify_invalid_json_fails_closed`.
- `classify_missing_body_fails_closed` — setup: none; action: classify 200 with
  `None`; assert: `Err(findings)`, `findings.len() == 1`,
  `findings[0].detail` contains `"invalid JSON"`; command:
  `cargo test -p xtask --lib
  bridge_versions::releases::tests::classify_missing_body_fails_closed`.
- `classify_transport_error_fails_closed` — setup: none; action:
  `classify_transport_error("jms-bridge-v0.8.8", "timed out")`; assert:
  `detail` contains `"timed out"` and `"failing closed"`, `url` equals the API
  URL; command: `cargo test -p xtask --lib
  bridge_versions::releases::tests::classify_transport_error_fails_closed`.

Unit tests in `scripts/xtask/src/bridge_versions/mod.rs::tests`:

- `lint_without_flag_has_empty_release_results` — setup: `lint_fixture()` with
  matching registry and four constants; action: `lint_with(dir.path(), None, false)`;
  assert: `release_findings.is_empty()`, `release_passed.is_empty()`,
  `findings.is_empty()`, `!failed()`; command: `cargo test -p xtask --lib
  bridge_versions::tests::lint_without_flag_has_empty_release_results`; expected:
  red before (new symbol absent), green after.
- `outcome_lines_are_empty_when_clean` — setup: `LintOutcome` with no findings
  and no release results; action: `lines()`; assert: `Vec::<String>::new()`,
  `!failed()`; command: `cargo test -p xtask --lib
  bridge_versions::tests::outcome_lines_are_empty_when_clean`.
- `outcome_fails_on_release_finding_only` — setup: `LintOutcome` with one
  `ReleaseFinding` and no offline findings; action: `failed()` and `lines()`;
  assert: `failed()` is true, `lines()` contains a line starting with
  `release finding: `; command: `cargo test -p xtask --lib
  bridge_versions::tests::outcome_fails_on_release_finding_only`.
- `outcome_reports_release_pass_alongside_offline_drift` — setup: `LintOutcome`
  with one offline `Finding` and `release_passed = [ReleasePass { tag: "jms-bridge-v0.8.8", assets: 4 }]`;
  action: `lines()`; assert: one line equals
  `release OK: jms-bridge-v0.8.8 (4 assets)` and one line contains the offline
  finding detail, `failed()` is true; command: `cargo test -p xtask --lib
  bridge_versions::tests::outcome_reports_release_pass_alongside_offline_drift`.

Scripted tests:

- `offline_path_byte_identical` — setup: step 0 baseline files; action: rebuild
  and run `./target/debug/xtask lint-bridge-versions` and
  `./target/debug/xtask lint-bridge-versions --tag cxf-bridge-v0.8.5` into
  `/tmp/blv-post.*`; assert: `diff -u /tmp/blv-pre.out /tmp/blv-post.out`,
  `diff -u /tmp/blv-pre.err /tmp/blv-post.err`, and the rc files are equal;
  command: `systemd-run --user --scope --collect --unit=fleet-bridgereleaseguard -p MemoryMax=12G env TMPDIR=/tmp bash -lc 'cd /home/shared/rust-camel-worktrees/bridgereleaseguard && cargo build -p xtask && ./target/debug/xtask lint-bridge-versions >/tmp/blv-post.out 2>/tmp/blv-post.err; echo $? >/tmp/blv-post.rc'`
  followed by the two `diff` calls and the tag variant; expected: red before
  (post files absent), green after.
- `network_negative_current_registry` — setup: committed registry; action: run
  the script below, which uses committed `0.9.1` when that release is absent and
  falls back to a transient scratch `0.0.0-does-not-exist` only when `0.9.1`
  has since been published. Both branches carry a mandatory restore trap:
  ```
  set -u
  fail() { echo "FAIL: $1" >&2; exit 1; }
  cp bridges/versions.toml /tmp/blv-neg-registry.bak
  trap 'cp /tmp/blv-neg-registry.bak bridges/versions.toml' EXIT
  tag="jms-bridge-v0.9.1"
  ./target/debug/xtask lint-bridge-versions --check-releases >/tmp/blv-neg.out 2>/tmp/blv-neg.err; rc=$?
  if ! grep -qF "release finding: $tag " /tmp/blv-neg.out; then
    grep -qF "release OK: $tag " /tmp/blv-neg.out || fail "no release verdict for $tag"
    sed -i 's/^jms = "0.9.1"$/jms = "0.0.0-does-not-exist"/' bridges/versions.toml
    tag="jms-bridge-v0.0.0-does-not-exist"
    ./target/debug/xtask lint-bridge-versions --check-releases >/tmp/blv-neg.out 2>/tmp/blv-neg.err; rc=$?
  fi
  [ "$rc" -ne 0 ] || fail "expected inner xtask non-zero exit"
  line="$(grep -F "release finding: $tag " /tmp/blv-neg.out | head -n1)"
  [ -n "$line" ] || fail "no release finding line for $tag"
  expected="release finding: $tag https://api.github.com/repos/kennycallado/rust-camel/releases/tags/$tag: release $tag not found - publish the release or fix bridges/versions.toml"
  [ "$line" = "$expected" ] || fail "not the exact 404 diagnostic: $line"
  case "$line" in
    *"transport error"*|*"unexpected HTTP status"*|*"invalid JSON"*)
      fail "infra error reported as a missing release: $line" ;;
  esac
  ! grep -qF "release OK: $tag " /tmp/blv-neg.out || fail "unexpected pass for $tag"
  cp /tmp/blv-neg-registry.bak bridges/versions.toml
  cmp /tmp/blv-neg-registry.bak bridges/versions.toml || fail "registry not restored"
  [ -z "$(git status --porcelain -- bridges/versions.toml)" ] || fail "registry dirty"
  echo "ok ($tag)"
  ```
  assert: the harness script exits 0 only when the inner `xtask` exited non-zero
  **and** printed the exact 404 diagnostic (full line equality) for `$tag`;
  transport, timeout, HTTP 500, and invalid-JSON variants fail the proof because
  they do not match that line or are rejected by the `case`. A harness exit 0
  means the proof held; it does not mean `lint-bridge-versions` passed. The
  script prints `ok (jms-bridge-v0.9.1)` or
  `ok (jms-bridge-v0.0.0-does-not-exist)`; command: the script body under
  `systemd-run --user --scope --collect --unit=fleet-bridgereleaseguard -p MemoryMax=12G env TMPDIR=/tmp bash -lc 'cd /home/shared/rust-camel-worktrees/bridgereleaseguard && <script>'`;
  expected: red before (`--check-releases` is an unknown argument, so the first
  invocation exits 2 and no `release finding` line exists), green after.
- `network_positive_scratch_registry` — setup: the restore trap and `0.8.8`
  edit from step 15; action: run `--check-releases`; assert: rc is non-zero
  (offline constant drift is reported), output contains
  `release OK: jms-bridge-v0.8.8 (4 assets)`, output contains
  `bridge version mismatch: registry key 'jms' expected '0.8.8' actual '0.9.1'`,
  and after restore `cmp /tmp/blv-registry.bak bridges/versions.toml` exits 0
  and `git status --porcelain -- bridges/versions.toml` is empty; command: step
  15 block plus `grep -F` assertions; expected: red before (flag unknown), green
  after.
- `ci_step_present_and_ordered` — setup: `.github/workflows/ci.yml`; action:
  locate the `lint-bridge-versions` and `lint-bridge-releases` step names;
  assert: both present with `run: cargo xtask lint-bridge-versions` and
  `run: cargo xtask lint-bridge-versions --check-releases` respectively, and the
  `lint-bridge-releases` name line number is greater than the
  `lint-bridge-versions` name line number; command:
  `grep -n 'name: lint-bridge' .github/workflows/ci.yml` and
  `grep -F 'run: cargo xtask lint-bridge-versions --check-releases' .github/workflows/ci.yml`;
  expected: red before (second grep empty), green after.
- `agents_gate_registered` — setup: `AGENTS.md`; action: inspect the
  `## QUALITY GATES` block; assert: it contains `- name: lint-bridge-releases`
  followed by `run: cargo xtask lint-bridge-versions --check-releases`; command:
  `grep -F 'lint-bridge-releases' AGENTS.md` and
  `grep -F 'run: cargo xtask lint-bridge-versions --check-releases' AGENTS.md`;
  expected: red before, green after.
- `gates_local_skips_by_name` — setup: updated `scripts/gates-local.sh`; action:
  `bash -n scripts/gates-local.sh` then
  `GATES_ONLY='lint-bridge-releases lint-bridge-versions' bash scripts/gates-local.sh >/tmp/gates.out 2>&1`;
  assert: rc 0, `/tmp/gates.out` contains `skipped: 1` and `lint-bridge-releases`,
  contains `PASS lint-bridge-versions`, and does not contain
  `=== lint-bridge-releases`; command:
  `systemd-run --user --scope --collect --unit=fleet-bridgereleaseguard -p MemoryMax=12G env TMPDIR=/tmp bash -lc 'cd /home/shared/rust-camel-worktrees/bridgereleaseguard && GATES_ONLY="lint-bridge-releases lint-bridge-versions" bash scripts/gates-local.sh >/tmp/gates.out 2>&1; echo $? >/tmp/gates.rc'`
  then the greps; expected: red before (`lint-bridge-releases` is parsed or
  missing), green after.

**Acceptance:**
- `cargo fmt --all -- --check` exits 0 in the worktree.
- `cargo clippy -p xtask --all-targets -- -D warnings` exits 0 in the worktree.
- `cargo test -p xtask --lib bridge_versions` exits 0 (all cases above).
- `cargo xtask lint-unwrap` exits 0.
- `offline_path_byte_identical` passes for both plain and `--tag` invocations.
- `network_negative_current_registry` harness exits 0 only when the inner
  `xtask` exited non-zero and printed the exact missing-release diagnostic
  (`release finding: <tag> <api-url>: release <tag> not found - publish the
  release or fix bridges/versions.toml`) for `jms-bridge-v0.9.1` when that
  release is absent, or for `jms-bridge-v0.0.0-does-not-exist` in the
  published-fallback branch. Transport, timeout, non-404 status, and
  invalid-JSON findings fail the proof. The harness also restores
  `bridges/versions.toml` byte-for-byte and leaves
  `git status --porcelain -- bridges/versions.toml` empty.
- `network_positive_scratch_registry` prints `release OK: jms-bridge-v0.8.8 (4 assets)`,
  reports the constant drift, exits non-zero overall, restores
  `bridges/versions.toml` byte-for-byte, and leaves `git status` clean for it.
- `ci_step_present_and_ordered`, `agents_gate_registered`, and
  `gates_local_skips_by_name` pass.
- `git status --porcelain` lists only the six intended paths above; no
  `bridges/**` source, bridge constant, or `crates/**/src/**` diff exists.
- No commit, tag, or push was made.

- [x] 1.1

## Phase 2: JMS published smoke

### `.github/workflows`

#### Task 2.1: Add the JMS published-bridge smoke workflow as a sibling of the CXF smoke

**Files:**
- `.github/workflows/published-jms-bridge-smoke.yml` (new)
- `.github/workflows/published-bridge-smoke.yml` (must stay unmodified; verified, never edited)
- `crates/camel-test/tests/jms_test.rs` (read-only reference for the test name)
- `crates/camel-test/Cargo.toml` (read-only reference for the feature gate)

**Steps:**

1. Create `.github/workflows/published-jms-bridge-smoke.yml` with header comment
   explaining the failure class, the trigger chain (`JMS Bridge Release` →
   `jms-bridge-v*`), the download URL shape
   `https://github.com/kennycallado/rust-camel/releases/download/jms-bridge-v<VER>/jms-bridge-<VER>-linux-musl-x86_64.tar.gz`,
   and a "how to read a failure" note. State that the CXF smoke is intentionally
   separate and unchanged.

2. Set `name: Published JMS Bridge Smoke` and triggers exactly:
   ```
   on:
     workflow_run:
       workflows: ["JMS Bridge Release"]
       types: [completed]
       branches:
         - 'jms-bridge-v*'
     workflow_dispatch:
       inputs:
         bridge_version:
           description: 'Bridge version (e.g. 0.8.8 or jms-bridge-v0.8.8). Empty = newest jms-bridge release.'
           required: false
           type: string
   ```
   Add `permissions: contents: read`, a `concurrency` group keyed on
   `github.event.inputs.bridge_version || github.event.workflow_run.head_branch || 'latest'`
   with `cancel-in-progress: false`, and the same `env` block as the CXF smoke.

3. Define job `smoke` with
   `if: github.event_name == 'workflow_dispatch' || github.event.workflow_run.conclusion == 'success'`,
   `runs-on: ubuntu-latest`, `timeout-minutes: 90`, and a job-level `env:` block
   setting `GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}` and
   `REPO: ${{ github.repository }}` so every `gh` step inherits both.

4. Steps in this exact order: `actions/checkout` (pin the same SHA as the CXF
   smoke); `actions/setup-java` with `java-version: '21'`, `distribution: 'temurin'`;
   `dtolnay/rust-toolchain` stable; `Swatinem/rust-cache`;
   `- name: Install libclang for bindgen` with
   `run: sudo apt-get install -y libclang-dev` (byte-identical to the
   `full-tests-linux` step in `.github/workflows/ci.yml`); then the five steps
   below. `jms_test` compiles bindgen-dependent workspace crates, so the apt
   install is required.

5. `- name: Resolve download version` (`id: version`). Mirror the CXF resolver
   but for JMS: take `github.event.inputs.bridge_version`, else
   `github.event.workflow_run.head_branch`, else the first tag from
   `gh api "repos/${REPO}/releases" --paginate --jq '.[].tag_name' | grep -E '^jms-bridge-v[0-9]' | head -n1`.
   It inherits `GH_TOKEN` and `REPO` from the job `env` block and defines no
   per-step copy. Strip the `jms-bridge-v` prefix, require
   `^[0-9]+\.[0-9]+\.[0-9]+`, write `version=<v>` to `$GITHUB_OUTPUT`.

6. `- name: Assert registry tag is published`, placed BEFORE the download step
   and independent of `steps.version.outputs.version`:
   ```
   registry_version="$(awk -F'"' '$1 ~ /^jms[[:space:]]*=/ {print $2; exit}' bridges/versions.toml)"
   test -n "$registry_version"
   registry_tag="jms-bridge-v${registry_version}"
   gh release view "$registry_tag" --repo "$REPO" >/dev/null
   ```
   On failure emit
   `::error::bridges/versions.toml declares ${registry_tag} but that release does not exist`
   and exit 1. It must never read `steps.version.outputs.version`.

7. `- name: Download and verify published linux-musl artifact` with this
   step-level YAML block (`GH_TOKEN` and `REPO` come from the job `env`):
   ```yaml
   env:
     VERSION: ${{ steps.version.outputs.version }}
   ```
   and this shell body:
   ```
   asset="jms-bridge-${VERSION}-linux-musl-x86_64.tar.gz"
   tag="jms-bridge-v${VERSION}"
   mkdir -p published
   gh release download "$tag" --pattern "$asset" --dir published
   gh release download "$tag" --pattern "jms-bridge-${VERSION}-SHA256SUMS" --dir published
   cd published
   grep " ${asset}$" "jms-bridge-${VERSION}-SHA256SUMS" | sha256sum -c -
   tar -xzf "$asset"
   test -x "jms-bridge-${VERSION}-linux-musl-x86_64/bin/jms-bridge"
   echo "CAMEL_JMS_BRIDGE_BINARY_PATH=$PWD/jms-bridge-${VERSION}-linux-musl-x86_64/bin/jms-bridge" >> "$GITHUB_ENV"
   ```
   The inner path is fixed by `package_release` in `scripts/xtask/src/main.rs`:
   `<binary>-<version>-<target>/bin/<binary>`, binary `jms-bridge`.

8. `- name: Run JMS consumer test against the published binary` with this
   step-level YAML key:
   ```yaml
   shell: bash
   ```
   `set +e` disables the implicit GitHub `-e`; the log file is
   created before `tail` starts; `wait` failure is captured without aborting so
   the diagnostics block always executes:
   ```
   set +e
   set -uo pipefail
   log="$RUNNER_TEMP/jms-smoke.log"
   : > "$log"
   setsid bash -c 'cargo test -p camel-test --features integration-tests --test jms_test \
       -- jms_consumer_receives_from_activemq --nocapture --test-threads=1 > "$1" 2>&1' _ "$log" &
   pid=$!
   tail -n +1 --pid="$pid" -f "$log" &
   tailpid=$!
   deadline=$((SECONDS + 4800))
   while kill -0 "$pid" 2>/dev/null && [ "$SECONDS" -lt "$deadline" ]; do sleep 5; done
   if kill -0 "$pid" 2>/dev/null; then
     echo "::error::test phase exceeded 4800s; killing the process group"
     kill -TERM -- "-$pid" 2>/dev/null || true; sleep 5; kill -KILL -- "-$pid" 2>/dev/null || true
     status=124
   else
     if wait "$pid"; then status=0; else status=$?; fi
   fi
   kill "$tailpid" 2>/dev/null || true
   if [ "$status" -ne 0 ]; then ps -eo pid,etime,args | grep -i '[j]ms-bridge' || true; fi
   grep -q 'jms_consumer_receives_from_activemq' "$log" || { echo "::error::test did not run"; exit 1; }
   exit "$status"
   ```
   Never pipe cargo output; redirect to a file and `tail --pid` it. Print
   `$CAMEL_JMS_BRIDGE_BINARY_PATH` and `ls -l` it before the run.

9. `- name: Kill leftover bridge processes` with `if: always()` and
   `run: pkill -f 'jms-bridge' || true`.

10. Do not edit `.github/workflows/published-bridge-smoke.yml`; verify it with
    `git diff --exit-code -- .github/workflows/published-bridge-smoke.yml`.

**Tests:** (executable spec — name, setup, action, assert, command, expected)

- `workflow_structure_and_yaml_parse` — setup: the new workflow file; action:
  write `/tmp/jms-smoke-structure.py` with:
  ```
  import yaml
  from pathlib import Path
  text = Path(".github/workflows/published-jms-bridge-smoke.yml").read_text()
  assert "\t" not in text, "workflow contains a tab"
  data = yaml.safe_load(text)
  print("top-level keys:", sorted(map(str, data.keys())))
  print("jobs:", sorted(data["jobs"].keys()))
  assert data["name"] == "Published JMS Bridge Smoke"
  assert "smoke" in data["jobs"], "missing smoke job"
  assert data["jobs"]["smoke"]["timeout-minutes"] == 90
  steps = [step.get("name", "") for step in data["jobs"]["smoke"]["steps"]]
  for need in ["Resolve download version", "Assert registry tag is published",
               "Download and verify published linux-musl artifact",
               "Run JMS consumer test against the published binary",
               "Kill leftover bridge processes"]:
      assert need in steps, need
  download = next(s for s in data["jobs"]["smoke"]["steps"]
                  if s.get("name") == "Download and verify published linux-musl artifact")
  assert download["env"]["VERSION"] == "${{ steps.version.outputs.version }}", \
      "download step must bind VERSION to the resolved version"
  run_step = next(s for s in data["jobs"]["smoke"]["steps"]
                  if s.get("name") == "Run JMS consumer test against the published binary")
  assert run_step["shell"] == "bash", "test step must use bash"
  assert "set +e" in text, "must disable inherited errexit"
  assert "set -uo pipefail" in text, "must set nounset and pipefail"
  assert ': > "$log"' in text, "log must exist before tail"
  assert 'if wait "$pid"; then status=0; else status=$?; fi' in text, \
      "failed wait must not abort before diagnostics"
  for needle in [
      'workflow_run:', 'workflows: ["JMS Bridge Release"]', "'jms-bridge-v*'",
      'workflow_dispatch:', 'bridge_version:',
      'VERSION: ${{ steps.version.outputs.version }}',
      'GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}',
      'REPO: ${{ github.repository }}',
      '- name: Install libclang for bindgen',
      'sudo apt-get install -y libclang-dev',
      'bridges/versions.toml', 'gh release view "$registry_tag"',
      'gh release download "$tag"', 'CAMEL_JMS_BRIDGE_BINARY_PATH',
      'sha256sum -c -', 'bin/jms-bridge',
      'cargo test -p camel-test --features integration-tests --test jms_test',
      'jms_consumer_receives_from_activemq', 'setsid',
      'kill -TERM -- "-$pid"', 'kill -KILL -- "-$pid"',
      'status=124', '::error::test phase exceeded 4800s',
      'if: always()', "pkill -f 'jms-bridge'",
  ]:
      assert needle in text, needle
  start = text.index("Assert registry tag is published")
  end = text.index("- name: Download", start)
  assert "steps.version.outputs.version" not in text[start:end]
  assert text.count("GH_TOKEN:") == 1, "GH_TOKEN must be job-level only"
  assert text.count("REPO:") == 1, "REPO must be job-level only"
  print("workflow structure OK")
  ```
  assert: the script exits 0, prints `jobs: ['smoke']`, and prints
  `workflow structure OK`; command: run from the worktree root with PyYAML
  available —
  `nix shell nixpkgs#python3Packages.pyyaml --command python3 /tmp/jms-smoke-structure.py`
  (if the ambient `python3` already imports `yaml`, plain
  `python3 /tmp/jms-smoke-structure.py` is equivalent); expected: red before
  (file absent → `FileNotFoundError`), green after.
- `registry_tag_extraction` — setup: committed `bridges/versions.toml` and a
  temp file with `jms = "0.9.1"`; action: run
  `awk -F'"' '$1 ~ /^jms[[:space:]]*=/ {print $2; exit}'` on both; assert:
  output is `0.9.1` for both, and `jms-bridge-v0.9.1` is the derived tag; command:
  `test "$(awk -F'"' '$1 ~ /^jms[[:space:]]*=/ {print $2; exit}' bridges/versions.toml)" = "0.9.1"`;
  expected: green before and after — this characterizes the extraction contract;
  the red-before coverage for the workflow file is owned by
  `workflow_structure_and_yaml_parse`.
- `test_name_and_feature_exist` — setup: the repo; action:
  `grep -F 'async fn jms_consumer_receives_from_activemq' crates/camel-test/tests/jms_test.rs`
  and `grep -F 'required-features = ["integration-tests"]' crates/camel-test/Cargo.toml`;
  assert: both match; command as written; expected: green before and after
  (guards against a renamed test; if red, stop and report, do not rename).
- `cxf_workflow_unchanged` — setup: the branch; action:
  `git diff --exit-code -- .github/workflows/published-bridge-smoke.yml` and
  `git status --porcelain -- .github/workflows/published-bridge-smoke.yml`;
  assert: both exit 0 / produce no output; command as written; expected: green
  after; red if any accidental edit lands.
- `download_layout_matches_packager` — setup: `scripts/xtask/src/main.rs`; action:
  inspect `package_release`; assert: the workflow's asserted inner path matches
  `format!("{binary_name}-{version}-{target}")` + `/bin/` + `binary_name` with
  `binary_name = "jms-bridge"` and `target = "linux-musl-x86_64"`; command:
  `grep -n 'let dist_name = format!' scripts/xtask/src/main.rs` and
  `grep -F 'jms-bridge-${VERSION}-linux-musl-x86_64/bin/jms-bridge' .github/workflows/published-jms-bridge-smoke.yml`;
  expected: green after.

**Acceptance:**
- The new workflow file exists, `yaml.safe_load` succeeds, the script prints
  `jobs: ['smoke']`, and every assertion in
  `workflow_structure_and_yaml_parse` passes.
- The parsed file carries job-level `GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}` and
  `REPO: ${{ github.repository }}` (one occurrence each), the download step's
  `env.VERSION == "${{ steps.version.outputs.version }}"`, the libclang apt step
  matching `ci.yml` `full-tests-linux`, and the test step's `shell: bash` with
  `set +e`, `set -uo pipefail`, pre-created log, and non-aborting `wait`.
- `registry_tag_extraction` prints `0.9.1` and the derived tag
  `jms-bridge-v0.9.1` (green before and after).
- `test_name_and_feature_exist` passes without modifying `camel-test`.
- `.github/workflows/published-bridge-smoke.yml` has no diff and no untracked
  changes.
- `download_layout_matches_packager` passes.
- No Rust build or test is run for this task; no release-registry or `gh` API
  call is made locally.
- No commit, tag, or push was made.

- [x] 2.1

## Phase 3: bump procedure

### `.opencode/prompts`

#### Task 3.1: Rewrite `bump-bridge.md` around `bridges/versions.toml`

**Files:**
- `.opencode/prompts/bump-bridge.md` (modified)

**Steps:**

1. Replace the whole file. Open with a one-paragraph statement that
   `bridges/versions.toml` is the single version source for `cxf`, `jms`, and
   `xml`, and that the tag `{bridge}-bridge-vX.Y.Z` is the release trigger for
   `.github/workflows/{bridge}-bridge-release.yml`.

2. Add a table with columns
   `Bridge | Registry key | BRIDGE_VERSION const(s) | Tag`, rows:
   - `cxf` | `cxf` | `crates/components/camel-cxf/src/lib.rs` | `cxf-bridge-vX.Y.Z`
   - `jms` | `jms` | `crates/components/camel-jms/src/lib.rs` | `jms-bridge-vX.Y.Z`
   - `xml` | `xml` | `crates/components/camel-validator/src/lib.rs` **and**
     `crates/components/camel-xslt/src/lib.rs` | `xml-bridge-vX.Y.Z`
   State explicitly that `xml` feeds both components and both constants move
   together.

3. Add the numbered procedure, per bridge, OLD → NEW:
   1. Run `cargo xtask bump-bridge <bridge> <version>`. State that it atomically
      writes `bridges/versions.toml` and the registered `BRIDGE_VERSION`
      constant(s), and restores every target on failure.
   2. Run `cargo xtask lint-bridge-versions`; require a clean exit before
      committing.
   3. Commit with `caveman-commit`: `chore(bridge): bump version OLD to NEW`,
      body only if non-obvious, ending with a `Bd:` line naming the tracked
      issue (for example `Bd: rc-bxjzq`).
   4. Stop. State exactly: `The agent never tags, pushes, or triggers a release.`
      State exactly: `The human creates and pushes the tag {bridge}-bridge-vX.Y.Z.`
      State that the tag push is what starts the release workflow.

4. Add a `## After the tag is pushed (human-run verification)` section:
   - Run `cargo xtask lint-bridge-versions --check-releases`; it must exit 0.
   - Confirm the release carries `{bridge}-bridge-{version}-linux-musl-x86_64.tar.gz`,
     `{bridge}-bridge-{version}-macos-aarch64.tar.gz`,
     `{bridge}-bridge-{version}-windows-x86_64.zip`, and
     `{bridge}-bridge-{version}-SHA256SUMS`.
   - Confirm the published tag equals the registry version.
   - State that `--check-releases` fails before the release exists, and that this
     is expected, not a bump failure; the bump is not complete until the release
     exists and the check is green.
   - Point at the JMS smoke workflow
     `.github/workflows/published-jms-bridge-smoke.yml` as the artifact check.

5. Remove every Gradle-literal instruction: no `build.gradle.kts`, no
   `sed -i`, no `?: "OLD"`, no "gradle default version" column, and no
   `git tag` command executed by the agent. Do not keep the old line numbers.

6. Keep the file in English and do not add a `## QUALITY GATES` edit here (that
   is Phase 1).

**Tests:** (executable spec — name, setup, action, assert, command, expected)

- `prompt_requires_registry_path` — setup: the rewritten prompt; action: run
  `grep -F` for each required string; assert all present:
  `bridges/versions.toml`, `cargo xtask bump-bridge`,
  `cargo xtask lint-bridge-versions`, `cargo xtask lint-bridge-versions --check-releases`,
  `crates/components/camel-validator/src/lib.rs`,
  `crates/components/camel-xslt/src/lib.rs`,
  `crates/components/camel-jms/src/lib.rs`, `BRIDGE_VERSION`,
  `linux-musl-x86_64.tar.gz`, `macos-aarch64.tar.gz`, `windows-x86_64.zip`,
  `SHA256SUMS`, `jms-bridge-v`; command:
  `grep -F 'cargo xtask bump-bridge' .opencode/prompts/bump-bridge.md` and the
  same for each needle; expected: red before (the old prompt lacks
  `cargo xtask bump-bridge` and `--check-releases`), green after.
- `prompt_forbids_gradle_and_agent_tagging` — setup: the rewritten prompt;
  action: `! grep -F`; assert absent: `build.gradle.kts`, `sed -i`,
  `gradle default`, `?: "`, `Local tag:`, `git tag {bridge}`; command:
  `! grep -F 'build.gradle.kts' .opencode/prompts/bump-bridge.md` and the same
  for each forbidden needle; expected: red before (old prompt contains the first
  four), green after.
- `prompt_states_human_pushes_and_verifies` — setup: the rewritten prompt;
  action: `grep -F`; assert present:
  `The agent never tags, pushes, or triggers a release.`,
  `The human creates and pushes the tag`, `--check-releases`,
  `not a bump failure`, `linux-musl-x86_64.tar.gz`,
  `published-jms-bridge-smoke.yml`; command: `grep -F 'The agent never tags,
  pushes, or triggers a release.' .opencode/prompts/bump-bridge.md` and the
  same for each needle; expected: red before, green after.
- `prompt_only_change` — setup: the branch; action: `git status --porcelain`;
  assert: the only modified path for this task is
  `.opencode/prompts/bump-bridge.md`; command: `git status --porcelain --
  .opencode/prompts/bump-bridge.md`; expected: red before (no diff), green after
  (one `M` line).

**Acceptance:**
- `prompt_requires_registry_path`, `prompt_forbids_gradle_and_agent_tagging`,
  and `prompt_states_human_pushes_and_verifies` pass.
- `prompt_only_change` shows exactly one modified path in this task's scope.
- The prompt contains no Gradle literal, no `sed` edit, and no agent-side tag
  or push instruction.
- No code build is needed; no other file is edited.
- No commit, tag, or push was made.

- [x] 3.1

## Spec coverage self-review

Every `### Scenario` in `specs/bridge-release-guard/spec.md` has an owning test
or proof:

| Spec scenario | Owning evidence |
|---|---|
| Published release with every matrix asset passes | `classify_published_release_passes`, `network_positive_scratch_registry` |
| Declared but never released version fails with the exact URL | `classify_404_names_api_url`, `network_negative_current_registry` |
| Published release missing a platform asset fails | `classify_missing_windows_asset_fails`, `classify_missing_checksums_fails`, `classify_each_missing_asset_is_a_separate_finding` |
| Registry outage fails closed | `classify_non_404_status_fails_closed`, `classify_invalid_json_fails_closed`, `classify_missing_body_fails_closed`, `classify_transport_error_fails_closed` |
| Default invocation has no network effect | `offline_path_byte_identical`, `lint_without_flag_has_empty_release_results` |
| Tag validation is unchanged | `offline_path_byte_identical` (tag variant), existing `mismatched_tag_prints_exact_diagnostic` |
| Offline drift does not suppress release checks | `network_positive_scratch_registry`, `outcome_reports_release_pass_alongside_offline_drift` |
| CI executes the gate | `ci_step_present_and_ordered` |
| Local sweep skips the gate by name | `gates_local_skips_by_name` |
| Post-release smoke runs the published JMS binary | `workflow_structure_and_yaml_parse` (parsed `env.VERSION` binding, `shell: bash` guards, download/checksum/env/test needles), `download_layout_matches_packager` |
| Dispatch override cannot hide an unreleased registry version | `workflow_structure_and_yaml_parse` (parsed job/steps plus registry assertion slice with no `steps.version.outputs.version`), `registry_tag_extraction` |
| Timeout kills the process group | `workflow_structure_and_yaml_parse` (`setsid`, `status=124`, `::error::test phase exceeded 4800s`, `kill -TERM -- "-$pid"`, `kill -KILL -- "-$pid"`, `timeout-minutes: 90`, `pkill -f 'jms-bridge'` under `if: always()`) |
| Bump uses the registry path | `prompt_requires_registry_path`, `prompt_forbids_gradle_and_agent_tagging`, `prompt_states_human_pushes_and_verifies` |
| Post-tag verification catches an unpublished version | `prompt_states_human_pushes_and_verifies` (`--check-releases`, `not a bump failure`), `network_negative_current_registry` |
| Published scratch version demonstrates the pass path | `network_positive_scratch_registry` |
| Scratch change is reverted | `network_positive_scratch_registry` (`cmp` + `git status --porcelain -- bridges/versions.toml`) |

Placeholder scan: PASS. The document contains no deferred-content token, no
ellipsis used as content, no vague "handle edge cases" instruction, and no
cross-task "similar to" reference; every test names its setup, action,
assertion, command, and expected pre-implementation result.

NEW-symbol consistency: `ReleaseFinding`, `ReleasePass`, `release_tag`,
`release_api_url`, `release_download_base`, `expected_assets`,
`classify_response`, `classify_transport_error`, `check_releases` are introduced
in Task 1.1 step 1 and used with the same signatures in every later step and
test. `LintOutcome::failed` and `LintOutcome::lines` are introduced in Task 1.1
step 10 and used only by Task 1.1 step 13 and its tests. No later task refers to
an undefined symbol.
