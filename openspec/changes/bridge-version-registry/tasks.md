# Tasks: bridge-version-registry

Spec-bless: `sha256:e66253707f810706abcfd9719b855abd7bb9b9b5a7d113770fa076ba3a023870`.

## Execution constraints

- Set the executor working directory (`workdir`) from the command registry.
  Never put `cd` in a shell command. Never run a build or test in the main
  checkout.
- Every `cargo` command below is already wrapped in
  `systemd-run --user --scope --collect` with `MemoryMax=12G`,
  `MemorySwapMax=2G`, `CPUQuota=400%`, `TasksMax=1024`, `OOMPolicy=kill`,
  `TMPDIR=/home/shared/tmp`, `CARGO_BUILD_JOBS=6`, and
  `RUSTC_WRAPPER=sccache`. Keep the wrapper when running a command.
- Do not run `bump-bridge` against this repository. Bump verification uses
  fixture workspaces only, so no version in `bridges/versions.toml` or a
  registered constant changes.
- Stop each scope unit after it exits when the unit still exists: run
  `systemctl --user stop` with the exact unit name used in that command, for
  example `systemctl --user stop fleet-rc0fdrp-1a.scope`.

## Command registry

Set the executor working directory (`workdir`) to the absolute path named for
each command. No shell command uses `cd`.

- `W_ROOT`: `/home/shared/rust-camel-worktrees/379-bridge-version-registry`
- `W_CXF`: `/home/shared/rust-camel-worktrees/379-bridge-version-registry/bridges/cxf`
- `W_JMS`: `/home/shared/rust-camel-worktrees/379-bridge-version-registry/bridges/jms`
- `W_XML`: `/home/shared/rust-camel-worktrees/379-bridge-version-registry/bridges/xml`
- `W_DOCS`: `/home/shared/rust-camel-worktrees/379-bridge-version-registry/docs`

`C1` — workdir `W_ROOT`; registry parser tests:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-1a -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 RUSTC_WRAPPER=sccache SCCACHE_DIR=/home/shared/sccache cargo test -p xtask --lib bridge_versions::registry::tests -j4
```

`C2` — workdir `W_ROOT`; registry-to-constant tests:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-1b -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 RUSTC_WRAPPER=sccache SCCACHE_DIR=/home/shared/sccache cargo test -p xtask --lib bridge_versions::tests -j4
```

`C3` — workdir `W_ROOT`; contextual live-set tests:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-2a -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 RUSTC_WRAPPER=sccache SCCACHE_DIR=/home/shared/sccache cargo test -p xtask --lib bridge_versions::scan::tests -j4
```

`C4` — workdir `W_ROOT`; tag-mode tests:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-2b -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 RUSTC_WRAPPER=sccache SCCACHE_DIR=/home/shared/sccache cargo test -p xtask --lib bridge_versions::tests -j4
```

`C5` — workdir `W_ROOT`; atomic bump tests:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-4a -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 RUSTC_WRAPPER=sccache SCCACHE_DIR=/home/shared/sccache cargo test -p xtask --lib bridge_versions::bump::tests -j4
```

`C6` — workdir `W_ROOT`; workflow YAML/gate tests:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-4b -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 RUSTC_WRAPPER=sccache SCCACHE_DIR=/home/shared/sccache cargo test -p xtask --test bridge_release_workflows -j4
```

`C7` — workdir `W_ROOT`; all xtask tests:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-4c -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 RUSTC_WRAPPER=sccache SCCACHE_DIR=/home/shared/sccache cargo test -p xtask -j4
```

`C8` — workdir `W_ROOT`; xtask clippy:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-4d -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 RUSTC_WRAPPER=sccache SCCACHE_DIR=/home/shared/sccache cargo clippy -p xtask --all-targets -j4 -- -D warnings
```

`C9` — workdir `W_ROOT`; formatting:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-4e -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp cargo fmt --check --all
```

`C10` — workdir `W_ROOT`; repository bridge-version lint:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-3a -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 RUSTC_WRAPPER=sccache SCCACHE_DIR=/home/shared/sccache cargo xtask lint-bridge-versions
```

`C11` — workdir `W_ROOT`; matching release tag:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-3b -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 RUSTC_WRAPPER=sccache SCCACHE_DIR=/home/shared/sccache cargo xtask lint-bridge-versions --tag cxf-bridge-v0.8.4
```

`C12` — workdir `W_ROOT`; mismatched release tag:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-3c -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 RUSTC_WRAPPER=sccache SCCACHE_DIR=/home/shared/sccache cargo xtask lint-bridge-versions --tag xml-bridge-v0.8.2
```

`C13` — workdir `W_ROOT`; schema check:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-4f -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 RUSTC_WRAPPER=sccache SCCACHE_DIR=/home/shared/sccache cargo xtask schema --check
```

`G1a` — workdir `W_CXF`; CXF default:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-g1a -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp nix shell nixpkgs#jdk21_headless -c ./gradlew -q properties
```

`G1b` — workdir `W_JMS`; JMS default:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-g1b -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp nix shell nixpkgs#jdk21_headless -c ./gradlew -q properties
```

`G1c` — workdir `W_XML`; XML default:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-g1c -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp nix shell nixpkgs#jdk21_headless -c ./gradlew -q properties
```

`G2a` — workdir `W_CXF`; CXF override:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-g2a -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp nix shell nixpkgs#jdk21_headless -c ./gradlew -q properties -Pversion=9.9.9
```

`G2b` — workdir `W_JMS`; JMS override:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-g2b -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp nix shell nixpkgs#jdk21_headless -c ./gradlew -q properties -Pversion=9.9.9
```

`G2c` — workdir `W_XML`; XML override:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-g2c -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp nix shell nixpkgs#jdk21_headless -c ./gradlew -q properties -Pversion=9.9.9
```

`G3` — workdir `W_ROOT`; no Gradle literal:

```bash
if grep -rn '0\.8\.' bridges/cxf/build.gradle.kts bridges/jms/build.gradle.kts bridges/xml/build.gradle.kts; then exit 1; else echo 'no literal bridge version in Gradle'; fi
```

`G4a` — workdir `W_ROOT`; CXF override with no parent registry:

```bash
set -euo pipefail
root=$(mktemp -d /home/shared/tmp/rc0fdrp-g4a.XXXXXX)
trap 'rm -rf "$root"' EXIT
mkdir "$root/bridge"
cp -a bridges/cxf/. "$root/bridge"/
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-g4a -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill --setenv=APP_HOME="$root/bridge" -- env TMPDIR=/home/shared/tmp nix shell nixpkgs#jdk21_headless -c "$root/bridge/gradlew" -p "$root/bridge" -q properties -Pversion=9.9.9
```

`G4b` — workdir `W_ROOT`; JMS override with no parent registry:

```bash
set -euo pipefail
root=$(mktemp -d /home/shared/tmp/rc0fdrp-g4b.XXXXXX)
trap 'rm -rf "$root"' EXIT
mkdir "$root/bridge"
cp -a bridges/jms/. "$root/bridge"/
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-g4b -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill --setenv=APP_HOME="$root/bridge" -- env TMPDIR=/home/shared/tmp nix shell nixpkgs#jdk21_headless -c "$root/bridge/gradlew" -p "$root/bridge" -q properties -Pversion=9.9.9
```

`G4c` — workdir `W_ROOT`; XML override with no parent registry:

```bash
set -euo pipefail
root=$(mktemp -d /home/shared/tmp/rc0fdrp-g4c.XXXXXX)
trap 'rm -rf "$root"' EXIT
mkdir "$root/bridge"
cp -a bridges/xml/. "$root/bridge"/
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-g4c -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill --setenv=APP_HOME="$root/bridge" -- env TMPDIR=/home/shared/tmp nix shell nixpkgs#jdk21_headless -c "$root/bridge/gradlew" -p "$root/bridge" -q properties -Pversion=9.9.9
```

Each G4 command runs the copied wrapper through
`nix shell nixpkgs#jdk21_headless -c` with `-p "$root/bridge"`, like G1 and G2,
and never changes directory. The wrapper resolves `APP_HOME` from the working
directory, so `--setenv=APP_HOME="$root/bridge"` points it at the copied
wrapper's jar without changing directory.

