# Tasks: binstall-metadata

## binstall-metadata

### Task 1.1: Add [package.metadata.binstall] block with manifest-contract tests

**Files:**
- `crates/camel-cli/Cargo.toml` (modified)
- `crates/camel-cli/tests/binstall_metadata.rs` (new)

**Steps:**
1. In `crates/camel-cli/Cargo.toml`, after the `[features]` table (which
   stays untouched — `default = ["flavor-regular"]` is already correct),
   add exactly:
   ```toml
   # Prebuilt installs: `cargo binstall camel-cli` fetches the flat tarball
   # from GitHub releases. Flavor selection is cfg(target)-only (binstall
   # has no feature variable): full everywhere it exists, regular on musl
   # (platform ceiling — librdkafka cannot build there). Literal `.tar.gz`
   # on purpose: {archive-suffix} renders `.tgz` for the tgz format and
   # would not match our asset names.
   [package.metadata.binstall]
   pkg-url = "{repo}/releases/download/v{version}/camel-full-{target}.tar.gz"
   pkg-fmt = "tgz"
   bin-dir = "{bin}{binary-ext}"
   disabled-strategies = ["quick-install"]

   [package.metadata.binstall.overrides.'cfg(all(target_os = "linux", target_env = "musl"))']
   pkg-url = "{repo}/releases/download/v{version}/camel-{target}.tar.gz"
   ```
2. Add `toml.workspace = true` to `[dev-dependencies]` in the same file
   (workspace already declares `toml = "1"`).
3. Create `crates/camel-cli/tests/binstall_metadata.rs` — a manifest-contract
   test that parses `concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")`
   with the `toml` crate and asserts the metadata contract (test bodies
   specified below; one `#[test]` fn per bullet, shared helper
   `fn manifest() -> toml::Value` that parses the file once per test).

**Tests:** (executable spec — name, arrange, act, assert)
- `metadata_block_fields`: parse manifest → `["package"]["metadata"]["binstall"]` exists → assert `pkg-url` equals `{repo}/releases/download/v{version}/camel-full-{target}.tar.gz` (exact string), `pkg-fmt` equals `tgz`, `bin-dir` equals `{bin}{binary-ext}`, `disabled-strategies` equals the one-element array `["quick-install"]`.
- `musl_override_targets_regular`: parse manifest → `["package"]["metadata"]["binstall"]["overrides"]` contains key `cfg(all(target_os = "linux", target_env = "musl"))` → its `pkg-url` equals `{repo}/releases/download/v{version}/camel-{target}.tar.gz` (regular prefix, exact string) and the override table introduces no other keys.
- `pkg_url_has_single_url_guarantee`: parse manifest → default `pkg-url` and override `pkg-url` contain none of the substrings `{archive-suffix}`, `{archive-format}`, `{format}` (binstall only iterates format extensions when one of those keys is present — their absence guarantees exactly one URL per target).
- `default_features_still_regular`: parse manifest → `["features"]["default"]` equals `["flavor-regular"]` (guards against an accidental default flip re-coupling compile fallback to a different flavor).
- `command`: `cargo test -p camel-cli --test binstall_metadata` — all four pass; `expected`: fails before step 1 (table absent → `metadata_block_fields` panics on missing key), passes after.

**Acceptance:**
- `cargo test -p camel-cli --test binstall_metadata` exits 0.
- `cargo fmt --check --all` exits 0 and `cargo clippy -p camel-cli -- -D warnings` exits 0 (no code changed, but the gate must stay green).
- `git diff crates/camel-cli/Cargo.toml` touches only: the new metadata block (+comment) and the `toml` dev-dependency line.

- [x] 1.1

### Task 1.2: Smoke check script — every supported target maps to a real release asset

**Files:**
- `scripts/binstall-smoke.sh` (new)

