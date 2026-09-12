# Tasks: bench-axum-bare

## Records baseline

### Task 1.1: Capture baseline records-state before any roster edit

**Files:**
- `openspec/changes/bench-axum-bare/baseline-records.sha256` (new — PURE
  sha256sum output only; consumed by the byte-identity diffs in 3.2/6.1)
- `openspec/changes/bench-axum-bare/baseline-check.txt` (new — the green
  `--check` evidence; NEVER mixed into the sha256 file)

**Steps:**
1. From the worktree root run
   `python3 benchmarks/harness/summarize.py --check benchmarks/records`
   and capture its exit code + final output into
   `openspec/changes/bench-axum-bare/baseline-check.txt`. Also run
   `git diff --exit-code -- benchmarks/records` (records tree must be
   clean in git BEFORE this change — record that in the same file).
2. Run `find benchmarks/records -type f \( -name 'run.json' -o -name 'summary.md' -o -name 'index.json' \) | sort | xargs sha256sum > openspec/changes/bench-axum-bare/baseline-records.sha256`
   (from the worktree root so paths are repo-relative; the file must
   contain ONLY sha256sum lines — anything else breaks the later
   empty-diff proof). Hash ONLY the published record artifacts —
   deliberately EXCLUDE `records/SCHEMA.md`: it is a mutable contract
   document that Task 5.1 legitimately edits; byte-identity is asserted
   over record content, not over docs.
3. Commit nothing yet — evidence consumed by Tasks 3.2 and 6.1.

**Tests:** (executable spec — name, arrange, act, assert)
- `baseline-check-green`: worktree at spec-bless commit → run
  `python3 benchmarks/harness/summarize.py --check benchmarks/records` →
  exit code 0 and no per-record mismatch lines on stdout.
- `baseline-hash-captured`: after step 2 → the sha256 file exists and its
  line count equals the file count from the same `find` (today: 3 —
  run.json, summary.md, index.json of the single era-2 record).

**Acceptance:**
- `baseline-records.sha256` exists, pure sha256sum lines, count == find
  count; `baseline-check.txt` records the green `--check` result AND the
  clean `git diff --exit-code -- benchmarks/records`.
- This task completes BEFORE any edit to summarize.py / run.sh / warm-24.py
  (it is the byte-identity baseline; e_glm pre-flight ruling).

- [x] 1.1

## Fixture crate

### Task 2.1: axum-bare fixture crate with TDD integration test

**Files:**
- `benchmarks/contenders/axum-bare/Cargo.toml` (new)
- `benchmarks/contenders/axum-bare/.cargo/config.toml` (new)
- `benchmarks/contenders/axum-bare/src/main.rs` (new)
- `benchmarks/contenders/axum-bare/tests/integration.rs` (new)
- `Cargo.toml` (modified — workspace `members` append
  `benchmarks/contenders/axum-bare` ONLY; it is implicitly outside
  `default-members`, do not touch anything else: no deps, no profiles, no
  metadata — blessed out-of-zone exception, exact scope)

**Steps:**
1. Write `tests/integration.rs` FIRST (both tests below), run
   `env -u CARGO_TARGET_DIR cargo test -p axum-bare-fixture` from the crate
   dir and confirm it FAILS (crate does not exist yet / binary missing).
2. Create `Cargo.toml`: package `axum-bare-fixture`, edition 2021, `[[bin]]`
   name `axum-bare-fixture` path `src/main.rs`; deps:
   `axum.workspace = true` (root Cargo.toml:203 `axum = { version = "0.8" }`
   carries no extra features) and DIRECT
   `tokio = { version = "1", features = ["rt-multi-thread", "macros", "net"] }`
   — do NOT use `tokio.workspace = true`: the root workspace tokio dep
   carries `features = ["full"]` (root Cargo.toml:145), which contradicts
   the minimal feature set and inflates the fixture's dep graph;
   `[dev-dependencies]` none beyond tokio itself (test uses std
   TcpStream). NO camel-* dependency anywhere.