`R1` — workdir `W_ROOT`; no live pin in rewritten files:

```bash
if rg -n '0\.8\.[0-9]' examples/cxf-example/Camel.toml crates/components/camel-cxf/README.md crates/components/camel-jms/README.md crates/components/camel-xj/README.md docs/src/components/ws-soap.md docs/src/components/rabbitmq.md bridges/cxf/README.md examples/jms-container-example/src/main.rs; then exit 1; else echo 'rewritten files contain no bridge pin'; fi
```

`R2` — workdir `W_ROOT`; JMS-only drift test deleted:

```bash
if rg -n 'mod bridge_version_test|bridge_version_pin_matches_gradle_default|locate_build_gradle' crates/components/camel-jms/src/lib.rs; then exit 1; else echo 'JMS drift test deleted'; fi
```

`R3` — workdir `W_ROOT`; ws-soap capability guidance retained:

```bash
set -e; for term in 'allow_expired_pinned_certificate' 'one-way' 'CAMEL_CXF_BRIDGE_BINARY_PATH' 'camel_cxf::BRIDGE_VERSION'; do rg -q --fixed-strings "$term" docs/src/components/ws-soap.md || { echo "missing $term"; exit 1; }; done; echo 'ws-soap guidance retained'
```

`R4` — workdir `W_ROOT`; JMS/CXF/XJ prose retained:

```bash
set -e; rg -q 'redeliver' crates/components/camel-jms/README.md; rg -q 'sendAcksAsync|blockOnAcknowledge' crates/components/camel-jms/README.md; rg -q 'labelled `dev`' bridges/cxf/README.md; rg -q 'build-cxf-bridge` without' bridges/cxf/README.md; rg -q --fixed-strings 'bridge_version: camel_xslt::BRIDGE_VERSION.to_string()' crates/components/camel-xj/README.md; echo 'capability prose retained'
```

`M1` — workdir `W_DOCS`; docs guide tests:

```bash
systemd-run --user --scope --collect --unit=fleet-rc0fdrp-m1 -p MemoryMax=12G -p MemorySwapMax=2G -p CPUQuota=400% -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp mdbook test
```

`V1` — workdir `W_ROOT`; no registry, constant, or history-tree change:

```bash
set -euo pipefail
test "$(grep -E '^(cxf|jms|xml) = ' bridges/versions.toml)" = "$(printf 'cxf = "0.8.4"\njms = "0.8.4"\nxml = "0.8.1"')"
for f in crates/components/camel-cxf/src/lib.rs crates/components/camel-jms/src/lib.rs crates/components/camel-xslt/src/lib.rs crates/components/camel-validator/src/lib.rs; do
  cur=$(grep -oE 'pub const BRIDGE_VERSION: &str = "[^"]+"' "$f" | sed -E 's/.*"([^"]+)"$/\1/')
  base=$(git show main:"$f" | grep -oE 'pub const BRIDGE_VERSION: &str = "[^"]+"' | sed -E 's/.*"([^"]+)"$/\1/')
  test "$cur" = "$base"
