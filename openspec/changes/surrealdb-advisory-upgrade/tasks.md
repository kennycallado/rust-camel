# Tasks: surrealdb-advisory-upgrade

Single-phase verification-class change (design.md `## Phases` declares one
phase; no `## Phase N` headings here). STAGE 4 quality gates (clippy, fmt,
lint battery, final `cargo audit`, doc-guide-test, holistic review) and the
fleet-inbox park file (`.opencode/fleet/inbox/surrealdbadv-parked.json`,
mission order Reporting section) are conductor-owned closing steps and
intentionally not worker tasks.

**Containment (all Tasks 1.2 / 1.3 cargo invocations).** Run every cargo
command inside the mission containment scope, foreground, per design.md
Verification strategy and the mission order PATH warning:

    systemd-run --user --scope --collect --unit=fleet-surrealdbadv \
      -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill \
      -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6 \
      PATH="/nix/store/3f9dx2fh65r95i8cwrr1qqsjnqbbgf9w-sccache-0.15.0/bin:/nix/store/0592hadxldzhhd3ppc6nskslzd39fwlz-gcc-wrapper-15.3.0/bin:/nix/store/2j53g1z8k3p41x7nzdayl7x5l2ffrc9g-gnumake-4.4.1/bin:/nix/store/1lw1c6srhq7nsigvj0dymzg73w450sy4-rust-default-1.99.1/bin:/nix/store/6j49hlq8na8px4z8a6mamf3qayf9041f-cargo-audit-0.22.1/bin:/nix/store/kwc34h21my8mf7xbqm9gxs1xfx07xb4i-coreutils-9.11/bin:/run/current-system/sw/bin:/usr/bin:/bin" \
      cargo <command> -j4

This PATH is the validated devshell subset (sccache, gcc-wrapper `cc`,
gnumake, rust-default cargo/rustc, cargo-audit, coreutils,
/run/current-system/sw/bin, /usr/bin, /bin; docker resolves via
/run/current-system/sw/bin). Before and after each scoped run:
`pgrep -c compiled_artifa` must be 0; `systemctl --user stop
fleet-surrealdbadv.scope` after each run. Docker must be reachable from
inside the unit for Task 1.3.

## Dependency bump and evidence

### Task 1.1: Bump surrealdb to 4.0.0 and capture lock/audit evidence

**Files:**
- `Cargo.toml` (modified)
- `Cargo.lock` (modified)
- `openspec/changes/surrealdb-advisory-upgrade/evidence.md` (new)

**Steps:**
1. In root `Cargo.toml`, change the workspace dependency line
   `surrealdb = { version = "3", default-features = false, features = ["protocol-ws", "protocol-http", "rustls", "kv-mem"] }`
   to the identical line with `version = "4"`. Touch nothing else in the file.
2. Run `cargo update -p surrealdb` (worktree root). Do NOT run a bare
   `cargo update`.
3. Verify `Cargo.lock`: `surrealdb` = `4.0.0`, `surrealdb-core` = `4.0.0`,
   `affinitypool` ≥ `0.8.0`. If resolution fails outright, or any of the
   three version requirements misses, STOP and report
   `lock-resolution-failure: <resolver output or the version mismatch>`.
4. Record the lock delta against base commit `c70c29cb0` (working tree —
   the change is not yet committed):
   `git diff c70c29cb0 -- Cargo.lock > /home/shared/tmp/surrealdbadv-evidence/lock-diff.txt`
   and summarize the moved crates (count + names of the non-`surrealdb*`
   transitives) in `evidence.md`.
5. Run `cargo audit` and save full output to
   `/home/shared/tmp/surrealdbadv-evidence/audit-after.txt`. Expect exit 0,
   `0 vulnerabilities`, the same 7 allowed warnings as
   `audit-before.txt`. If any vulnerability appears, STOP and report
   `audit-regression: <advisory ids>`. If the warning set differs from
   `audit-before.txt`, STOP and report `warning-set-drift: <ids>` — do not
   guess.
6. Run the inverted/dup trees and save each under
   `/home/shared/tmp/surrealdbadv-evidence/` as `tree-surrealdb.txt`,
   `tree-jwt.txt`, `tree-ed25519.txt`, `tree-dup-target-all.txt`. The
   jsonwebtoken and ed25519-dalek specs are AMBIGUOUS (two versions in
   lock) and must be qualified; capture stderr too:
   `cargo tree --target all -i surrealdb`,
   `cargo tree --target all -i 'jsonwebtoken@10.4.0'`,
   `cargo tree --target all -i 'jsonwebtoken@11.1.0'`,
   `cargo tree --target all -i 'ed25519-dalek@2.2.0'`,
   `cargo tree --target all -i 'ed25519-dalek@3.0.0'`,
   `cargo tree -d --target all 2>&1`. Save the two jsonwebtoken trees to
   `tree-jwt.txt` (the second with `>>`; same for the two dalek trees in
   `tree-ed25519.txt`), and require the `surrealdb-core` chain line on the
   `@10.4.0` / `@2.2.0` trees only (the 11.1.0 / 3.0.0 trees route through
   camel-auth / camel-cli, not surrealdb-core). Each saved file MUST be
   non-empty (ambiguous-spec exit-101 output means the evidence silently
   degraded).