3. Create `.cargo/config.toml` pinning fixture-local
   `target-dir = "target"` (copy the rust-camel-lib fixture pattern at
   `benchmarks/contenders/rust-camel-lib/.cargo/config.toml`, including its
   NOTE comment about CARGO_TARGET_DIR precedence).
4. Implement `src/main.rs`:
   - `const DEFAULT_PORT: u16 = 8080;` read `BENCH_AXUM_BARE_PORT` env
     override (parse failure → stderr warn + DEFAULT_PORT, mirroring
     `benchmarks/harness/loadgen/src/bin/devnull.rs` unwrap_or pattern).
   - `#[tokio::main]` (default multi-thread runtime).
   - `tokio::net::TcpListener::bind(("0.0.0.0", port)).await?` — on error
     print `axum-bare: error: <e>` to stderr, exit 1 (matches devnull bin).
   - After bind: spawn the `axum::serve(listener, app)` future, then print
     `BENCH_ROUTE_READY` to stdout and CALL
     `std::io::stdout().flush()` (keep the explicit flush per the devnull
     precedent, cli_runtime.rs run_devnull — println!'s LineWriter would
     emit the line, but the flush makes the marker-then-serve ordering
     explicit and matches the family convention; do NOT claim block-buffer
     hazard in comments — std stdout is line-buffered even when piped).
   - Router: single route `/bench` accepting any method. Handler: take
     `axum::body::Body`, print `BENCH_HTTP_REQUEST received`, drain via
     `axum::body::to_bytes(body, 1 MiB)` (REQUIRED mechanism — do not
     substitute an http-body-util dep unless axum's re-export is genuinely
     insufficient, in which case stop and report), increment
     `Arc<AtomicU64>` counter (`fetch_add(1, Relaxed) + 1`) and print
     `BENCH_HTTP_REQUEST id=<n>`, return `(StatusCode::OK,
     [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], "pong")`.
   - Doc comments: module header states purpose (rc-u034 reference
     contender isolating hyper/axum/tower/tokio stack-tax from camel-tax;
     measured at the next canonical run) and the marker contract.
5. Append `benchmarks/contenders/axum-bare` to root workspace `members`
   (NOT default-members). A new workspace member ALWAYS gains a package
   entry in Cargo.lock (precedent: the rust-camel-lib fixture entry at
   Cargo.lock:9574) — inspect `git diff Cargo.lock` and confirm the diff
   contains ONLY the `axum-bare-fixture` package entry with dependency
   edges to already-pinned versions (axum 0.8.9, tokio); ANY third-party
   version change or new third-party package → stop and report (blessed
   acceptance criterion).
6. Run the tests again — both must pass. Run
   `env -u CARGO_TARGET_DIR cargo clippy -p axum-bare-fixture --all-targets -- -D warnings`
   and `cargo fmt --check` from the crate dir; fix anything.

**Tests:** (executable spec)
- `marker_then_two_keepalive_requests_succeed`: test spawns
  `env!("CARGO_BIN_EXE_axum-bare-fixture")` with
  `BENCH_AXUM_BARE_PORT=<free port>` (bind a TcpListener to port 0, read
  its port, drop it) and `Command::env`; stdout is read by a dedicated
  reader thread with an overall deadline (channel + 5 s timeout — the
  test FAILS on deadline, never hangs) → assert a stdout line equals
  `BENCH_ROUTE_READY` within the deadline → open ONE `std::net::TcpStream`
  with `set_read_timeout(5s)`, write POST `/bench` with
  `Content-Length: 32768` and a 32768-byte body issued as 3 separate
  `write_all` calls (several client writes; TCP may coalesce them — the
  drain proof is the keep-alive reuse, not the write count), read the response HEADERS then EXACTLY the
  Content-Length byte count of body (framed reading — never read-to-EOF),
  assert `HTTP/1.1 200`, header `content-type: text/plain; charset=utf-8`,
  body `pong` → on the SAME stream write an identical second POST, read
  framed response, assert 200 + `pong` again (keep-alive reuse = drain
  proof) → assert collected stdout lines contain
  `BENCH_HTTP_REQUEST received` (≥2) and `BENCH_HTTP_REQUEST id=1` and
  `id=2` → the child process is killed and `wait()`-ed in EVERY path
  (success, assertion failure, panic — use a RAII guard struct with
  `Drop` that kills+waits, so a failed assert cannot leak the server).
