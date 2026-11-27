# Tasks: dep-hygiene-2026-11

## Cargo.lock

### Task 1.1: Apply the six in-scope dependency bumps and prove the lock graph

**Files:**
- `Cargo.lock` (modified)
- `/home/shared/tmp/dep-hygiene-2026-11/*` (measurement logs, not committed)

**Steps:**
1. Confirm the worktree is clean except the change dir. Baseline logs are in
   `/home/shared/tmp/dep-hygiene-2026-11/` (`outdated-before.txt`,
   `tree-dup-before.txt`, `audit-before.log`, `Cargo.lock.base`).
2. Run, in this order, in the worktree with
   `CARGO_TARGET_DIR=$PWD/target TMPDIR=/home/shared/tmp`:
   ```
   cargo update -p bytes        --precise 1.12.2
   cargo update -p sha2@0.11.1  --precise 0.11.2
   cargo update -p trybuild     --precise 1.0.125
   cargo update -p ureq         --precise 3.5.2
   cargo update -p walkdir      --precise 2.5.1
   cargo update -p hyper        --precise 1.13.0
   ```
   `filetime` stays 0.2.29 exactly as on `main`: 0.2.30 deprecates
   `set_file_mtime` on rustc 1.99 and would force a test-only source edit,
   which the lock-only scope forbids (prerequisite migration: bd rc-5jzr7).
   If `--precise` reports that a target is not a semver-compatible update,
   STOP and report which crate — do not edit a manifest.
3. Save the full lock evidence with
   `git diff 1de271e1d -- Cargo.lock > lock-diff.txt` (the base commit, so the
   diff includes committed changes). Enumerate every package whose version
   changed. `ureq-proto 0.6.5 → 0.7.1` is a REPLACEMENT dragged by ureq, not
   an addition; the six targets plus it must be the complete version-change
   set, and `filetime` must be absent from it. Cargo re-disambiguates duplicate
   edges when it rewrites the lock: the ureq update flipped
   `serde-saphyr → base64 0.22.1` and `tempfile → getrandom 0.3.4`; both
   requirements are open ranges, so the baseline edges were restored by hand
   (`base64 0.23.1`, `getrandom 0.4.3`) and `cargo metadata --locked` exits 0.
   Restore them again if cargo flips them.
4. Measure duplicates with this exact command (distinct `(name, version)`
   pairs at column zero, names with more than one version):
   ```
   cargo tree -d --workspace --locked \
     | sed -nE 's/^([A-Za-z0-9_-]+) v([^ (]+).*/\1 \2/p' \
     | sort -u | awk '{print $1}' | uniq -d | wc -l   # baseline: 59
   cargo tree -d --workspace --locked | wc -l          # baseline: 5539 lines
   ```
   Compare the duplicate-name SETS before/after (not only the count): they
   must be identical, with only `sha2`'s version changing (0.11.1 → 0.11.2).
   Also verify from `Cargo.lock` that no package name gains a parallel
   version. `parking_lot` remains 0.12.5 + 0.13.0 (dedupe dropped by
   dispatcher decision).
5. `cargo audit --file Cargo.lock` → exit 0; diff against
   `audit-before.log` and confirm no new advisory and no change to
   `.cargo/audit.toml` (the four ignores stay).
6. `cargo build --workspace` → exit 0.
7. Record actual digest evidence, not just test determinism:
   - Known-answer SHA-256 over fixed input (e.g. `b"abc"`) built against
     sha2 0.11.1 and 0.11.2 in a scratch crate → identical hex, equal to the
     standard vector.
   - Suites that pin literal digests (`camel-cli` signature test, the cxf
     signer test) and the proto-compiler/xtask hash suites → exit 0.