7. Create `evidence.md` in the change dir with sections: base commit +
   worktree, before-audit summary (2 vulnerabilities: RUSTSEC-2026-0453,
   -0454, both 3.2.4), after-audit summary (0 vulnerabilities, 7 allowed
   warnings), version table (`surrealdb`, `surrealdb-core`, `affinitypool`,
   `reblessive`, plus post-bump versions of `rsa`, `rkyv`, `ammonia`,
   `crypto-bigint` — the crates behind the surrealdb-chain advisories in
   `.cargo/audit.toml`) before → after, lock-delta summary, one line
   `audit.toml unchanged — all 5 ignores still encountered (warning set
   identical to audit-before.txt)` — written only when the warning set
   matches; if `warning-set-drift` fired, record the drift instead — and dup-tree findings
   (jsonwebtoken 10.4.0 + ed25519-dalek 2.2.0 remain via surrealdb-core —
   ecosystem-blocked, out of scope, same class as mission 515's deferral).

**Tests:** (verification commands, not new test code)
- `lock-versions`: setup = bumped lock → action =
  `grep -A1 '^name = "surrealdb"$' Cargo.lock` and same for
  `surrealdb-core` → assert versions are exactly `4.0.0` / `4.0.0`.
- `audit-clean`: setup = bumped lock → action = `cargo audit` → assert exit
  0 and output contains `0 vulnerabilities` (wording per cargo-audit 0.22:
  no `error:` line, warnings section lists exactly the 7 pre-allowed
  advisories from `audit-before.txt`).

**Acceptance:**
- `git diff c70c29cb0 -- Cargo.toml` shows exactly one changed line
  (the `surrealdb` requirement `"3"` → `"4"`).
- `cargo audit` exit 0; `evidence.md` contains the version table and both
  audit summaries.
- Evidence files exist in `/home/shared/tmp/surrealdbadv-evidence/` and are
  NON-EMPTY: `lock-diff.txt`, `audit-after.txt`, `tree-surrealdb.txt`,
  `tree-jwt.txt` (both qualified jsonwebtoken trees, showing the
  `surrealdb-core` chain), `tree-ed25519.txt` (both qualified
  ed25519-dalek trees), `tree-dup-target-all.txt`.

- [x] 1.1

## Verification

### Task 1.2: Local compile + suite verification after the bump

**Files:**
- `openspec/changes/surrealdb-advisory-upgrade/evidence.md` (modified —
  appends a verification section)

**Steps:**
1. `cargo build --workspace` (worktree root). Expect exit 0.
2. `cargo check --workspace --all-targets`. Expect exit 0.
3. `cargo test -p camel-component-surrealdb --all-targets`. Expect all
   green. Record per-binary `test result:` lines in `evidence.md`.
4. `cargo test -p camel-test --all-targets`. Expect all green (this does
   NOT compile the feature-gated surrealdb suite; that is Task 1.3).
   Record per-binary summary lines.
5. `cargo test --workspace --lib`. Expect all green. Record the aggregate
   pass/fail counts.
6. If any step fails with a surrealdb-4.x-behavior error (compile or test),
   apply the minimal adapter fix in the affected crate, record the diff and
   the reason in `evidence.md` ("test change rationale"), and re-run the
   failed command until green. If a fix would change the component's
   public surface or documented behavior, STOP and report
   `behavior-change-needs-design: <what>` instead of improvising.

**Tests:** (existing suites, no new test code)
- `component-suite`: setup = bumped lock → action =
  `cargo test -p camel-component-surrealdb --all-targets` → assert exit 0
  and `0 failed` on every binary; record executed-test counts (must be > 0).
- `workspace-lib`: setup = bumped lock → action =
  `cargo test --workspace --lib` → assert exit 0, aggregate `0 failed`,
  executed-test count recorded and > 0.

**Acceptance:**
- All five commands exit 0 with counts recorded in `evidence.md`
  (`cargo build --workspace`, `cargo check --workspace --all-targets`,
  component suite, `camel-test --all-targets`, `cargo test --workspace
  --lib`).
- Any code change made under step 6 is justified line-by-line in
  `evidence.md` and `git diff c70c29cb0 -- '**/*.rs'` contains no
  change outside what that justification covers.

- [x] 1.2

### Task 1.3: Docker-backed SurrealDB runtime gate

**Files:**
- `openspec/changes/surrealdb-advisory-upgrade/evidence.md` (modified —
  appends the runtime-gate section)
- Only if a behavior fix is forced: the specific component/test file named
  in the failure (recorded in `evidence.md` with rationale)

**Steps:**
1. Verify Docker is reachable: `docker info >/dev/null && echo docker-ok`.
   If it fails, STOP and report `docker-unreachable: <error>` — the mission
   order requires naming any gate leg that cannot run and why; do not
   fabricate a pass.
2. Run the feature-gated suite with executed-test requirement:
   `cargo test -p camel-test --features integration-tests --test surrealdb_test`.
   The suite is `#![cfg(feature = "integration-tests")]` and spins the
   testcontainers server fixture pinned `v3.1.4`
   (`crates/camel-test/tests/support/surrealdb.rs`).
3. Assert executed tests > 0 and `0 failed`. A "0 tests" pass (e.g. feature
   not actually enabled) is a FAILURE of this task — re-check the command.
4. Record per-`test result:` lines and total counts in `evidence.md` under
   `## Runtime gate (Docker, server v3.1.4)`.
5. If tests fail on 4.x behavior drift (query result shapes, LIVE-select
   payloads, auth handshake), classify first:
   - **Contract-level drift** (public surface, URI parameters, error
     taxonomy, documented producer/consumer contracts): STOP and report
     `behavior-change-needs-design: <contract>` — a design revision and
     fresh spec-bless are required; do not improvise.
   - **Documentation-only drift** (the code is correct under 4.0.0 but
     prose that describes upstream behavior is stale): apply the minimal
     adapter fix if one is needed, record it, report
     `documented-behavior-moved: <contract>` for Task 1.4, and continue —
     do not edit docs in this task.

**Tests:** (existing suite, executed)
- `surrealdb_test`: setup = Docker up, feature enabled, server container
  `v3.1.4` → action =
  `cargo test -p camel-test --features integration-tests --test surrealdb_test`
  → assert exit 0, executed tests > 0, `0 failed`; record counts.

**Acceptance:**
- Suite green with executed tests > 0; counts recorded in `evidence.md`.
- Any adapter fix is justified in `evidence.md` and covered by the Task 1.2
  acceptance diff rule.

- [x] 1.3

## Documentation

### Task 1.4: CHANGELOG entry and evidence finalization

**Files:**
- `CHANGELOG.md` (modified)
- `openspec/changes/surrealdb-advisory-upgrade/evidence.md` (modified —
  final pass)
- `crates/components/camel-component-surrealdb/CONTEXT.md` and/or
  `docs/src/components/surrealdb.md` (modified) — ONLY if Task 1.3 reported
  `documented-behavior-moved`; otherwise these two files are explicitly
  untouched and `evidence.md` records `doc-guide-test: N/A — no docs
  changed`.

**Steps:**
1. Add one entry under the existing `## Unreleased` → `### Changed` section
   of `CHANGELOG.md`, first line of the section, style-matched to the
   neighboring entries:
   `change(deps): surrealdb 3.2.4 → 4.0.0 clears RUSTSEC-2026-0453 (KV
   use-after-free via affinitypool) and RUSTSEC-2026-0454 (reblessive
   Send/Sync); workspace requirement moves "3" → "4" with the same feature
   set, zero component code changes (or only the minimal adapter fixes
   recorded in evidence.md), cargo audit now vulnerability-free
   (bd rc-yhpyz).` (Compress to one bullet; keep the bd reference. Use the
   parenthetical clause only if adapter fixes were actually made.)
2. If (and only if) `documented-behavior-moved` was reported: update the
   specific prose in `crates/components/camel-component-surrealdb/CONTEXT.md`
   and/or `docs/src/components/surrealdb.md` that the drift invalidates,
   quoting the observed 4.0.0 behavior.
3. Finalize `evidence.md`: verify every section from Task 1.1 step 7 plus
   the Task 1.2/1.3 verification sections are present, add a
   `## Gate provenance` table mapping each design.md gate (items 1–10) to
   its run location (task vs conductor STAGE 4), and add the exact final
   `Cargo.lock` versions of `surrealdb` and `surrealdb-core`.
4. Scan `evidence.md` for forbidden placeholders (TBD/TODO/`...`) — zero
   required.

**Tests:** (format checks)
- `changelog-class`: setup = edited CHANGELOG → action = read the
  `### Changed` section → assert the new bullet is present, starts with
  `change(deps):`, and references `bd rc-yhpyz`.
- `evidence-complete`: setup = finalized evidence.md → action = check
  sections → assert: base commit, before/after audit summaries, version
  table, lock delta, dup-tree findings, component + workspace-lib +
  Docker runtime counts, gate provenance table, zero placeholder hits.

**Acceptance:**
- `CHANGELOG.md` entry present as specified.
- `evidence.md` complete per `evidence-complete`; zero TBD/TODO/ellipsis.
- If docs were touched: `cd docs && mdbook test` exits 0; else
  `doc-guide-test: N/A — no docs changed` recorded.
- No edits to `crates/components/camel-component-surrealdb/CONTEXT.md` or
  `docs/**` unless justified by a recorded `documented-behavior-moved`.

- [x] 1.4