- `bind_failure_exits_nonzero`: spawn with
  `BENCH_AXUM_BARE_PORT=<port held by a pre-bound std TcpListener>` →
  assert process exits nonzero within 10 s and stderr contains `error`
  (same RAII cleanup guard).
- command: `env -u CARGO_TARGET_DIR cargo test -p axum-bare-fixture` from
  `benchmarks/contenders/axum-bare` (debug profile is fine — the harness
  builds release separately; timing is NOT under test).
- expected: fail before step 4, pass after.

**Acceptance:**
- `cargo test -p axum-bare-fixture` green; clippy `-D warnings` green;
  `cargo fmt --check` green (run `cargo fmt` in the crate as needed).
- `cargo tree -p axum-bare-fixture` output contains no `camel-` crate.
- `git diff Cargo.lock` shows ONLY the `axum-bare-fixture` package entry
  (no third-party version changes, no new third-party packages).
- `cargo doc -p axum-bare-fixture --no-deps` succeeds without warnings
  (RUSTDOCFLAGS="-D warnings").

- [x] 2.1

## Harness roster wiring

### Task 3.1: run.sh reference-contender registration

**Files:**
- `benchmarks/harness/run.sh` (modified)

**Steps:**
1. Add beside `SCENARIO_ARTIFACT_SET` (near line 187) a greppable block:
   `declare -A REFERENCE_CONTENDERS=( ["http-server"]="axum-bare" )` with a
   comment naming the rc-u034 contract (reference cell is http-server-only,
   NOT a Pair A/B member, NOT FAMILY_COMPLETENESS-declared — per-scenario
   opt-in is the documented mechanism).
2. Add `resolve_axum_bare_bin()` beside `resolve_devnull_bin()` (near line
   2058): resolve
   `benchmarks/contenders/axum-bare/target/release/axum-bare-fixture`
   relative to the worktree root — copy `resolve_rust_lib_bin`'s
   missing-binary idiom VERBATIM (run.sh:894: dry-run placeholder
   a would-build placeholder-echo branch (echo the path that would be built) + loud error and nonzero exit only on a real
   run; NOT devnull's shared-root-target idiom via
   `resolve_cargo_target_dir`, which points at the shared root target the
   fixture-local pin keeps empty).