8. `cargo test --workspace --lib` → exit 0.
9. Affected lanes, each asserted exit 0 (docker rows are CI-owned and
   identified separately):
   ```
   cargo test -p camel-core --test hexagonal_architecture_boundaries_test
   cargo test -p camel-component-http
   cargo test -p camel-test --features integration-sql --test sql_deadline_ordering
   cargo test -p camel-endpoint             # trybuild
   cargo test -p camel-auth                 # trybuild
   cargo test -p camel-endpoint-macros      # trybuild
   cargo test -p xtask
   cargo test -p camel-component-file
   cargo test -p camel-bridge
   cargo test -p camel-proto-compiler
   cargo test -p camel-component-jms
   cargo test -p camel-component-mqtt
   cargo test -p camel-component-validator
   cargo test -p camel-xslt
   cargo test -p camel-component-grpc
   cargo test -p camel-integration-test
   cargo test -p camel-api
   cargo test -p camel-dsl
   cargo test -p camel-builder
   cargo test -p camel-lint
   cargo test -p camel-test --features integration-tests --no-run
   ```
   Remaining consumer crates without a declared local lane are covered by
   `cargo build --workspace` + `cargo test --workspace --lib`. Docker rows
   (`camel-component-kafka`, redis, jms/cxf/xslt integration, mail,
   kubernetes) are CI-owned and not run here; record that explicitly.
10. AGENTS.md clippy registry, exactly:
    ```
    cargo clippy --workspace --all-features --exclude camel-cli \
      --exclude camel-component-kafka --exclude security-keycloak \
      --exclude security-wasm-policy --all-targets -- -D warnings
    cargo clippy -p camel-component-kafka --all-targets -- -D warnings
    cargo clippy -p camel-cli -- -D warnings
    cargo clippy -p camel-cli --no-default-features \
      --features flavor-regular,exec --all-targets -- -D warnings
    ```
11. `bash scripts/gates-local.sh` as a transient
    `systemd-run --user --unit=gates-dep-hygiene --collect
    --setenv="PATH=$PATH" --working-directory="$PWD" ...` unit, polled
    in-turn until exit; any commit after the sweep forces a re-run. The final
    gate table must also carry `schema-check` and `doc-build` exit codes.
    Note: the lane phase selects crates by changed paths, not lock
    dependencies (`scripts/gates-local.sh:727–737`), so a Cargo.lock-only
    diff runs no lanes — step 9 covers them explicitly.
12. `cargo outdated --workspace --root-deps-only` (network; via
    `nix shell nixpkgs#cargo-outdated -c ...`) → the six bumped crates
    disappear from the list; filetime (0.2.30 held), zip, parking_lot and the
    other out-of-scope entries remain. Save the after-list.

**Tests:** (pre-existing suites — no new tests; run each and assert exit 0)
- Duplicate-set check: 59 distinct names with 2+ versions, identical
  before/after sets; 5539 dump lines; `parking_lot` two versions; `zip`
  9.2.2 unchanged; `filetime` 0.2.29.
- `cargo audit --file Cargo.lock`: exit 0; no advisory delta.
- `cargo build --workspace`: exit 0.
- Digest evidence from step 7.
- `cargo test --workspace --lib`: exit 0.
- The 21 lane commands in step 9: exit 0 each.
- The four clippy invocations: exit 0 each, verified by hand (gates-local's
  clippy verdict is known-spurious, bd rc-rpv43).
- `bash scripts/gates-local.sh`: exit 0 on the final commit, with
  `schema-check` and `doc-build` exit codes recorded.

**Acceptance:**
- `Cargo.lock` contains bytes 1.12.2, sha2 0.11.2 (only the 0.11 line; 0.10.9
  may remain transitively), trybuild 1.0.125, ureq 3.5.2, walkdir 2.5.1,
  hyper 1.13.0, ureq-proto 0.7.1. `filetime` remains 0.2.29; `zip` remains
  9.2.2.
- No manifest file modified; no duplicate `(name, version)` edge flipped away
  from the baseline.
- Duplicate-name set unchanged (59 names, same set).
- `cargo audit` exit 0 with the ignore list untouched.
- Lock diff and before/after duplicate sets saved for the park.
- If a bump forced a source edit, changed a rust-camel behaviour requirement,
  or introduced a parallel version: stop and park with the evidence; do not
  ship a partial batch.

**Rollback:** if any bump requires a source edit, changes a rust-camel
behaviour requirement, or introduces a parallel version, stop and park with
the evidence. Do not ship a partial batch.

- [x] 1.1