done
test -z "$(git diff --name-only main -- docs/adr openspec/changes/archive)"
echo 'no bridge version or history tree changed'
```

## Bridge version registry

### Task 1: Add the flat registry, Gradle defaults, and constant comparison

**Spec refs:** Requirement `Bridge versions have one flat registry and no Gradle
literal`; Requirement `Exact positive registry is compared against Rust
constants`.

**Files:**
- `bridges/versions.toml` (new)
- `bridges/cxf/build.gradle.kts` (modified)
- `bridges/jms/build.gradle.kts` (modified)
- `bridges/xml/build.gradle.kts` (modified)
- `scripts/xtask/src/bridge_versions/mod.rs` (new)
- `scripts/xtask/src/bridge_versions/registry.rs` (new)
- `scripts/xtask/src/lib.rs` (modified)
- `scripts/xtask/src/main.rs` (modified)

**Steps:**

1. Create `bridges/versions.toml` with exactly these three lines and no table
   headers:
   ```toml
   cxf = "0.8.4"
   jms = "0.8.4"
   xml = "0.8.1"
   ```
2. Create `scripts/xtask/src/bridge_versions/registry.rs` with
   `pub enum Bridge { Cxf, Jms, Xml }`, `impl Bridge { pub fn key(self) -> &'static str; pub fn parse(value: &str) -> Option<Bridge>; pub fn const_paths(self) -> &'static [&'static str]; }`,
   `pub struct Registry { pub cxf: String, pub jms: String, pub xml: String }`,
   and `pub fn parse_registry(path: &Path) -> Result<Registry, Vec<Finding>>`.
   `const_paths` returns
   `["crates/components/camel-cxf/src/lib.rs"]` for Cxf,
   `["crates/components/camel-jms/src/lib.rs"]` for Jms, and
   `["crates/components/camel-xslt/src/lib.rs", "crates/components/camel-validator/src/lib.rs"]`
   for Xml.
3. Implement `parse_registry` with 1-based line tracking. Trim each line, skip
   empty lines and lines starting with `#`. Reject a line starting with `[` as
   `table header`; reject `[`, `]`, `{`, or `}` anywhere as `non-flat value`;
   reject a missing `=`; reject a key outside `cxf`, `jms`, `xml` as
   `unknown key`; reject a value that lacks a leading or closing double quote
   as `unquoted value`; when a closing quote exists and any non-whitespace byte
   follows it, report `trailing content` (so `cxf = "0.8.4" # pin` reports
   `trailing content`, not `unquoted value`); then reject the quoted inner value
   when it does not match
   `^\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?$`; reject duplicate
   keys; and after the scan return `missing key` when any of the three keys is
   absent. Every finding carries the file path, line, offending token, and the
   shape name. A missing registry file returns one finding at line 0 with detail
   `missing registry file` and the path. Add `#[cfg(test)] mod tests` to
   `registry.rs` for the parser
   fixtures described below.
4. Create `scripts/xtask/src/bridge_versions/mod.rs` with
   `pub mod registry;`, `pub struct Finding { pub path: PathBuf, pub line: usize, pub token: String, pub detail: String }`,
   and
   `pub fn lint_registry_constants(root: &Path, registry: &Registry) -> Result<Vec<Finding>, String>`.
   For each registered file, read the file and match each trimmed line against
   `^pub const BRIDGE_VERSION: &str = "([^"]+)";$`, capturing the declared value
   independently of the registry. Do not match the registry value in the
   pattern. Zero matches produce a `missing BRIDGE_VERSION` finding; a read
   error or missing file produces the same missing finding with the path; more
   than one produce a `duplicate BRIDGE_VERSION` finding; one match compares
   the captured value with the registry entry. A mismatch produces a finding
   whose detail is
   `bridge version mismatch: registry key 'cxf' expected '0.8.4' actual '0.8.3'; replace with camel_cxf::BRIDGE_VERSION or the bridge release page`. Render the observed key, expected value, actual value, and path-specific remedy into that same sentence.
   Add `#[cfg(test)] mod tests` to `mod.rs` for the constant fixtures described
   below.
5. Add `pub mod bridge_versions;` to `scripts/xtask/src/lib.rs`.
6. Add `use xtask::bridge_versions;` to `scripts/xtask/src/main.rs`, add
   `LintBridgeVersions` to the command enum, and dispatch it: discover the
   workspace root with the existing `workspace_root_or_exit`,
   parse `bridges/versions.toml`, call `lint_registry_constants`, print one
   `path:line: detail [token]` line per finding, print
   `lint-bridge-versions: OK (no findings)` on success, and exit 1 on findings
   or a parse error.
7. In each `build.gradle.kts`, replace the existing
   `version = project.findProperty("version")?.toString() ?: "0.8.4"` line (in `bridges/xml/build.gradle.kts` the literal is `0.8.1`)
   with this block, using the file's key in `bridgeKey` (`cxf`, `jms`, or
   `xml`):
   ```kotlin
   version = providers.gradleProperty("version").orNull ?: run {
       val bridgeRegistry = rootDir.resolve("../versions.toml")
       check(bridgeRegistry.isFile) {
           "bridge version registry not found: ${bridgeRegistry.absolutePath}"
       }
       val bridgeKey = "cxf"
       Regex("""(?m)^\s*${Regex.escape(bridgeKey)}\s*=\s*"([^"]+)"\s*$""")
           .find(bridgeRegistry.readText())
           ?.groupValues
           ?.get(1)
           ?: error("bridge version key '$bridgeKey' not found in ${bridgeRegistry.absolutePath}")
   }
   ```
   The same block is used in all three files; only the `bridgeKey` literal
   differs.
8. Keep the registry check and regex only on the right-hand side of `?:`. With
   `-Pversion` set, Gradle must not check or read `../versions.toml`. This keeps
   `./gradlew -q properties -Pversion=9.9.9` working when Docker mounts only the
   bridge directory and no parent registry exists.
9. Confirm `bridges/jms/gradle.properties` is not required: the override is
   read through `providers.gradleProperty("version")`, so no new Gradle
   properties file is added.

**Tests:**

- `registry_parses_flat_three_keys` — setup: fixture `bridges/versions.toml`
  with the three required lines; action: call `parse_registry`; assert: returns
  `cxf=0.8.4`, `jms=0.8.4`, `xml=0.8.1`; command: `C1`; expected RED: compile
  error because `bridge_versions` does not exist; expected GREEN: passes.
- `registry_allows_blank_and_comment_lines` — setup: fixture with a leading
  blank line and `# bridge versions`; action: call `parse_registry`; assert:
  parses all three values; command: `C1`; expected RED: compile error; expected
  GREEN: passes.
- `registry_rejects_table_header` — setup: fixture adds `[bridges]`; action:
  call `parse_registry`; assert: one finding at line 4 with detail containing
  `table header` and the path; command: `C1`; expected RED: compile error;
  expected GREEN: passes.
- `registry_rejects_unknown_key` — setup: fixture adds `http = "0.8.4"`;
  action: call `parse_registry`; assert: finding names `http` and
  `unknown key`; command: `C1`; expected RED: compile error; expected GREEN:
  passes.
- `registry_rejects_duplicate_key` — setup: fixture repeats `xml`; action: call
  `parse_registry`; assert: finding names `xml`, the duplicate line, and
  `duplicate key`; command: `C1`; expected RED: compile error; expected GREEN:
  passes.
- `registry_rejects_unquoted_value` — setup: fixture uses `cxf = 0.8.4`;
  action: call `parse_registry`; assert: finding names `unquoted value`;
  command: `C1`; expected RED: compile error; expected GREEN: passes.
- `registry_rejects_trailing_content` — setup: fixture uses
  `cxf = "0.8.4" # pin`; action: call `parse_registry`; assert: finding names
  `trailing content`; command: `C1`; expected RED: compile error; expected
  GREEN: passes.
- `registry_rejects_missing_key` — setup: fixture omits `xml`; action: call
  `parse_registry`; assert: finding names `xml` and `missing key`; command:
  `C1`; expected RED: compile error; expected GREEN: passes.
- `constants_match_registry_in_fixture` — setup: fixture with the registry and
  four `lib.rs` files whose constants match; action: call
  `lint_registry_constants`; assert: empty findings; command: `C2`; expected
  RED: compile error; expected GREEN: passes.
- `constant_drift_reports_exact_evidence` — setup: fixture `camel-cxf/src/lib.rs`
  declares `0.8.3` while the registry says `0.8.4`; action: call
  `lint_registry_constants`; assert: one finding with the CXF path, its line,
  `cxf`, `0.8.4`, and `0.8.3` in the detail; command: `C2`; expected RED:
  compile error; expected GREEN: passes.
- `both_xml_constants_are_checked` — setup: XSLT declares `0.8.1` and Validator
  declares `0.8.0`; action: call `lint_registry_constants`; assert: one finding
  naming the Validator path and the `xml` key; command: `C2`; expected RED:
  compile error; expected GREEN: passes.
- `missing_constant_is_reported` — setup: fixture deletes the Validator
  `BRIDGE_VERSION` line; action: call `lint_registry_constants`; assert: finding
  names the Validator path and `missing BRIDGE_VERSION`; command: `C2`;
  expected RED: compile error; expected GREEN: passes.
- `duplicate_constant_is_reported` — setup: fixture repeats the CXF
  `BRIDGE_VERSION` line; action: call `lint_registry_constants`; assert: finding
  names the CXF path and `duplicate BRIDGE_VERSION`; command: `C2`; expected
  RED: compile error; expected GREEN: passes.
- `gradle_defaults_report_registry_versions` — setup: `versions.toml` present
  and no `-Pversion`; action: run `G1a`, `G1b`, and `G1c`; assert: `G1a` prints
  `version: 0.8.4`, `G1b` prints `version: 0.8.4`, and `G1c` prints
  `version: 0.8.1`; command: `G1a`, `G1b`, `G1c`; expected RED: `G3` fails
  while the literals remain; expected GREEN: all three print their registry
  values and exit 0.
- `gradle_property_override_wins` — setup: registry present; action: run
  `G2a`, `G2b`, and `G2c`; assert: each prints `version: 9.9.9`; command:
  `G2a`, `G2b`, `G2c`; expected RED: not applicable because the current
  override path already returns the property; expected GREEN: all three print
  `version: 9.9.9` after the rewrite.
- `gradle_override_without_parent_registry_succeeds` — setup: `G4a` copies
  `bridges/cxf` into an isolated temp directory with no parent
  `bridges/versions.toml`, and `G4b` and `G4c` do the same for JMS and XML;
  action: run `G4a`, `G4b`, and `G4c`; assert: each prints `version: 9.9.9`
  with no registry lookup; command: `G4a`, `G4b`, `G4c`; expected RED: before
  step 7 the eager `check` fails with `bridge version registry not found`;
  expected GREEN: all three print `version: 9.9.9`.
- `gradle_contains_no_literal_bridge_version` — setup: rewritten Gradle files;
  action: run `G3`; assert: prints `no literal bridge version in Gradle` and
  exits 0; command: `G3`; expected RED: before step 7 `G3` exits 1 and prints
  matches in all three files; expected GREEN: exits 0 with no matches.

**Acceptance:**
- `C1`, `C2`, `C9`, `G1a`, `G1b`, `G1c`, `G2a`, `G2b`, `G2c`, `G3`, `G4a`,
  `G4b`, and `G4c` exit 0.
- `git diff --stat` lists only the eight files in this task.

- [x] 1

### Task 2: Implement contextual live-set detection and tag mode

**Spec refs:** Requirement `No bridge version appears in the live doc set`;
Requirement `Tag mode fails with an actionable exact diagnostic`.

**Files:**
- `scripts/xtask/src/bridge_versions/scan.rs` (new)
- `scripts/xtask/src/bridge_versions/mod.rs` (modified)
- `scripts/xtask/src/main.rs` (modified)

**Steps:**

1. Create `scripts/xtask/src/bridge_versions/scan.rs` with
   `pub struct ScanReport { pub findings: Vec<Finding> }` and
   `pub fn scan_live_set(root: &Path) -> Result<ScanReport, String>`.
2. Enumerate the live set with `walkdir` and deduplicate paths in a
   `BTreeSet<PathBuf>`:
   - every file named `README.md` under `crates/`;
   - every `.md` file under `docs/src/`;
   - files under `examples/` whose extension is one of `md`, `toml`, `yaml`,
     `yml`, `json`, `rs`, `sh`, `properties`, `txt`, `conf`, or `env`;
   - every file named `README.md` under `bridges/`.
   Skip any path containing a `target` component, a `Cargo.lock` file, any path
   under `docs/adr`, `docs/archived`, `docs/reviews`, `openspec/changes/archive`,
   any file whose name starts with `CHANGELOG`, and any `Cargo.toml` whose path
   contains a component whose name contains `wasm` (for example `wasm` or
   `wasm-guest`). Skip a root directory that does not exist.
3. Define `TOKEN_RE` as the lookaround-free regex
   `\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?`. Reject a match when
   the preceding byte is an ASCII digit or `.`, the next byte is an ASCII digit,
   or the next byte is `.` and the byte after it is ASCII alphanumeric or `_`.
   A trailing sentence period is therefore allowed.
4. Define `SUBJECT_RE` as the case-insensitive regex
   `\b(?:cxf|jms|xml)-bridge(?:-v[0-9.]+)?\b|\bBRIDGE_VERSION\b|\bbridge_version\b|\bbridge\b`.
   When a match is the bare word `bridge`, ignore it if the six preceding bytes
   are `camel-` case-insensitively. Define `CONTEXT_RE` as the case-insensitive
   alternation of
   `version`, `since`, `breaking`, `needs`, `requires`, `supports`,
   `or newer`, `or later`, `minimum`, `pinned`, `release pin`, and `>=`.
5. Define `PRODUCT_RE` as the case-insensitive word alternation of `Quarkus`,
   `GraalVM`, `ActiveMQ`, `Artemis`, `MQTT`, `OpenAPI`, `cargo`, `rustc`, `JDK`,
   `Saxon`, `Xerces`, and `camel-bridge`. A product exception applies only when
   a product match ends no more than 32 bytes before the token start. A product
   after the token never applies.
6. Scan Markdown files as paragraphs of consecutive nonblank lines. Scan a line
   whose first non-space byte is `|` as one sentence. For `.rs` files, process
   every line: strip a leading `//!`, `///`, or `//` when present, group
   consecutive comment-only lines into one paragraph, and treat each
   non-comment line as its own sentence. Apply rule 2 to every `.rs` line,
   including executable assignments. Join paragraph lines with `\n` and record
   each line's starting byte offset so every token maps back to its original
   line and file.
7. Split a paragraph into sentence spans. A sentence ends at `.`, `!`, `?`, or
   `;` only when the next byte is whitespace, a newline, or the paragraph end.
   A dot inside a token never ends a sentence. Keep a table row as one span.
8. For each token, evaluate these rules in order:
   - rule 1: the token line contains `version = "0.8.4"` (with the observed token as the value) and the active TOML
     section is `[components.cxf]`, `[components.jms]`, or any
     `[components.xml` section heading, including `[components.xml]`,
     `[components.xml.xslt]`, and `[components.xml.validator]`;
   - rule 2: the token line contains `bridge_version` followed by `:` or `=`
     and the observed token is a literal on that line; a
     `bridge_version: camel_xslt::BRIDGE_VERSION.to_string()` expression has no
     literal token and is legal;
   - rule 3: the product exception does not apply and either a subject lies
     within 32 bytes on either side of the token, or the sentence contains both
     a subject and a context word;
   - rule 4: the file is a bridge-topic doc and the sentence contains a context
     word.
   Rules 1 and 2 always report; the product exception applies only to rules 3
   and 4.
9. Define bridge-topic docs as
   `crates/components/camel-{cxf,jms,xslt,validator,xj}/README.md`,
   `docs/src/components/{ws-soap,jms,xml-transform,validator}.md`,
   `docs/src/services/bridge.md`, and every `bridges/**/README.md`. Map the
   remedy by path: CXF paths name `camel_cxf::BRIDGE_VERSION`, JMS paths name
   `camel_jms::BRIDGE_VERSION`, XSLT/XJ paths name
   `camel_xslt::BRIDGE_VERSION`, Validator paths name
   `camel_validator::BRIDGE_VERSION`, and all other paths name
   `the bridge release page`. Every finding detail is
   `bridge version '0.8.4' must not appear in live bridge documentation; use camel_cxf::BRIDGE_VERSION`, rendering the observed token and path-specific remedy.
10. In `scripts/xtask/src/bridge_versions/mod.rs`, add `pub mod scan;`,
    `pub struct LintOutcome { pub findings: Vec<Finding>, pub tag_error: Option<String> }`,
    and
    `pub fn lint(root: &Path, tag: Option<&str>) -> Result<LintOutcome, String>`.
    Parse the tag syntax first. On an unknown prefix return
    `Ok(LintOutcome { findings: Vec::new(), tag_error: Some(message) })` without
    scanning. Otherwise parse the registry; when `parse_registry` returns
    findings, return them in `LintOutcome.findings` and skip the constant and
    live scans. On a valid registry, compare a known tag prefix with its
    registry value; a mismatch returns
    `Ok(LintOutcome { findings: Vec::new(), tag_error: Some(message) })`. A
    matching tag falls through to `lint_registry_constants` and
    `scan_live_set`, whose findings are concatenated in that order. Map an
    `Err(String)` from either helper to `Err(String)`.
11. Implement tag parsing in `lint`. Accept only tags with the prefixes
    `cxf-bridge-v`, `jms-bridge-v`, and `xml-bridge-v`. For any other prefix set
    `tag_error` to a message in the shape `unknown tag 'foo-bridge-v1.2.3': accepted forms are cxf-bridge-v*, jms-bridge-v*, xml-bridge-v*`, rendering the observed tag.
    When the bridge and version parse but differ from the registry entry, set
    `tag_error` to exactly:
    ```text
    tag xml-bridge-v0.8.2 != bridges/versions.toml xml=0.8.1 — run 'cargo xtask bump-bridge xml 0.8.2', commit, delete and re-push the tag
    ```
    Use the observed tag and registry key/value in the rendered string. A tag
    mismatch returns that single `tag_error` and does not print registry or scan
    findings. A matching tag still runs both checks and returns their findings.
12. In `scripts/xtask/src/main.rs`, add `tag: Option<String>` to
    `LintBridgeVersions`, call `bridge_versions::lint(&root, tag.as_deref())`,
    print `tag_error` to stderr and exit 1 when present, print each finding as
    `path:line: detail [token]`, print `lint-bridge-versions: OK (no findings)`
    otherwise, and exit 1 on findings or a parse error.
13. Add `#[cfg(test)] mod tests` to `scan.rs` and extend the existing
    `#[cfg(test)] mod tests` in `mod.rs` with the tag tests. Use a fixture
    helper that writes `(relative_path, content)` pairs into a `tempfile::TempDir`
    and calls `scan_live_set` or `lint` with that directory. The fixture helper
    creates both `bridges/` and `crates/` directories when absent.

**Tests:**

- `component_block_pin_is_rejected` — setup: `examples/cxf-example/Camel.toml`
  with `[components.cxf]` then `version = "0.8.4"`; action: scan; assert: one
  finding on the version line with token `0.8.4`; command: `C3`; expected RED:
  compile error; expected GREEN: passes.
- `history_and_minimum_prose_are_rejected` — setup:
  `crates/components/camel-jms/README.md` with `since bridge 0.8.0`,
  `Breaking in 0.8.0`, and `needs 0.8.4 or newer` on separate lines; action:
  scan; assert: three findings with tokens `0.8.0`, `0.8.0`, `0.8.4`; command:
  `C3`; expected RED: compile error; expected GREEN: passes.
- `sentence_final_version_and_ip_boundaries` — setup:
  `crates/components/camel-xj/README.md` with `Since bridge 0.8.0.` and
  `127.0.0.1:8080` and `0.0.0.0:9090`; action: scan; assert: exactly one
  finding with token `0.8.0`; command: `C3`; expected RED: compile error;
  expected GREEN: passes.
- `paragraph_join_reports_cross_line_version` — setup:
  `crates/components/camel-jms/README.md` with `The bridge` on line 1 and
  `requires 0.8.4.` on line 2; action: scan; assert: one finding at line 2 with
  token `0.8.4`; command: `C3`; expected RED: compile error; expected GREEN:
  passes.
- `sentence_punctuation_keeps_tokens_intact` — setup:
  `crates/components/camel-jms/README.md` with
  `Since bridge 0.8.0. A later bridge needs 0.8.4;`; action: scan; assert: two
  findings with tokens `0.8.0` and `0.8.4`; command: `C3`; expected RED:
  compile error; expected GREEN: passes.
- `later_sentence_bridge_subject_proximity` — setup: `crates/demo/README.md`
  with
  `Introduction lasting at least forty characters. The bridge 0.8.0 drops headers.`;
  action: scan; assert: one finding on line 1 with token `0.8.0`; command:
  `C3`; expected RED: the later sentence's subject is measured against the
  paragraph offset and reports zero findings; expected GREEN: passes.
- `product_exception_is_position_bound` — setup: bridge-topic
  `docs/src/components/jms.md` with
  `| ActiveMQ Classic 5.18.3 | bridge cap |`,
  `| ActiveMQ Artemis 2.36.0 | bridge cap |`, and
  `the bridge 0.8.0 acks ActiveMQ deliveries`; action: scan; assert: exactly
  one finding with token `0.8.0`; command: `C3`; expected RED: compile error;
  expected GREEN: passes.
- `later_sentence_product_exception` — setup: bridge-topic
  `bridges/cxf/README.md` with
  `Introduction lasting at least forty characters. ActiveMQ version 5.18.3.`;
  action: scan; assert: zero findings; command: `C3`; expected RED: the later
  sentence's product exception is measured against the paragraph offset, so the
  exception misses and the token is reported; expected GREEN: passes.
- `product_never_overrides_component_block_or_bridge_version` — setup:
  `examples/cxf-example/Camel.toml` with `[components.cxf]` and
  `# Saxon version = "0.8.4"`, plus `crates/components/camel-xj/README.md` with
  `// Saxon bridge_version: "1.0.0"`; action: scan; assert: two findings with
  tokens `0.8.4` and `1.0.0`; command: `C3`; expected RED: compile error;
  expected GREEN: passes.
- `explicit_bridge_binary_and_tag_terms` — setup:
  `bridges/probe/README.md` (a bridge-topic doc) with `cxf-bridge-v0.8.4` and
  `jms-bridge 0.8.0`; action: scan; assert: two findings with tokens `0.8.4`
  and `0.8.0`; command: `C3`; expected RED: compile error; expected GREEN:
  passes.
- `bridge_subjects_are_case_insensitive_whole_words` — setup:
  `crates/components/camel-jms/README.md` with `Since bridge 0.8.0`,
  `**Breaking in 0.8.0**`, and `camel-bridge v0.59.0`; action: scan; assert:
  two findings, and no finding contains `0.59.0`; command: `C3`; expected RED:
  compile error; expected GREEN: passes.
- `bridge_version_field_is_rejected` — setup:
  `crates/components/camel-xj/README.md` with
  `bridge_version: "1.0.0"`; action: scan; assert: one finding on that line with
  token `1.0.0` and a `camel_xslt::BRIDGE_VERSION` remedy; command: `C3`;
  expected RED: compile error; expected GREEN: passes.
- `rust_executable_bridge_version_literal_is_reported` — setup:
  `examples/probe/src/lib.rs` with
  `let cfg = XjComponentConfig { bridge_version: "1.0.0".to_string(), bridge_binary_path: None };`;
  action: scan; assert: one finding on that executable line with token `1.0.0`;
  command: `C3`; expected RED: compile error; expected GREEN: passes.
- `rust_bridge_version_constant_expression_is_legal` — setup:
  `examples/probe/src/lib.rs` with
  `let cfg = XjComponentConfig { bridge_version: camel_xslt::BRIDGE_VERSION.to_string(), bridge_binary_path: None };`;
  action: scan; assert: zero findings because the assignment has no literal
  token; command: `C3`; expected RED: compile error; expected GREEN: passes.
- `bridge_readme_numeric_versions_are_rejected` — setup:
  `bridges/cxf/README.md` with `pinned 0.8.1` and
  `release pin is 0.8.1`; action: scan; assert: two findings with token
  `0.8.1`; command: `C3`; expected RED: compile error; expected GREEN: passes.
- `rust_comment_bridge_versions_are_reported` — setup:
  `examples/jms-container-example/src/main.rs` with
  `//! Since bridge 0.8.0` on line 1 and `// it bridge 0.8.0 drops headers` on
  line 3, separated by a blank line; action: scan; assert: two findings at lines
  1 and 3 with token `0.8.0`; command: `C3`; expected RED: compile error;
  expected GREEN: passes.
- `rust_comment_paragraph_joins_lines` — setup:
  `examples/jms-container-example/src/main.rs` with `//! The JMS bridge` on line
  1 and `//! requires 0.8.4.` on line 2; action: scan; assert: one finding at
  line 2 with token `0.8.4`; command: `C3`; expected RED: compile error;
  expected GREEN: passes.
- `bridge_topic_context_word_reports_without_subject` — setup:
  `crates/components/camel-jms/README.md` with `**Breaking in 0.8.0:**`;
  action: scan; assert: one finding with token `0.8.0`; command: `C3`;
  expected RED: compile error; expected GREEN: passes.
- `unrelated_versions_and_ips_stay_legal` — setup: `crates/foo/README.md` with
  `since 0.16.0`; `examples/foo/Cargo.toml` with `version = "0.1.0"`;
  `examples/wasm-guest/Cargo.toml` with `version = "0.8.1"`;
  `examples/foo/README.md` with `minijinja/2.21.0`, `MQTT 3.1.1`, OpenAPI
  `3.0.3`, and `127.0.0.1:8080`; action: scan; assert: zero findings; command:
  `C3`; expected RED: compile error; expected GREEN: passes.
- `history_directories_stay_exempt` — setup: `docs/adr/0001.md`,
  `docs/archived/old.md`, `docs/reviews/old.md`,
  `openspec/changes/archive/old/spec.md`, and `CHANGELOG.md` each containing
  `bridge 0.8.0`, plus `examples/probe/README.md` containing
  `bridge 0.8.0`; action: scan; assert: exactly one finding and its path ends
  with `examples/probe/README.md`; command: `C3`; expected RED: compile error;
  expected GREEN: passes.
- `matching_tag_passes` — setup: fixture registry with `cxf = "0.8.4"`, four
  matching constants, and no live findings; action: call
  `lint(&root, Some("cxf-bridge-v0.8.4"))`; assert:
  `tag_error` is `None` and findings are empty; command: `C4`; expected RED:
  compile error; expected GREEN: passes.
- `mismatched_tag_prints_exact_diagnostic` — setup: fixture registry with
  `xml = "0.8.1"` and matching constants; action: call
  `lint(&root, Some("xml-bridge-v0.8.2"))`; assert: `tag_error` equals the
  exact string
  `tag xml-bridge-v0.8.2 != bridges/versions.toml xml=0.8.1 — run 'cargo xtask bump-bridge xml 0.8.2', commit, delete and re-push the tag`;
  command: `C4`; expected RED: compile error; expected GREEN: passes.
- `unknown_tag_prefix_lists_accepted_forms` — setup: fixture registry; action:
  call `lint(&root, Some("foo-bridge-v1.2.3"))`; assert: `tag_error` contains
  `cxf-bridge-v*`, `jms-bridge-v*`, and `xml-bridge-v*`; command: `C4`;
  expected RED: compile error; expected GREEN: passes.
- `matching_tag_does_not_bypass_constant_drift` — setup: fixture registry with
  `cxf = "0.8.4"` and CXF constant `0.8.3`; action: call
  `lint(&root, Some("cxf-bridge-v0.8.4"))`; assert: one constant finding and no
  `tag_error`; command: `C4`; expected RED: compile error; expected GREEN:
  passes.
- `matching_tag_does_not_bypass_live_finding` — setup: fixture registry with
  `cxf = "0.8.4"`, matching constants, and
  `examples/cxf-example/Camel.toml` containing a component-block pin; action:
  call `lint(&root, Some("cxf-bridge-v0.8.4"))`; assert: one live finding and no
  `tag_error`; command: `C4`; expected RED: compile error; expected GREEN:
  passes.

**Acceptance:**
- `C3`, `C4`, and `C9` exit 0.
- No repository live file is rewritten in this task.

- [x] 2

### Task 3: Rewrite live prose and delete the JMS-only drift test

**Spec refs:** Requirement `No bridge version appears in the live doc set`
(rewrite half); Requirement `Live bridge prose states capabilities in the
present tense`.

**Files:**
- `examples/cxf-example/Camel.toml` (modified)
- `crates/components/camel-cxf/README.md` (modified)
- `docs/src/components/ws-soap.md` (modified)
- `bridges/cxf/README.md` (modified)
- `crates/components/camel-jms/README.md` (modified)
- `docs/src/components/rabbitmq.md` (modified)
- `examples/jms-container-example/src/main.rs` (modified)
- `crates/components/camel-xj/README.md` (modified)
- `crates/components/camel-jms/src/lib.rs` (modified)

**Steps:**

1. In `examples/cxf-example/Camel.toml`, delete only the line
   `version = "0.8.4"` from the `[components.cxf]` block. Keep `bind_address`
   and the profile entries unchanged.
2. In `crates/components/camel-cxf/README.md`, delete only the line
   `version = "0.8.4"` from the `## Camel.toml Configuration` code block. Keep
   the profile entries and comments unchanged.
3. In `docs/src/components/ws-soap.md`, delete only the line
   `version = "0.8.4"` from the CXF `Camel.toml` block. Replace the single
   paragraph that starts with `CXF and XML bridge release pins are` with:
   ```text
   Per-profile consumer dispatch, `allow_expired_pinned_certificate`, and one-way operations are current capabilities of the CXF bridge. For the current bridge version, use `camel_cxf::BRIDGE_VERSION` or the bridge release page. One-way acceptance and the mission-361 regressions run against a bridge rebuilt from source: set `CAMEL_CXF_BRIDGE_BINARY_PATH` to the rebuilt `cxf-bridge` binary to use it. Publication of bridge tags and public release assets is an owner-only action (the `*-bridge-release` workflows build and publish on tag push).
   ```
4. In `bridges/cxf/README.md`, replace the `## Native acceptance` paragraph
   and its code block, from `One-way behavior is accepted against the pinned`
   through the exact source text in this block:
   ```text
   The override skips the download. The release pin is `0.8.1`.
   ```
   Replace that paragraph and its code block with:
   ````text
   One-way behavior is accepted against a bridge built from source, not a
   downloaded binary. Build it with `cargo xtask build-cxf-bridge` without
   `--version`; the result is labelled `dev` by `build-native.sh`. Then point the
   Rust component at the rebuilt binary:

   ```bash
   export CAMEL_CXF_BRIDGE_BINARY_PATH=/path/to/cxf-bridge
   ```

   The override skips the download. For the current released version, use
   `camel_cxf::BRIDGE_VERSION` or the bridge release page.
   ````
   Do not claim that the build reads `versions.toml`; the build behavior is
   unchanged.
5. In `crates/components/camel-jms/README.md`, replace the
   `jmsHeaderAllowlist` table cell text
   `**Breaking in 0.8.0:** with the empty default no user header is forwarded.`
   with `With the empty default no user header is forwarded.` Then replace the
   `- Queue messages are acknowledged after the route completes (bridge 0.8.0).`
   bullet with:
   ```text
   - Queue messages are acknowledged after the route completes. An `Ok` result acknowledges; an unhandled `Err` rolls back and the broker redelivers. After 6 delivery attempts (`JMS_MAX_DELIVERY_ATTEMPTS`) the bridge acknowledges and discards the message, logging only its ID — no dead-letter forwarding. A broker-native redelivery limit that fires first takes precedence. Attempt counts persist under `$XDG_STATE_HOME/rust-camel/jms-bridge` (else `$HOME/.local/state/rust-camel/jms-bridge`); see `docs/src/components/jms.md` in the repository. The broker acknowledgement is issued synchronously (ActiveMQ Classic `sendAcksAsync(false)`, Artemis `blockOnAcknowledge(true)`), so a crash immediately after the broker commits cannot redeliver a message whose attempt budget was already cleared.
   ```
6. In `docs/src/components/rabbitmq.md`, replace
   `The Consumer acknowledges a delivery only after the route completes. The JMS component does the same since bridge 0.8.0.`
   with
   `The Consumer acknowledges a delivery only after the route completes. The JMS component does the same.`
7. In `examples/jms-container-example/src/main.rs`, replace
   `Since bridge 0.8.0 the producer forwards no user header unless the` with
   `The producer forwards no user header unless the`; replace
   `//! ## Acknowledgement (bridge 0.8.0)` with
   `//! ## Acknowledgement`; and replace
   `it bridge 0.8.0 drops every user header` with
   `the bridge drops every user header`.
8. In `crates/components/camel-xj/README.md`, replace
   `bridge_version: "1.0.0".to_string(),` with
   `bridge_version: camel_xslt::BRIDGE_VERSION.to_string(),`.
9. In `crates/components/camel-jms/src/lib.rs`, delete the entire
   module that starts at `#[cfg(test)] mod bridge_version_test {` and ends at its
   closing `}` immediately before `pub mod proto`, including
   `bridge_version_pin_matches_gradle_default` and `locate_build_gradle`. Keep
   `BRIDGE_ENV_LOCK` and `mod bridge_client_test;` unchanged.
10. Run `C10` and `C11` to prove the repository live set is clean and a matching
    tag passes. If `C10` reports a live finding in a file not listed above,
    apply the same present-tense rewrite there and add that path to the Files
    list before continuing. Run `C12` and capture the exact mismatch diagnostic.

**Tests:**

- `no_live_bridge_pin_in_rewritten_files` — setup: steps 1 through 8 applied;
  action: run `R1`; assert: prints
  `rewritten files contain no bridge pin`; command: `R1`; expected RED: before
  the rewrites `R1` exits 1 and prints the pins in the eight files; expected
  GREEN: exits 0.
- `ws_soap_capability_rewrite_keeps_guidance` — setup: step 3 applied; action:
  run `R3`; assert: prints `ws-soap guidance retained`; command: `R3`;
  expected RED: before step 3 `R3` exits 1 naming the first missing term;
  expected GREEN: exits 0.
- `cxf_bridge_readme_states_dev_label_and_source_build` — setup: step 4
  applied; action: run `R4`; assert: `R4` prints `capability prose retained`
  and `R1` finds no pin; command: `R4`; expected RED: before step 4 `R4` exits
  1 because the `labelled` phrase is absent; expected GREEN: exits 0.
- `jms_prose_survives_without_version` — setup: steps 5 and 6 applied; action:
  run `R4`; assert: `redeliver`, `sendAcksAsync`, and `blockOnAcknowledge` are
  present and `R1` reports no pin; command: `R4`; expected RED: before steps 5
  and 6 `R4` exits 1; expected GREEN: exits 0.
- `xj_readme_uses_the_constant` — setup: step 8 applied; action: run `R4`;
  assert: `R4` prints `capability prose retained`, the XJ constant expression is
  present, and `R1` reports no pin; command: `R4`; expected RED: before step 8
  the file contains `"1.0.0"` and `R4` exits 1; expected GREEN: exits 0.
- `jms_only_drift_test_is_deleted` — setup: step 9 applied; action: run `R2`;
  assert: prints `JMS drift test deleted`; command: `R2`; expected RED: before
  step 9 `R2` exits 1 and prints the module and helper; expected GREEN: exits 0.
- `repository_lint_exits_zero` — setup: steps 1 through 9 applied; action: run
  `C10`; assert: exit code 0 and stdout contains
  `lint-bridge-versions: OK (no findings)`; command: `C10`; expected RED:
  before this task `C10` exits 1 with findings in the rewritten files; expected
  GREEN: exits 0.
- `matching_release_tag_passes_on_repository` — setup: clean live set; action:
  run `C11`; assert: exit code 0; command: `C11`; expected RED: before this
  task `C11` exits 1 on live findings; expected GREEN: exits 0.
- `mismatched_release_tag_fails_with_exact_message` — setup: clean live set;
  action: run `C12`; assert: exit code is non-zero and stderr contains exactly
  `tag xml-bridge-v0.8.2 != bridges/versions.toml xml=0.8.1 — run 'cargo xtask bump-bridge xml 0.8.2', commit, delete and re-push the tag`;
  command: `C12`; expected RED: not applicable because Task 2 returns the tag
  diagnostic before scanning; this is a regression guard; expected GREEN:
  non-zero with the exact line after the rewrites.
- `docs_guide_still_builds` — setup: docs edits applied; action: run `M1`;
  assert: exit code 0; command: `M1`; expected RED: before the rewrites `M1`
  exits 0 on the old prose, so this is a regression guard; expected GREEN:
  remains 0.
- `no_gradle_literal_after_rewrite` — setup: Task 1 Gradle edits present;
  action: run `G3`; assert: `no literal bridge version in Gradle`; command:
  `G3`; expected RED: before Task 1 `G3` exits 1; expected GREEN: exits 0.

**Acceptance:**
- `R1`, `R2`, `R3`, `R4`, `C10`, `C11`, `M1`, and `G3` exit 0.
- `C12` exits non-zero and prints the exact diagnostic in the task block.
- `git diff --stat` lists only the files in this task's Files list.

- [x] 3

### Task 4: Add atomic bump, release guards, and gate registration

**Spec refs:** Requirement `bump-bridge is atomic and lint-verified`;
Requirement `Release workflows guard tags and gates register the lint`;
Requirement `No bridge behavior or version changes`.

**Files:**
- `scripts/xtask/src/bridge_versions/bump.rs` (new)
- `scripts/xtask/src/bridge_versions/mod.rs` (modified)
- `scripts/xtask/src/main.rs` (modified)
- `scripts/xtask/Cargo.toml` (modified)
- `scripts/xtask/tests/bridge_release_workflows.rs` (new)
- `.github/workflows/cxf-bridge-release.yml` (modified)
- `.github/workflows/jms-bridge-release.yml` (modified)
- `.github/workflows/xml-bridge-release.yml` (modified)
- `.github/workflows/ci.yml` (modified)
- `AGENTS.md` (modified)

**Steps:**

1. Add `noyalib.workspace = true` to the `[dev-dependencies]` section of
   `scripts/xtask/Cargo.toml`; it is test-only and must not enter the xtask
   production dependency graph.
2. Create `scripts/xtask/src/bridge_versions/bump.rs` with
   `pub enum BumpError { InvalidBridge(String), InvalidVersion(String), Snapshot(String), Write { detail: String, rolled_back: bool }, PostWrite { detail: String, rolled_back: bool }, Rollback { original: String, rollback: String } }`,
   `pub struct BumpReport { pub changed: Vec<PathBuf> }`,
   `pub(crate) enum BumpEvent { BeforeWrite(PathBuf), AfterPersist(PathBuf), AfterReadback(PathBuf), BeforePostLint }`,
   `pub(crate) fn bump_bridge_with_hook<F>(root: &Path, bridge_name: &str, version: &str, hook: &mut F) -> Result<BumpReport, BumpError> where F: FnMut(&BumpEvent) -> Result<(), String>`,
   and
   `pub fn bump_bridge(root: &Path, bridge_name: &str, version: &str) -> Result<BumpReport, BumpError>`.
   `bump_bridge` calls `bump_bridge_with_hook` with `&mut |_| Ok(())`. The hook
   is invocation-local; there is no global fault flag or shared mutable state.
3. In `bump_bridge_with_hook`, validate the bridge with `Bridge::parse` and the
   version with the same semver regex as the registry. Invalid input returns
   before any file is read or written. A registry read or target snapshot read
   failure returns `BumpError::Snapshot` before any write. Before writing,
   compare every target constant for that bridge with the registry old value;
   if any declaration differs from the registry, return `BumpError::Snapshot`
   before any write.
4. Read the registry, resolve the target list as
   `bridges/versions.toml` plus `Bridge::const_paths`, and snapshot every target
   with `fs::read` before writing. For the registry, replace only the line
   `^cxf = "0.8.4"$` with `^cxf = "9.9.9"$` (using the selected key and the old and new versions). For each constant, replace only
   the single `pub const BRIDGE_VERSION: &str = "0.8.4";` declaration with the new version. Write
   each changed file through `tempfile::NamedTempFile` in the target directory
   followed by `persist`, then read back the written bytes. Call the hook with
   `BeforeWrite` before each write, `AfterPersist` after each successful
   persist, and `AfterReadback` after each successful readback. Any write,
   persist, readback, or hook error at those points restores every snapshot
   byte-for-byte and returns
   `BumpError::Write { detail, rolled_back: true }`; a restore failure returns
   `BumpError::Rollback { original, rollback }` naming both failures.
5. After all files, call the hook with `BeforePostLint`, then call
   `crate::bridge_versions::lint(root, None)`. A hook error, lint error, or any
   finding restores every snapshot byte-for-byte and returns
   `BumpError::PostWrite { detail, rolled_back: true }`; a restore failure
   returns `BumpError::Rollback { original, rollback }` naming both failures.
   On success return `BumpReport { changed }` with exactly the files whose bytes
   changed.
6. Do not add a global `AtomicBool` or any other process-wide fault switch. The
   tests inject failures by passing a closure that returns `Err` at the required
   `BumpEvent`, so parallel tests cannot interfere.
7. Add `pub mod bump;` to `scripts/xtask/src/bridge_versions/mod.rs`.
8. In `scripts/xtask/src/main.rs`, add
   `BumpBridge { bridge: String, version: String }`, call
   `bridge_versions::bump::bump_bridge`, print
   `bumped cxf to 9.9.9:` (using the observed bridge and version) followed by one changed path per line on
   success, print the `BumpError` detail on failure, and exit 0 or 1.
9. In each of `.github/workflows/cxf-bridge-release.yml`,
   `.github/workflows/jms-bridge-release.yml`, and
   `.github/workflows/xml-bridge-release.yml`, insert this exact `guard` job
   before the existing `build` job:
   ```yaml
     guard:
       runs-on: ubuntu-latest
       steps:
         - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v4
         - uses: dtolnay/rust-toolchain@4cda84d5c5c54efe2404f9d843567869ab1699d4 # stable
         - name: Check bridge tag against registry
           run: cargo xtask lint-bridge-versions --tag "$GITHUB_REF_NAME"
   ```
   Add `needs: guard` as the first key under each `build:` job, above
   `strategy:`. Leave the existing `release` job and its `needs: build`
   unchanged.
10. In `.github/workflows/ci.yml`, add this exact pair after the
    `lint-component-deps` step:
    ```yaml
      - name: lint-bridge-versions
        run: cargo xtask lint-bridge-versions
    ```
    The `- name` line begins with six spaces and the `run` line with eight,
    matching the `steps:` block in `ci.yml`.
11. In `AGENTS.md`, add this exact pair to the `QUALITY GATES` list after the
    `lint-component-deps` entry:
    ```text
    - name: lint-bridge-versions
      run: cargo xtask lint-bridge-versions
    ```
12. Create `scripts/xtask/tests/bridge_release_workflows.rs` with
    `use noyalib::compat::serde_yaml;`. Resolve the repository root as
    `Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")` and join every
    workflow, `ci.yml`, and `AGENTS.md` path to that root. Parse each of the
    three release workflow files with
    `serde_yaml::from_str::<serde_yaml::Value>`. Assert
    `jobs.guard` exists, its `steps` contain an `actions/checkout@` entry, a
    `dtolnay/rust-toolchain@` entry, and a `run` value containing exactly
    `cargo xtask lint-bridge-versions --tag "$GITHUB_REF_NAME"`. Assert
    `jobs.build.needs` is the string `guard`. A second test reads
    `.github/workflows/ci.yml` and `AGENTS.md` and asserts both contain
    `cargo xtask lint-bridge-versions`.
13. The success fixture tests call `bump_bridge`; the failure fixture tests
    call `bump_bridge_with_hook` with an invocation-local closure. The CLI
    dispatches `bump_bridge`, so no mock is involved. Do not run
    `cargo xtask bump-bridge` against this repository. The bump tests create a
    fixture with `bridges/versions.toml` and four constant files in a
    `tempfile::TempDir`, then run only against that fixture.

**Tests:**

- `bump_xml_updates_registry_and_both_constants` — setup: fixture registry
  `cxf=0.8.4`, `jms=0.8.4`, `xml=0.8.1` and four matching constants; action:
  call `bump_bridge(&root, "xml", "0.8.2")`; assert: registry `xml` is `0.8.2`,
  XSLT and Validator constants are `0.8.2`, CXF and JMS registry values and
  constants are unchanged, `lint(&root, None)` has no findings, and
  `report.changed` contains exactly `bridges/versions.toml`,
  `crates/components/camel-xslt/src/lib.rs`, and
  `crates/components/camel-validator/src/lib.rs`; command: `C5`; expected RED:
  compile error; expected GREEN: passes.
- `bump_cxf_changes_only_cxf` — setup: same fixture; action: call
  `bump_bridge(&root, "cxf", "9.9.9")`; assert: only the CXF registry line and
  CXF constant change; JMS and XML bytes are identical to their snapshots;
  command: `C5`; expected RED: compile error; expected GREEN: passes.
- `bump_constant_mismatch_fails_before_writes` — setup: fixture registry says
  `cxf = "0.8.4"` while the CXF constant declares `0.8.3`, with byte snapshots
  for every target; action: call `bump_bridge(&root, "cxf", "9.9.9")`; assert:
  result is `Err(BumpError::Snapshot)` and every target byte is unchanged;
  command: `C5`; expected RED: compile error; expected GREEN: passes.
- `bump_rolls_back_every_byte_after_postlint_failure` — setup: same fixture,
  snapshot every target byte; action: call
  `bump_bridge_with_hook(&root, "cxf", "9.9.9", &mut |event| match event { BumpEvent::BeforePostLint => Err("injected post-lint failure".to_string()), _ => Ok(()) })`;
  assert: result is
  `Err(BumpError::PostWrite { rolled_back: true, .. })` and every target file is
  byte-identical to its snapshot; command: `C5`; expected RED: compile error;
  expected GREEN: passes.
- `bump_rolls_back_after_first_persist_failure` — setup: same fixture and byte
  snapshots; action: call `bump_bridge_with_hook` with a local `seen_first`
  boolean that returns `Err("injected persist failure".to_string())` on the
  first `BumpEvent::AfterPersist` and `Ok(())` for every other event; assert:
  result is `Err(BumpError::Write { rolled_back: true, .. })` and every target
  file is byte-identical to its snapshot; command: `C5`; expected RED: compile
  error; expected GREEN: passes.
- `bump_rolls_back_after_readback_failure` — setup: same fixture and byte
  snapshots; action: call `bump_bridge_with_hook` with a local `seen_first`
  boolean that returns `Err("injected readback failure".to_string())` on the
  first `BumpEvent::AfterReadback` and `Ok(())` for every other event; assert:
  result is `Err(BumpError::Write { rolled_back: true, .. })` and every target
  file is byte-identical to its snapshot; command: `C5`; expected RED: compile
  error; expected GREEN: passes.
- `bump_invalid_bridge_writes_nothing` — setup: same fixture and byte
  snapshots; action: call `bump_bridge(&root, "http", "0.8.2")`; assert:
  `Err(BumpError::InvalidBridge)` and every target byte is unchanged; command:
  `C5`; expected RED: compile error; expected GREEN: passes.
- `bump_invalid_version_writes_nothing` — setup: same fixture and byte
  snapshots; action: call `bump_bridge(&root, "xml", "not-a-version")`; assert:
  `Err(BumpError::InvalidVersion)` and every target byte is unchanged; command:
  `C5`; expected RED: compile error; expected GREEN: passes.
- `workflows_parse_and_build_needs_guard` — setup: the three workflow edits
  applied; action: run `C6`; assert: each file parses, `guard` exists, `build`
  has `needs: guard`, and the guard step runs the exact tag command; command:
  `C6`; expected RED: before step 9 `C6` fails because `jobs.guard` is absent;
  expected GREEN: passes.
- `gate_registration_present` — setup: steps 10 and 11 applied; action: run
  `C6`; assert: both `ci.yml` and `AGENTS.md` contain
  `cargo xtask lint-bridge-versions`; command: `C6`; expected RED: before steps
  10 and 11 the assertion fails; expected GREEN: passes.

**Acceptance:**
- `C5`, `C6`, `C7`, `C8`, `C9`, `C10`, `C11`, `C12`, `C13`, `G1a`, `G1b`,
  `G1c`, `G2a`, `G2b`, `G2c`, `G3`, `G4a`, `G4b`, `G4c`, `M1`, and `V1` exit
  with the expected status (`C12` non-zero, all others zero).
- `V1` prints `no bridge version or history tree changed`.
- `git status --short` lists only the ten files in this task plus the files
  already completed by Tasks 1 through 3.
- No file under `docs/adr/**` or `openspec/changes/archive/**` is modified.

- [x] 4

## Regression specs (holistic review)

These blocks correct the executable recipes above. They change no design,
acceptance criterion, or public interface.

### R1: TOML assignment syntax for component pins

The Task 2 recipe `component_block_pin_is_rejected` covers only the exact
spaced double-quoted form. `component_pin` must match a TOML assignment
independent of whitespace and accept double or single quotes, while still
requiring a supported component section and a quoted semantic-version value.
Do not replace the exact-substring check with another exact substring.

- `component_pins_accept_toml_spacing_and_quotes` — setup: fixture files
  `examples/cxf-example/Camel.toml` with `[components.cxf]` then
  `version="0.8.4"`; `examples/cxf-tabs-example/Camel.toml` with
  `[components.cxf]` then `version\t=\t"0.8.4"`;
  `examples/cxf-single-example/Camel.toml` with `[components.cxf]` then
  `version = '0.8.4'`; `examples/xml-example/Camel.toml` with
  `[components.xml]` then `version="0.8.4"`;
  `examples/xml-validator-example/Camel.toml` with
  `[components.xml_validator]` then `version = '0.8.4'`; and
  `examples/unrelated-example/Camel.toml` with `[components.unrelated]` then
  `version = "0.8.4"`; action: scan the fixture; assert: exactly five
  findings, each on line 2 with token `0.8.4`, one for each supported section,
  and no finding for the unrelated section; command: `C3`; expected RED: zero
  findings because the exact `version = "0.8.4"` substring is absent from the
  compact, tabbed, and single-quoted forms; expected GREEN: passes.

### R2: Rollback attempts every snapshot

Task 4 step 4 says a restore failure returns
`BumpError::Rollback { original, rollback }`. It must also attempt every
remaining snapshot before returning. `original` keeps the original failure;
`rollback` aggregates every restore failure, including the failing path.

- `rollback_continues_after_restore_failure` — setup: `xml` fixture with byte
  snapshots; action: call `bump_bridge_with_hook(&root, "xml", "0.8.2", ...)`;
  the hook records `AfterPersist` targets, asserts that the registry, XSLT,
  and Validator targets were persisted, then on `BeforePostLint` replaces the
  registry path with a directory and returns
  `Err("injected pre-lint failure".to_string())`; assert: result is
  `Err(BumpError::Rollback { original, rollback })`, `original` contains
  `hook before post-lint: injected pre-lint failure`, `rollback` contains
  `restore` and the registry path, and both XML constant files are
  byte-identical to their original snapshots; command: `C5`; expected RED: the
  first registry restore error returns early and leaves the XML constants
  updated; expected GREEN: passes.

### R3: Real post-lint findings roll back

The Task 4 recipe `bump_rolls_back_every_byte_after_postlint_failure` injects
a hook error before lint, so it does not prove finding-driven rollback. Keep
that recipe as the hook-error guard and add a live fixture violation that the
real in-process lint reports.

- `bump_rolls_back_after_real_postlint_findings` — setup: `xml` fixture plus
  `examples/xml-demo/README.md` containing
  `The bridge version 9.9.9 is pinned.`; action: call
  `bump_bridge_with_hook(&root, "xml", "0.8.2", ...)` where the hook records
  events and returns `Ok(())` for every event, including `BeforePostLint`;
  assert: `BeforePostLint` ran, result is
  `Err(BumpError::PostWrite { rolled_back: true, .. })`, the error contains
  `lint findings after write` and `bridge version '9.9.9'`, every target is
  byte-identical to its snapshot, and the live fixture is unchanged; command:
  `C5`; expected RED: not applicable — this is a regression guard for the real
  lint path; expected GREEN: passes.