3. Register the cell in `resolve_all_cells` (run.sh:1548-1790 — the
   full-scenario registration loop; NOTE: `resolve_bridge_scenario_cells`
   at :1326 is the BRIDGE-only path and must NOT be touched — registering
   there would be a silent no-op because REFERENCE_CONTENDERS has no bridge
   keys). Insert AFTER the per-scenario rust-camel-cli registration
   (~:1789, before the function's closing brace) — this is the same code
   region the core 6 add_cells and the node family loop (:1625) feed:
   `if [[ -n "${REFERENCE_CONTENDERS[$scenario]:-}" ]]; then local axum_bin; axum_bin="$(resolve_axum_bare_bin)"; if [[ "$DRY_RUN" == "true" && ! -x "$axum_bin" ]]; then echo "dry-run: cell $scenario/${REFERENCE_CONTENDERS[$scenario]} deferred (release binary not built: $axum_bin)"; else add_cell "$scenario" "${REFERENCE_CONTENDERS[$scenario]}" "$axum_bin" "BENCH_ROUTE_READY"; fi; fi`
   (deferral condition MUST be `"$DRY_RUN" == "true"` — run.sh sets
   DRY_RUN to the strings false/true (:254/:326) so `-n "$DRY_RUN"` is
   always true; defer semantics mirror the rust-camel-cli branch
   :907/:1566).
4. Extend the static completeness guard `_expected_cell_map`
   (run.sh:3261-3296, comment at :3267-3268 pins "= 52"): inside its
   per-scenario loop add the same conditional REFERENCE_CONTENDERS
   iteration, using the SAME deferred semantics the rust-camel-cli entry
   uses there (:3279-3281) — when the reference cell's registration was
   deferred (dry-run, binary missing), the expected map must ALSO drop it
   or the guard aborts `expected 53, got 52`; update the :3267 comment to
   53 (5×8 + 2×6 + 1 reference).
5. In the m1 console summary loop (the per-scenario block that prints the `=== Pair A` then `=== Pair B` tables, at ~line 3517), after the Pair B loop add a short block printing
   `  <ref>: time/rss` from the same `$SCRATCH_DIR/$local_safe.txt` idiom
   for every nonempty `REFERENCE_CONTENDERS[$scenario]` (so the reference
   cell is visible in the run log; keep it outside the Pair tables).
6. `bash -n benchmarks/harness/run.sh` — syntax must pass.

**Tests:** (executable spec — ALL local to this task; the python
`registration-site-pinned` assertion lives in Task 3.2 only)
- `runsh-syntax`: worktree after edits → `bash -n benchmarks/harness/run.sh`
  → exit 0.
- `registration-inside-resolve-all-cells` (local grep):
  `awk '/resolve_all_cells\(\) \{/,/^\}/' benchmarks/harness/run.sh | grep -c 'add_cell "\$scenario" "\${REFERENCE_CONTENDERS\[\$scenario\]\}"'`
  → ≥ 1.
- `registration-absent-from-bridge-resolver` (local grep):
  `awk '/resolve_bridge_scenario_cells\(\) \{/,/^\}/' benchmarks/harness/run.sh | grep -c 'REFERENCE_CONTENDERS'`
  → 0.
- command: the three commands above (all must produce the stated result).

**Acceptance:**
- All three local tests above pass (`bash -n` green, registration ≥1 in
  resolve_all_cells, 0 in the bridge resolver).
- grep confirms axum-bare appears NOWHERE in PAIR_A_CONTENDERS /
  PAIR_B_CONTENDERS / FAMILY_COMPLETENESS / warm-24.py.
- grep confirms REFERENCE_CONTENDERS appears in exactly FOUR run.sh sites:
  the declare block (~:187), the `resolve_all_cells` registration (~:1789),
  the `_expected_cell_map` extension, and the m1 summary print — never in
  `resolve_bridge_scenario_cells` and never in the PAIR arrays.

- [x] 3.1

### Task 3.2: summarize.py roster + drift-test extension + byte-identity proof

**Files:**
- `benchmarks/harness/summarize.py` (modified)
- `benchmarks/harness/test_summarize.py` (modified)

**Steps:**
1. In summarize.py, beside `FULL_CONTENDERS` (~line 157) add
   `HTTP_REFERENCE_CONTENDERS = {"http-server": ("axum-bare",)}` with a
   comment mirroring the rc-2k33 three-file contract note (run.sh
   REFERENCE_CONTENDERS is the bash projection; the drift test guards
   equality).
2. Extend `expected_roster(scenarios)` (~line 836): after the
   FULL/BRIDGE tuple choice, `contenders += HTTP_REFERENCE_CONTENDERS.get(scenario, ())`.
   Update its docstring: 5×8 + 2×6 + 1 reference = 53.
3. In test_summarize.py `test_roster_mirror_no_drift` (~line 1218):
   update the header comment (52→53 arithmetic naming the reference
   projection), then extend the test body:
   - grep run.sh for `declare -A REFERENCE_CONTENDERS` entries
     (`\["([^"]+)"\]="([^"]+)"` inside the block) and assert the resulting
     `{scenario: {member}}` mapping equals
     `{k: set(v) for k, v in summarize.HTTP_REFERENCE_CONTENDERS.items()}`
     (both directions come free: single equality on both-derived dicts).
   - keep every existing assertion untouched (PAIR 4+4 == FULL_CONTENDERS,
     warm-24 mirror, bridge set, WARM_APPLICABLE).
4. Update the 52-pins to 53: line ~1220 (comment), ~1346 (arithmetic:
   assert `5*8 + 2*6 + 1 == 53` — literal expression with the reference
   cell named in the comment, not a bare constant swap), ~1349
   (`len(summarize.expected_roster(sorted(all_scenarios))) == 53`), ~1353 (comment), ~1411
   (`len(record["expected_cells"]) == 53`).
5. The fixture-record test at ~1411 (its meta scenarios DO include
   http-server, test_summarize.py:1362) needs an UNCONDITIONAL fixture
   extension or it goes red: its m1 loop (:1371-1378) and flat
   protocol-A m2 loop (:1381-1386) iterate only FULL_CONTENDERS — after
   the roster change, `expected_cells`=53 with no axum-bare evidence ⇒
   `completeness_gaps` non-empty (:1412) and publish exits ≠0 (:1431).
   Extend BOTH loops to also emit the `http-server_axum-bare` cell
   (mirror the existing per-cell fixture idiom in each loop exactly —
   same dir shapes, same file contents pattern).
6. Add `test_reference_contender_http_only`: for each of the 7 active
   scenarios, `expected_roster([s])` contains `http-server/axum-bare` iff
   `s == "http-server"`; `axum-bare` not in any Pair (grep run.sh source:
   the `declare -a PAIR_A_CONTENDERS` / `PAIR_B_CONTENDERS` lines must not
   contain the token `axum-bare`). Add the `registration-site-pinned`
   test from Task 3.1's Tests block (resolve_all_cells body ≥1 match,
   resolve_bridge_scenario_cells body 0 matches).