**Steps:**
1. Create `scripts/binstall-smoke.sh` (executable, `#!/usr/bin/env bash`,
   `set -euo pipefail`) that answers ONE question: does the metadata in
   `crates/camel-cli/Cargo.toml` resolve every supported target to an asset
   that exists in a given release?
2. Interface:
   - `scripts/binstall-smoke.sh [--repo OWNER/NAME] [--release TAG] [--assets-file PATH]`
   - defaults: `--repo kennycallado/rust-camel`, `--release` REQUIRED
     (no default — always explicit), `--assets-file` optional (test hook:
     newline-separated asset names instead of the GitHub API).
3. Supported targets (the closed set from the spec scenarios):
   `x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu
   x86_64-unknown-linux-musl aarch64-unknown-linux-musl
   x86_64-apple-darwin aarch64-apple-darwin x86_64-pc-windows-msvc`.
4. Resolution (single source of truth — template FROM the manifest, never
   hardcode): read the two `pkg-url` templates out of
   `crates/camel-cli/Cargo.toml` (python3 one-liner with `tomllib`,
   printing default and override templates); musl targets use the override
   template, all others the default; substitute `{repo}` →
   `https://github.com/OWNER/NAME`, `{version}` → release tag without the
   leading `v`, `{target}` → the target triple; extract the basename after
   the final `/` as the expected asset name.
5. Asset list: `--assets-file` if given, else the pagination-exact
   two-call form: `ID=$(gh api repos/OWNER/NAME/releases/tags/TAG
   --jq '.id')` then
   `gh api "repos/OWNER/NAME/releases/ID/assets?per_page=100"
   --jq '.[].name'` (the tag endpoint's embedded assets array is capped;
   the sub-endpoint paginates explicitly).
6. Assert membership for all 7 targets in ONE collection loop (no early
   exit inside the loop — a bare `grep -q` under `set -euo pipefail` would
   abort at the first miss): on success print `smoke: 7/7 targets resolve`
   and exit 0. Print one
   `FAIL: <target> -> <expected-name> (missing from release TAG)` line per
   miss to stderr, then after the loop `exit 1` if any miss was collected.
7. If `cargo-binstall` is on PATH AND `--assets-file` was NOT given, also
   run `cargo binstall --dry-run --no-confirm
   --manifest-path crates/camel-cli/Cargo.toml --target <t> camel-cli`
   per target as a cross-check of the resolved URL. `--manifest-path` is
   cargo-binstall's documented flag for pre-publish metadata testing;
   pinning `camel-cli@<semver>` would read the PUBLISHED registry manifest,
   which predates this change and carries no metadata. Informational:
   prefix output `binstall-dryrun:`; a binstall mismatch does not fail the
   script — the manifest+asset-list assertion is the contract.

**Tests:** (executable spec — run them as shell verifications)
- `smoke_pass_real_release`: action: `scripts/binstall-smoke.sh --release v0.51.0-rc.2` → assert: exit 0 and stdout contains `smoke: 7/7 targets resolve`.
- `smoke_fail_on_missing_full_asset`: setup: temp assets file = the real 11 rc.2 tarball names minus `camel-full-x86_64-apple-darwin.tar.gz`; action: `scripts/binstall-smoke.sh --release v0.51.0-rc.2 --assets-file <tmp>` → assert: exit 1 and stderr contains `FAIL: x86_64-apple-darwin -> camel-full-x86_64-apple-darwin.tar.gz`.
- `smoke_fail_on_missing_musl_regular`: setup: temp assets file = the 11 names minus `camel-aarch64-unknown-linux-musl.tar.gz`; action: same invocation → assert: exit 1 and stderr names `aarch64-unknown-linux-musl -> camel-aarch64-unknown-linux-musl.tar.gz` (proves the musl override selects regular, not full).
- `smoke_reports_all_misses`: setup: temp assets file = the 11 names minus BOTH `camel-full-x86_64-apple-darwin.tar.gz` AND `camel-aarch64-unknown-linux-musl.tar.gz`; action: same invocation → assert: exit 1 and stderr contains BOTH `FAIL: x86_64-apple-darwin -> camel-full-x86_64-apple-darwin.tar.gz` AND `FAIL: aarch64-unknown-linux-musl -> camel-aarch64-unknown-linux-musl.tar.gz` (the loop collects all misses; no early abort).
- `smoke_uses_manifest_templates`: setup: temp assets file where `camel-full-x86_64-unknown-linux-gnu.tar.gz` is renamed to `camel-x86_64-unknown-linux-gnu-full.tgz`; action: same invocation → assert: exit 1 (the script derives names from the manifest templates, so a `.tgz` reorder never passes).
- `command`: the five invocations above, run from the repo root; `expected`: scripts 2–5 fail before any fix (the script does not exist → nonzero), script 1 passes once Tasks 1–2 are implemented.