7. `checks/parity-52.py` (dormant Phase A gate, EXPECTED_CELLS = 52 at
   :49, no caller in run.sh/run-all.sh — verify with a repo-wide grep
   excluding `archive/` before touching): bump EXPECTED_CELLS to 53 and,
   if the repo-wide grep confirms zero references to the filename, rename
   to `checks/parity-53.py` (name embeds the count); if ANY reference
   exists, keep the filename and bump only the constant + header comment.
8. Re-run the FULL suite: `python3 -m unittest discover -s
   benchmarks/harness -p 'test_*.py' -v` — the three harness test files
   total 77 today (test_summarize.py 50 + test_discover.py 2 + 25 others);
   after additions expect 79+ (50→52+ in test_summarize.py), all green.
9. Byte-identity proof: `python3 benchmarks/harness/summarize.py --check
   benchmarks/records` → exit 0; then
   `find benchmarks/records -type f \( -name 'run.json' -o -name 'summary.md' -o -name 'index.json' \) | sort | xargs sha256sum | diff - openspec/changes/bench-axum-bare/baseline-records.sha256`
   → EMPTY diff (proves published-record regeneration is byte-identical to
   the Task 1.1 baseline; SCHEMA.md is excluded by design — see Task 1.1
   step 2).

**Tests:** (executable spec)
- `test_roster_mirror_no_drift`: extended per step 3 → run → pass.
- `test_expected_roster_53_cells`: existing arithmetic test updated →
  `len(expected_roster(all 7)) == 53` and
  `"http-server/axum-bare" in expected_roster(all 7)`.
- `test_reference_contender_http_only` + `registration-site-pinned`: per
  steps 6 / Task 3.1 → pass.
- `records-byte-identity`: per step 9 → diff empty, --check exit 0.
- command: `python3 -m unittest discover -s benchmarks/harness -p 'test_*.py'`.

**Acceptance:**
- Whole harness python suite green (79+ tests across the three files).
- `--check` green AND sha256 diff vs baseline-records.sha256 is EMPTY.
- warm-24.py file untouched (`git diff --stat` shows no
  benchmarks/harness/checks/warm-24.py line).

- [x] 3.2

## Builder and smoke

### Task 4.1: build-all step + http-server smoke case + committed log

**Files:**
- `benchmarks/harness/builder/build-all.sh` (modified)
- `benchmarks/scenarios/http-server/smoke/run.sh` (modified)
- `benchmarks/scenarios/http-server/smoke/axum-bare.log` (new — committed
  evidence from a real run)

**Steps:**
1. In build-all.sh, after the rust-camel-lib block, add an axum-bare block
   mirroring it: `(cd "$REPO_ROOT"/benchmarks/contenders/axum-bare && env -u
   CARGO_TARGET_DIR cargo build --release -p axum-bare-fixture 2>&1 | tail -3)`
   with the same echo-arrow header comment style.
2. Extend smoke/run.sh: add an optional first-arg artifact filter
   (`ARTIFACT_FILTER="${1:-}"` — when set, only the matching artifact case
   runs; existing no-arg behavior unchanged). Add the axum-bare case:
   resolve the fixture binary (`$WORKTREE/benchmarks/contenders/axum-bare/target/release/axum-bare-fixture`),
   pick a free port, launch with `BENCH_AXUM_BARE_PORT=$port`, wait for
   `BENCH_ROUTE_READY` on stdout (same wait loop idiom as the other
   cases), POST `/bench` to `$port` — NOTE: the existing `post_smoke()`
   helper hardcodes `nc 127.0.0.1 8080` (smoke/run.sh:89-90), so add a
   port-parameterized variant (e.g. `post_smoke_port <port>`) for this
   case instead of modifying the helper's call sites — with a small body,
   assert the response contains `200` and body `pong`, and captured
   stdout contains `BENCH_HTTP_REQUEST received` and
   `BENCH_HTTP_REQUEST id=1`; kill the process; write the transcript to
   `axum-bare.log` in the smoke dir (NO timing-like numbers in the log —
   liveness evidence only).
3. Build the fixture release binary (worktree-local target only:
   `env -u CARGO_TARGET_DIR cargo build --release -p axum-bare-fixture`
   from the crate dir).
4. Check /proc/loadavg (< 2.0) and no cargo/rustc churn (`pgrep -f 'cargo|rustc'`
   empty besides none), then run
   `bash benchmarks/scenarios/http-server/smoke/run.sh axum-bare` → must
   exit 0; verify `axum-bare.log` contains marker + id=1 + 200/pong and no
   timing numbers; commit the log.

**Tests:** (executable spec)
- `smoke-axum-bare-green`: fixture built + quiet host →
  `bash benchmarks/scenarios/http-server/smoke/run.sh axum-bare` → exit 0
  and `axum-bare.log` contains `BENCH_ROUTE_READY`,
  `BENCH_HTTP_REQUEST id=1`, `200`, `pong`.
- `smoke-no-arg-unchanged`: `bash -n` on the script; grep confirms the
  no-arg path still iterates the full artifact set (the existing loop is
  wrapped, not replaced).
- command: see above.

**Acceptance:**
- Filtered smoke run green; committed axum-bare.log clean of timing
  numbers.
- `bash -n benchmarks/harness/builder/build-all.sh` green.

- [x] 4.1

## Documentation

### Task 5.1: roster-contract docs, SCHEMA prose, strategy §4 addendum, COVERAGE

**Files:**
- `benchmarks/harness/CONTEXT.md` (modified)
- `benchmarks/records/SCHEMA.md` (modified)
- `benchmarks/docs-investigation-strategy.md` (modified)
- `benchmarks/scenarios/COVERAGE.md` (modified)