**Acceptance:**
- The five shell verifications behave exactly as specified (1 pass, 4 fails with the exact `FAIL:` lines).
- `bash -n scripts/binstall-smoke.sh` exits 0; `shellcheck scripts/binstall-smoke.sh` (if installed) reports no errors.
- The script contains no hardcoded pkg-url template strings — grep for `camel-full-{target}` in the script returns only the python3/tomllib extraction, not a literal template assignment.

- [x] 1.2

### Task 1.3: Install-path documentation

**Files:**
- `README.md` (modified)
- `docs/src/operations/distribution-flavors.md` (modified)

**Steps:**
1. In `README.md`, under the `### Use the CLI` heading (where the
   `cargo install` fence already lives), add the binstall line as the first
   bullet BEFORE the `cargo install` fence, matching the existing list
   style:
   ```markdown
   - `cargo binstall camel-cli` — installs the fullest prebuilt binary for
     your platform from GitHub releases (regular on musl; other flavors via
     docker tags, direct release download, or `cargo install --features`).
   ```
2. In `docs/src/operations/distribution-flavors.md`, in the section that
   lists how each flavor reaches users, add the same binstall sentence
   adapted to the doc's voice, plus one clarifying sentence: prebuilts via
   binstall start at the first metadata-bearing release (0.51.0); older
   versions carry no metadata — no project-hosted prebuilt routing, and
   binstall may serve a third-party quickinstall build (guaranteed source
   compile: `cargo install camel-cli --version <version>`, regular flavor).

**Tests:** (executable spec — grep verifications)
- `readme_mentions_binstall`: action: `grep -n 'cargo binstall camel-cli' README.md` → assert: at least one hit; and `grep -n 'musl' README.md` → assert: at least one hit within 5 lines of the binstall hit (the parenthetical).
- `flavors_doc_mentions_binstall_and_floor`: action: `grep -n 'cargo binstall camel-cli' docs/src/operations/distribution-flavors.md` → assert: at least one hit; and `grep -n '0.51.0' docs/src/operations/distribution-flavors.md` → assert: at least one hit (the metadata floor).
- `command`: the two grep pairs above; `expected`: no hits before the edit, hits after.

**Acceptance:**
- Both grep verifications pass.
- No other documentation file gained binstall claims (single-source the guidance: `git diff --name-only` shows only README.md and distribution-flavors.md among docs).
- `cargo xtask lint-context-citations` exits 0 if the touched docs carry citation markers (verify; do not add new markers unless the file's existing convention requires them).

- [x] 1.3


> POSTSCRIPT (e_gpt merge-gate review, 2026-09-20): the original blessed
> wording claimed older versions "fall back to a source compile". That was
> wrong: versions without metadata have no disabled-strategies either, so
> binstall may serve a third-party quickinstall build before compile. The
> floor sentence in this task, ALL occurrences across proposal.md (Why and
> Risk budget) and design.md (Approach), and the shipped doc
> (distribution-flavors.md, commit f31db0eb) were corrected post-bless to
> the quickinstall-accurate wording. Ledger re-hashed and re-blessed.