**Steps:**
1. harness/CONTEXT.md — the "Roster authored in three places" decision
   row: extend the text with the fourth projection (`REFERENCE_CONTENDERS`
   in run.sh ↔ `HTTP_REFERENCE_CONTENDERS` in summarize.py, guarded by the
   same drift test) and the new arithmetic (5×8 + 2×6 + 1 reference = 53;
   warm-24 stays 24 — the reference contender is not a tick contender).
2. records/SCHEMA.md — expected_cells prose: qualify the count ("8 per
   full scenario, 6 per bridge scenario, plus the http-server reference
   contender for runs whose harness registers it — 53 cells; records
   published before the reference cell joined persist their 52-cell
   roster").
3. docs-investigation-strategy.md §4 — under the rc-audm.6 park note add
   an ADDITIVE supersession block (do NOT rewrite the original note):
   a dated (2026-09-11) pointer that §8 refuted the hypothesized note —
   bracket containment is anti-directional (cli bracket strictly contains
   lib bracket ⇒ asymmetry cannot yield cli < lib; wall-clock hypothesis
   refuted); residual ~1.3 ms is run-condition-associated; adjudicator =
   the next canonical run; probe-cost correction not needed (emit I/O
   outside brackets on both sides).
4. scenarios/COVERAGE.md — record the consolidated location of the
   axum-bare reference contender (crate at
   benchmarks/contenders/axum-bare, http-server-only cell) alongside the
   existing consolidation/exemption records.

**Tests:** (executable spec)
- `docs-no-stale-52`: `grep -n '52' benchmarks/harness/CONTEXT.md
  benchmarks/records/SCHEMA.md` → remaining hits are explicitly
  historical/era-qualified (read each hit; every live-contract sentence
   says 53 or is era-qualified).
- `addendum-additive`: `git diff benchmarks/docs-investigation-strategy.md`
   shows ONLY an addition under the rc-audm.6 §4 note (no removed lines in
   that section).
- command: `git diff --stat` + manual hit review.

**Acceptance:**
- All four docs updated; no unqualified live-contract "52-cell" claims
  remain in the two contract docs; addendum is purely additive.

- [x] 5.1

## Gates

### Task 6.1: full gate sweep + records byte-identity final proof

**Files:**
- `openspec/changes/bench-axum-bare/gate-results.txt` (new — evidence)

**Steps:**
1. From the worktree root run and record exit codes for: `cargo fmt
   --check --all`; `cargo clippy --workspace --all-features --exclude
   camel-cli --exclude camel-component-kafka --exclude security-keycloak
   --exclude security-wasm-policy -- -D warnings`; `cargo clippy -p
   axum-bare-fixture --all-targets -- -D warnings`; `cargo xtask
   lint-unwrap`; `cargo xtask lint-secrets`; `cargo xtask
   lint-non-exhaustive`; `cargo xtask lint-log-levels`; `cargo xtask
   lint-ignore`; `cargo xtask lint-publish-cycles`; `cargo xtask
   lint-component-deps`; `cargo xtask lint-gate-forwarding`; `cargo xtask
   lint-context-citations`; `cargo xtask lint-metric-labels`; `cargo xtask
   schema --check`; `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p
   camel-core -p camel-builder -p camel-dsl -p camel-endpoint -p
   axum-bare-fixture --no-deps`; `cargo check -p axum-bare-fixture`;
   `env -u CARGO_TARGET_DIR cargo test -p axum-bare-fixture` (run from
   `benchmarks/contenders/axum-bare`); `python3 -m unittest discover -s
   benchmarks/harness -p 'test_*.py'`; `python3
   benchmarks/harness/summarize.py --check benchmarks/records`. DELIBERATE
   OMISSIONS (record in gate-results.txt): the AGENTS.md per-package
   clippy invocations for `camel-component-kafka --all-targets` and
   `camel-cli` (benchmarks-only change — neither crate is touched);
   `lint-commits` (conductor policy: remote op, CI owns it); NO
   `cargo test --workspace` (Docker-dependent, conductor policy).
2. Records byte-identity FINAL proof (same command as Task 3.2 step 9 —
   published-artifact filter, SCHEMA.md excluded by design): sha256
   re-hash vs `baseline-records.sha256` must diff empty; record the result.
3. `cargo audit` — run; if it fails on a pre-existing advisory unrelated to
   this change, record it verbatim (conductor adjudicates at review, do not
   fix here).
4. Write every exit code + the byte-identity result into
   `gate-results.txt` under the change dir.

**Tests:** (executable spec)
- `all-gates-recorded`: gate-results.txt lists every command from step 1
  with an exit code; every code is 0 EXCEPT possibly cargo-audit with a
  recorded pre-existing advisory.
- `byte-identity-final`: recorded diff vs baseline is empty.

**Acceptance:**
- Every gate exit 0 (cargo-audit exemption only with verbatim recorded
  pre-existing advisory traceable to main).
- gate-results.txt committed with the change.

- [x] 6.1

## Spec traceability (every delta-spec scenario → owning task/test)

benchmark-suite ADDED `axum-bare reference contender`:
- marker after bind, flushed → 2.1 `marker_then_two_keepalive_requests_succeed`
- T3 route contract with observable drain → 2.1 same test (second
  same-connection POST) + 4.1 smoke assertions
- roster registration is http-server-only → 3.1 registration +
  `_expected_cell_map` extension; 3.2 `test_reference_contender_http_only`
  + `test_expected_roster_53_cells`
- roster drift guard covers the reference tuple → 3.2 extended
  `test_roster_mirror_no_drift` (both directions) +
  `registration-site-pinned`
- no-camel isolation with lock parity → 2.1 acceptance (`cargo tree` no
  camel crate; `git diff Cargo.lock` contains only the
  `axum-bare-fixture` package entry)
- smoke case with committed evidence → 4.1 (filtered run + committed
  `axum-bare.log`, no timing numbers)
- published records stay byte-identical → 1.1 baseline + 3.2 step 9 +
  6.1 step 2 empty-diff proofs

benchmark-suite MODIFIED `Canonical full-matrix run`:
- one command full coverage (53) → 3.1 registration + `_expected_cell_map`
  53; 3.2 `test_expected_roster_53_cells`
- no subset escape hatch / gauges and order / human-invoked execution →
  PRESERVED invariants: no code in this change touches subset handling,
  gauge wiring, or invocation flow; guarded by the existing harness suite
  (3.2 step 8 runs it whole) — no new task needed, by design

benchmark-suite MODIFIED `Consolidated contender builds`:
- single build, all scenarios → PRESERVED invariant (rust-camel-lib path
  untouched; existing suite guards)
- reference contender builds standalone → 4.1 step 1 (build-all.sh) +
  2.1 fixture-local target pin acceptance
- smoke parity after the move → 4.1: no-arg smoke path still iterates the
  full artifact set (now including axum-bare); pre-existing emitters
  unchanged; axum-bare validated by its own case
- dispatch does not perturb M1 / shared node runtime / completeness guard
  survives layout change → PRESERVED invariants (no dispatch/node/guard
  code touched; 3.1 greps prove FAMILY_COMPLETENESS + Pairs untouched)

benchmark-records MODIFIED `Fail-closed complete-record publish`:
- pre-reference records stay complete → 1.1 + 3.2 step 9 + 6.1 step 2
  (`--check` green + empty diff over published artifacts)
- complete record publishes clean / missing metric / wholly missing cell /
  n/a warm / unconverged / probe timeout / measured wins / conflicting /
  malformed / status schema additive → PRESERVED invariants: publisher
  logic untouched; the extended record-fixture test (3.2 step 5) keeps the
  53-cell synthetic record publishing clean, proving the gate logic end to
  end on the new arithmetic
