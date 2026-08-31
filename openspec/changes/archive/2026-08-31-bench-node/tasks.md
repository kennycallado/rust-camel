# Tasks: bench-node

## Phase 1: Runtime + seam (prove on http-server)

### Task 1.1: Pinned Node runtime in runner

**Files:**
- `benchmarks/runner/pin.sh` (modified)
- `benchmarks/runner/Dockerfile` (modified)
- `benchmarks/runner/README.md` (modified)
- `benchmarks/harness/run.sh` (modified)

**Steps:**
1. In `pin.sh`, add `NODE_VERSION="22.14.0"` (22.x LTS line) and
   `NODE_SHA256` (the sha256 of `node-v22.14.0-linux-x64.tar.gz` from
   nodejs.org/dist — download once during implementation and record the
   literal hex) to the pin record. ADD a `--report` mode to pin.sh
   (today it has no flags): `bash pin.sh --report` prints every recorded
   pin without building. Extend pin.sh's `docker build` invocation with
   `--build-arg NODE_VERSION="$NODE_VERSION"
   --build-arg NODE_SHA256="$NODE_SHA256"`.
2. In `Dockerfile`, after the JVM layer, add a Node layer: download
   `https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.gz`,
   verify with `sha256sum -c` against `NODE_SHA256` (fail closed on
   mismatch), extract to `/opt/node`, and symlink
   `/usr/local/bin/node` + `/usr/local/bin/npm` into it. Use `.tar.gz`
   (not `.tar.xz` — xz-utils is absent from the image). Declare
   `ARG NODE_VERSION` / `ARG NODE_SHA256` with defaults equal to the
   pin.sh literals (comment in both files: pin.sh is the single source
   of truth; defaults exist so a bare `docker build` still works).
3. In `benchmarks/harness/run.sh`, add `NODE_BIN` resolution: prefer
   `/opt/node/bin/node`, fall back to `node` on PATH (host smoke),
   overridable via env `NODE_BIN`. Print the resolved `NODE_BIN` in the
   dry-run header line (so `node-bin-resolution` below has something to
   assert).
4. `runner/README.md` today is a single line — CREATE a digest-pin
   table in it with a Node row: `NODE_VERSION` / `NODE_SHA256` /
   artifact URL, noting the tarball is SHA256-verified at build
   (stronger than the JVM format-only check).

**Tests:**
- `pin-emits-node`: pin record exists → run `bash benchmarks/runner/pin.sh --report` (or equivalent report mode) → output contains `NODE_VERSION=22.14.0` and a 64-hex `NODE_SHA256`.
- `docker-build-verifies`: Dockerfile built with a WRONG `NODE_SHA256` build arg → build FAILS at the sha256sum check (fail closed), it never reaches extraction.
- `node-bin-resolution`: on a host without `/opt/node` → `run.sh` dry-run header shows `NODE_BIN` resolving to the PATH `node` (or `<missing:node>` if absent) without aborting the dry-run.

**Acceptance:**
- `docker build` with correct args succeeds and `docker run <img> node --version` prints `v22.14.0`.
- `rg -n "NODE_SHA256" benchmarks/runner/` shows the literal recorded in both pin.sh and README.
- No `COPY --from=node` anywhere: `rg -c "COPY --from=node" benchmarks/runner/Dockerfile` → 0.

- [x] 1.1

### Task 1.2: Node family wiring + selection-scoped completeness guard

**Files:**
- `benchmarks/harness/run.sh` (modified)

**Steps:**
1. Add `build_node_artifact(fixture_dir)`: if `package.json` exists in
   `fixture_dir`, run `npm ci --omit=dev` there (respecting
   `DRY_RUN`: print `<would-build:npm-ci>` and skip); if no
   `package.json`, it is a no-op (the committed script IS the artifact).
2. Add `assert_family_completeness(family)`: given the run's SELECTED
   scenario list, if the family has a fixture dir in ≥1 selected active
   scenario, require a fixture dir in ALL selected active scenarios, else
   abort with `error: contender family <family> declares completeness but is missing fixtures in: <scenario list>`. A fixture dir under an
   INACTIVE scenario (no `SCENARIO_MARKER` entry, e.g. `multi-step`)
   emits `warning: <scenario> is inactive; fixture not registered` and
   continues. The node family is registered as completeness-declaring
   (`NODE_FAMILY=completeness` style declaration next to the wiring).
3. Add the node family branch to the per-scenario cell wiring, placed
   BEFORE the `SCENARIO_ARTIFACT_SET` bridge dispatch `case` (bridge
   scenarios `continue` past the standard block at run.sh ~1427-1435, so
   wiring added after it never registers xsd/xslt node cells): for each
   of `node-native` and `node-fastify` fixture dirs present, call
   `build_node_artifact` then ONE `add_cell` call PER contender
   (`add_cell` takes a single contender name, run.sh:1585):
   `add_cell "$scenario" "node-native" "$NODE_BIN <fixture>/route.mjs" "$marker"`
   and likewise for `node-fastify` (env contract carried via the
   existing `BENCH_*` mechanism).
4. Update the single `expected_cells` assertion site (shared by the
   dry-run and measure paths, run.sh ~2873-2886; today `bridge +4 /
   full +6` arithmetic): it becomes
   `+2` per scenario that has node fixtures (bridge scenarios with node
   → `4+2=6`, full scenarios → `6+2=8`). Read the actual current
   expression first and extend it, do not replace it blindly.
5. Dry-run behavior keyed on `package.json` presence (per Task 1.1's
   build rule) — not on contender name.

**Tests:**
- `completeness-happy-path`: create stub dirs `scenarios/http-server/node-native/route.mjs` (echo the marker) and `scenarios/http-server/node-fastify/{package.json,route.mjs}` (throwaway package.json, any name) → `bash benchmarks/bench run --scenarios=http-server --dry-run` exits 0 listing both node cells → delete the stubs after asserting.
- `completeness-aborts`: temporarily rename `scenarios/<sel-scenario>/node-fastify` → dry-run EXITS NONZERO with the error naming `node-fastify` and the missing scenario; restore the dir.
- `inactive-fixture-warns`: create `scenarios/multi-step/node-native/route.mjs` stub → dry-run prints the inactive warning and exits 0; remove the stub (multi-step stays out of the change).
- `npm-dry-run-marker`: same stub dirs as above (fastify's throwaway `package.json`) → dry-run prints `<would-build:npm-ci>` for the fastify cell and no build marker for the native cell → delete stubs after asserting.

**Acceptance:**
- The four test scenarios above pass as described (documented in the task's worktree commit message with the exact commands run).
- `rg -n "assert_family_completeness" benchmarks/harness/run.sh` shows the guard invoked before any node `add_cell`.

- [x] 1.2

### Task 1.3: http-server fixtures (node-native + node-fastify) end-to-end

**Files:**
- `benchmarks/scenarios/http-server/node-native/route.mjs` (new)
- `benchmarks/scenarios/http-server/smoke/run.sh` (modified — node cell
  wiring added during implementation; legitimate extension recorded in
  archive notes)
- `benchmarks/scenarios/http-server/node-native/README.md` (new)
- `benchmarks/scenarios/http-server/node-fastify/route.mjs` (new)
- `benchmarks/scenarios/http-server/node-fastify/package.json` (new)
- `benchmarks/scenarios/http-server/node-fastify/package-lock.json` (new)
- `benchmarks/scenarios/http-server/node-fastify/README.md` (new)

**Steps:**
1. Read the scenario's existing contenders
   (`scenarios/http-server/camel-quarkus/` server fixture and the
   run.sh `http-server` cell wiring) to extract the exact protocol-A
   contract: bind URL/port, request→response semantics, marker line
   `BENCH_ROUTE_READY`, latency-file format, and how the loadgen client
   drives the cell.
2. `node-native/route.mjs`: zero-dependency Node http server implementing
   the SAME request→response semantics as the existing contenders,
   reading `BENCH_HTTP_URL` (bind), `BENCH_LATENCY_FILE`, and payload
   args from the scenario's env contract; prints the scenario marker
   when ready; writes the latency file in the same format the JVM
   contenders write.
3. `node-fastify/`: same route semantics behind Fastify
   (`package.json` pins fastify to an exact version;
   `package-lock.json` generated and committed); binds identically;
   same marker + latency contract.
4. Both READMEs: one paragraph — what the fixture is, how to run it
   standalone, and (for fastify) the exact fastify version.
5. Smoke on a Node host: run the scenario smoke for both fixtures;
   confirm marker emission, latency file creation, and response digests
   matching the existing contenders' for the same canonical requests.

**Tests:**
- `marker-emission-native`: start `node route.mjs` with the env contract → stdout contains `BENCH_ROUTE_READY` within 5s → server answers the smoke request.
- `marker-emission-fastify`: after `npm ci`, same as above through fastify.
- `digest-parity-http`: the http-server smoke drives `POST /bench body=ping` — capture the node fixtures' response body sha256 and ONE existing contender's response body sha256 for the same request on the same host → equal. Commit the node smoke logs as `scenarios/http-server/smoke/node-native.log` and `node-fastify.log`, matching the scenario's existing `<contender>.log` naming (this is the reference evidence for future re-verification).
- `syntax-check`: `node --check route.mjs` exits 0 for both fixtures.

**Acceptance:**
- All four tests above pass on a host with Node ≥20 and a rust
  toolchain (the parity reference is the `rust-camel-lib` http-server
  fixture — cheapest to build; JDK not required).
- `git status` shows no `node_modules/` (gitignore lands in Task 1.4; do not commit it regardless).
- Dry-run: native cell resolves with no build; fastify cell shows `<would-build:npm-ci>`.

- [x] 1.3

### Task 1.4: gitignore node_modules + CI dry-run confirmation

**Files:**
- `.gitignore` (modified)
- `.github/workflows/ci.yml` (modified, if the smoke step needs the node env vars surfaced)

**Steps:**
1. Add `benchmarks/scenarios/**/node_modules/` to `.gitignore`
   (lockfiles stay committed).
2. Run the node-covered dry-run:
   `bash benchmarks/bench run --scenarios=http-server --dry-run`
   → exits 0, http-server shows both node cells (selection fully
   covered). Then positive guard evidence: the FULL-suite dry-run
   (all 7 scenarios selected, node fixtures only in http-server) must
   ABORT naming `node-fastify`/`node-native` and the 6 missing
   scenarios — that abort IS the completeness rule working mid-change;
   record the error line in the task result. After Phase 2/3 land all
   fixtures, the full-suite dry-run goes green (asserted in Task 3.3).
3. If the CI workflow's smoke step needs `NODE_BIN` or node-related env
   surfaced, adjust the step; otherwise leave ci.yml untouched and note
   that in the task result.

**Tests:**
- `gitignore-effective`: create `benchmarks/scenarios/http-server/node-fastify/node_modules/x` → `git status --porcelain` does NOT list it; `git check-ignore -v` names the new rule.
- `ci-dry-run-parity`: the exact CI smoke command (`bench help` + dry-run + records `--check`) exits 0 in the worktree.

**Acceptance:**
- Both tests pass; CI yaml unchanged OR the diff is a one-line env addition explained in the task result.

- [x] 1.4

Task 1.4 evidence (recorded post-implementation): full-suite dry-run aborts as designed —
`error: contender family node declares completeness but is missing fixtures in: split-aggregate/node-native startup-minimal/node-native t2-json/node-native t2-realistic-eip/node-native xsd-validation-bridge/node-native xslt-bridge/node-native split-aggregate/node-fastify startup-minimal/node-fastify t2-json/node-fastify t2-realistic-eip/node-fastify xsd-validation-bridge/node-fastify xslt-bridge/node-fastify`
(12 missing = 6 scenarios × 2 — positive guard evidence mid-change; green full-suite asserted in 3.3). ci.yml UNTOUCHED (bench-smoke selects t2-json,split-aggregate — zero node fixtures → guard no-op → selection green, verified exit 0).

DEVIATION (task 1.1 acceptance): `docker build` was not executed on the implementation/review hosts — fail-closed wrong-SHA logic verified statically + host-side tarball checksum, real image build deferred (bd rc-f4po). VERIFIED 2026-08-31 on this host (docker 29.6.2): happy path `docker build -t bench-node-runner-test benchmarks/runner` exit 0 (ARG defaults = pin.sh literals; node layer passed the `sha256sum -c` gate — layer cache-warm from the earlier identical pin.sh build, and cache keys only exist for successfully completed layers) and `docker run --rm bench-node-runner-test node --version` → `v22.14.0` plus the build-time toolchain gate (java/cargo/mvn/gradle/node/time) passing inside the build; fail-closed `docker build --build-arg NODE_SHA256=0000000000000000000000000000000000000000000000000000000000000000` FAILED exit 1 at the node layer — `sha256sum: WARNING: 1 computed checksum did NOT match` / `/tmp/node.tar.gz: FAILED` — never reaching extraction (sha256sum ran live against the real download). pin.sh's full image build + DIGEST recording still happens at the first canonical container run (bd rc-f4po). NODE_BIN resolution chain duplicated harness↔smoke (8 lines, cross-referenced) — accepted, noted here.

## Phase 2: stdlib scenarios (protocol-B, zero-dependency)

NOTE (CI atomicity): the CI bench-smoke selects
`t2-json,split-aggregate`; tasks 2.2 and 2.3 are CI-atomic — land them
together (same phase-group commit window), never push between them,
or the completeness guard trips inside the CI selection.

### Task 2.1: startup-minimal (both contenders)

**Files:**
- `benchmarks/scenarios/startup-minimal/node-native/route.mjs` (new)
- `benchmarks/scenarios/startup-minimal/node-native/README.md` (new)
- `benchmarks/scenarios/startup-minimal/node-fastify/route.mjs` (new)
- `benchmarks/scenarios/startup-minimal/node-fastify/package.json` (new)
- `benchmarks/scenarios/startup-minimal/node-fastify/package-lock.json` (new)
- `benchmarks/scenarios/startup-minimal/node-fastify/README.md` (new)

**Steps:**
1. Read `scenarios/startup-minimal/` fixtures (all four existing
   contender groups) and its run.sh wiring: this scenario measures
   process start → route execution → marker (protocol B, process-spawn
   shape for every contender INCLUDING rust-camel-lib).
2. `node-native/route.mjs`: minimal script that performs the scenario's
   route semantics and prints `BENCH_ROUTE_READY` (this scenario's
   marker), honoring the latency-file contract if the existing
   contenders write one. Zero dependencies.
3. `node-fastify/route.mjs`: boots the Fastify application (require +
   `fastify()` + route registration) WITHOUT binding a listener —
   module+init cost is the measured framework tax — then performs the
   same route semantics and prints the marker.
4. READMEs as in Task 1.3 step 4.

**Tests:**
- `startup-marker-native`: `time node route.mjs` (with env contract) → exits 0, marker on stdout.
- `startup-marker-fastify`: after `npm ci`, same.
- `no-listener-fastify`: `rg -n "listen" node-fastify/route.mjs` → 0 matches (the no-bind rule for protocol-B).
- `syntax-check`: `node --check` both fixtures.

**Acceptance:**
- All tests pass; smoke digests parity where the scenario defines a canonical output (if it has none — pure startup — the marker timing IS the output; note that in the result).

- [x] 2.1

### Task 2.2: t2-json (both contenders)

**Files:**
- `benchmarks/scenarios/t2-json/node-native/route.mjs` (new)
- `benchmarks/scenarios/t2-json/node-native/README.md` (new)
- `benchmarks/scenarios/t2-json/node-fastify/route.mjs` (new)
- `benchmarks/scenarios/t2-json/node-fastify/package.json` (new)
- `benchmarks/scenarios/t2-json/node-fastify/package-lock.json` (new)
- `benchmarks/scenarios/t2-json/node-fastify/README.md` (new)

**Steps:**
1. Read the scenario's shared canonical payload contract
   (`shared/bench-payload` generation via the harness loadgen, the
   marker `BENCH_ROUTE_READY bytes=$((BENCH_PAYLOAD_BYTES + 13))`, and
   the JVM fixture route: set_body → unmarshal → filter → transform →
   marshal) plus its `smoke/run.sh`.
2. `node-native/route.mjs`: reads the payload from the env contract,
   performs the SAME transform semantics (parse → filter by the
   scenario's predicate → transform by the scenario's mapping →
   serialize), and emits the scenario's exact marker with the same
   bytes formula. Zero dependencies — `JSON.parse`/`JSON.stringify`.
3. `node-fastify/route.mjs`: boots Fastify without binding, then the
   same route semantics and marker.
4. Digest parity (INPUT, not output — cross-runtime serialized output
   bytes may differ in field order, the documented caveat): each node
   fixture logs `BENCH_INPUT_SHA256` equal to the scenario's committed
   smoke golden for the canonical 32768-byte payload, and emits the
   exact marker formula. Commit the node smoke logs per the test block
   below.

**Tests:**
- `t2-marker-formula`: with `BENCH_PAYLOAD_BYTES=32768` → marker is exactly `BENCH_ROUTE_READY bytes=32781`.
- `t2-input-digest-parity`: parity in this suite is INPUT parity — run both node fixtures on the canonical 32768-byte payload → each logs `BENCH_INPUT_SHA256` EQUAL to the committed golden `a0db…` family value in `scenarios/t2-json/smoke/*.log` (byte-identical canonical input; cross-runtime OUTPUT byte-parity is NOT asserted — serializers differ). Also commit `scenarios/t2-json/smoke/node-native-32768.log` + `node-fastify-32768.log` following the existing per-contender convention.
- `no-listener-fastify` and `syntax-check`: as in Task 2.1.

**Acceptance:**
- All tests pass; both fixtures zero-dependency (no package.json for native).

- [x] 2.2

Task 2.2 evidence (recorded post-implementation): both fixtures log
`BENCH_INPUT_SHA256=a0db69e1146a29b0b25ca22435e51f39e271ecb1ac4ec1cee0ead3212eae10e9`
(golden 32768/tick-0) and emit exactly `BENCH_ROUTE_READY bytes=32781`;
fastify boots with `await app.ready()` pre-marker, binds nothing
(`rg listen` = 0). Scenario smoke extended with node legs 5
(SKIP-guarded on node binary / node_modules): full run 6 pass / 0 fail
(rebuilt both rust artifacts in the worktree; JVM legs skipped, no java);
committed `smoke/node-native-32768.log` + `node-fastify-32768.log`.
`t2-json` dry-run: exit 0, both node cells listed, expected==resolved
(7 with the CLI release binary absent, 8 after the smoke built it —
consistency asserted, not the literal count).

### Task 2.3: split-aggregate (both contenders)

**Files:**
- `benchmarks/scenarios/split-aggregate/node-native/route.mjs` (new)
- `benchmarks/scenarios/split-aggregate/node-native/README.md` (new)
- `benchmarks/scenarios/split-aggregate/node-fastify/route.mjs` (new)
- `benchmarks/scenarios/split-aggregate/node-fastify/package.json` (new)
- `benchmarks/scenarios/split-aggregate/node-fastify/package-lock.json` (new)
- `benchmarks/scenarios/split-aggregate/node-fastify/README.md` (new)

**Steps:**
1. Read the scenario contract: timer → canonical 100-item array
   (loadgen split-aggregate payload) → split (sequential) →
   `direct:agg-in` → aggregate (`completion_size=100`) → marker
   `BENCH_ROUTE_READY items=100`; plus `smoke/run.sh`.
2. `node-native/route.mjs`: reads the canonical array payload, splits
   it into the 100 items, processes each item with the scenario's
   per-item semantics, aggregates the completion (`completion_size=100`),
   emits the marker. Zero dependencies, hand-rolled async coordination
   is the honest point of this contender — do NOT reach for an
   orchestration library.
3. `node-fastify/route.mjs`: boots Fastify without binding, same
   semantics and marker.
4. Digest parity as in Task 2.2 step 4: INPUT parity — the logged
   `BENCH_INPUT_SHA256` equals the scenario's committed smoke golden;
   aggregate OUTPUT byte-parity across runtimes is not asserted.

**Tests:**
- `split-marker`: marker is exactly `BENCH_ROUTE_READY items=100`.
- `split-input-digest-parity`: both node fixtures log `BENCH_INPUT_SHA256` equal to the scenario's committed smoke golden for the canonical payload; commit the node smoke logs under `scenarios/split-aggregate/smoke/` following the existing convention.
- `no-listener-fastify` and `syntax-check`: as in Task 2.1.

**Acceptance:**
- All tests pass; native fixture has no package.json.

- [x] 2.3

Task 2.3 evidence (recorded post-implementation): both fixtures log
`BENCH_INPUT_SHA256=123444b475c48473309ed966eb69896c6725429021a5a5d2e0eaa0a77a159316`
(the fixed-array golden, tick-independent) and emit exactly
`BENCH_ROUTE_READY items=100` once; fastify boots with
`await app.ready()` pre-marker, binds nothing (`rg listen` = 0).
NOTE: this scenario has NO env contract — the canonical array is fixed
and `BENCH_PAYLOAD_BYTES` is ignored exactly as the rust fixture
ignores it — so the strict-parse rejection test has nothing to assert
and was skipped (per task-block fallback). Scenario smoke extended
with node legs 3 (SKIP-guarded on node binary / node_modules;
size-suffix-free log names per scenario convention): full run 4 pass /
0 fail (lib fixture built debug in the worktree; the live rust pair
cross-verified the digest against the node legs). Committed
`smoke/node-native.log` + `node-fastify.log`. Dry-runs:
`--scenarios=split-aggregate` exit 0, 8/8 cells (both node cells
listed); CI pair `--scenarios=t2-json,split-aggregate` exit 0, 16/16
cells — the CI-atomic selection is green.

### Task 2.4: t2-realistic-eip (both contenders)

**Files:**
- `benchmarks/scenarios/t2-realistic-eip/node-native/route.mjs` (new)
- `benchmarks/scenarios/t2-realistic-eip/node-native/README.md` (new)
- `benchmarks/scenarios/t2-realistic-eip/node-fastify/route.mjs` (new)
- `benchmarks/scenarios/t2-realistic-eip/node-fastify/package.json` (new)
- `benchmarks/scenarios/t2-realistic-eip/node-fastify/package-lock.json` (new)
- `benchmarks/scenarios/t2-realistic-eip/node-fastify/README.md` (new)

**Steps:**
1. Read `scenarios/t2-realistic-eip/` existing fixture sources + run.sh
   wiring as the source of truth (the scenario has NO README and NO
   smoke dir — do not reference either): the rust fixtures
   (`rust-camel-lib/src/main.rs` — note it sets its own body, no
   `BENCH_INPUT_SHA256` logging in this scenario) define the chain:
   choice/when branch semantics over the fixture's own body, ending in
   the marker `BENCH_ROUTE_READY body=pong-bench` (the scenario's
   branch-execution proof). Mirror the chain step-by-step from the
   fixture sources and the run.sh cell wiring.
2. Implement the same chain in `node-native/route.mjs` (zero deps) and
   `node-fastify/route.mjs` (boots app, no listener), emitting the
   scenario's exact marker `BENCH_ROUTE_READY body=pong-bench`.
3. Parity evidence: this scenario has no committed golden — record the
   node fixtures' behavior (marker line, run command, observed output)
   in each fixture's README, and assert semantic parity with the
   existing fixtures' chain (same choice/when outcomes, same final
   body) rather than digest equality.

**Tests:**
- `eip-marker-native` / `eip-marker-fastify`: marker matches the scenario's registered `SCENARIO_MARKER` exactly.
- `eip-semantic-parity`: node fixture output equals the existing
  `rust-camel-lib` fixture chain's choice/when outcomes and final body
  (same `pong-bench` final body, same branch decisions per the fixture
  sources read in step 1); evidence recorded in the fixture READMEs.
- `no-listener-fastify` and `syntax-check`: as before.

**Acceptance:**
- All tests pass on a Node host after `npm ci` in both fixture dirs.

- [x] 2.4

Task 2.4 evidence (recorded post-implementation): both fixtures emit
exactly `BENCH_ROUTE_READY body=pong-bench` once and no `pong-other`;
semantic parity asserted against the LIVE rust reference
(`cargo build -p t2-realistic-eip-rust-camel-lib` then
`timeout 8 ./target/debug/t2-realistic-eip` → marker once, when
branch taken, final body `pong-bench`, then idles) — same choice/when
outcomes, same final body; no golden digest exists in this scenario
(the fixture sets its own body — no BENCH_INPUT_SHA256, no env
contract, BENCH_PAYLOAD_BYTES ignored as in the rust fixture).
Divergence documented in both READMEs: the rust reference's extra
static `BENCH_ROUTE_READY exchange_id=…` line is not mirrored (4 of 5
fixtures — every one except rust-camel-lib — emit only the dynamic log
line); the choice nests inside
the filter scope (YAML/Java shape) — rust-lib's empty `end_filter()`
before `.choice()` is a builder artifact, observationally identical
under an always-true filter. Fastify boots `await app.ready()`
(route.mjs:34) pre-marker (:58), binds nothing (`rg listen` = 0);
clean `npm ci --omit=dev` verified. Tests: syntax-check both,
marker native/fastify (exact, once), no-listener-fastify,
ready-before-marker, eip-semantic-parity vs live rust,
harness-dry-run exit 0 with 8/8 cells (both node cells listed).

### Task 3.1: xsd-validation-bridge (both contenders, xmllint-wasm)

**Files:**
- `benchmarks/scenarios/xsd-validation-bridge/node-native/route.mjs` (new)
- `benchmarks/scenarios/xsd-validation-bridge/node-native/package.json` (new)
- `benchmarks/scenarios/xsd-validation-bridge/node-native/package-lock.json` (new)
- `benchmarks/scenarios/xsd-validation-bridge/node-native/README.md` (new)
- `benchmarks/scenarios/xsd-validation-bridge/node-fastify/route.mjs` (new)
- `benchmarks/scenarios/xsd-validation-bridge/node-fastify/package.json` (new)
- `benchmarks/scenarios/xsd-validation-bridge/node-fastify/package-lock.json` (new)
- `benchmarks/scenarios/xsd-validation-bridge/node-fastify/README.md` (new)

**Steps:**
1. Read the scenario contract: shared `bench-payload.xml` +
   `schema.xsd` (byte-pinned assets under `shared/`), validation
   semantics, marker, and smoke evidence. Note: this scenario's
   existing contenders go through the compiled `xml-bridge` subprocess
   — node fixtures are EXEMPT from that seam: validate IN-PROCESS using
   `xmllint-wasm`, reusing ONLY the shared assets (digest parity by
   construction).
2. `node-native`: `package.json` pins `xmllint-wasm` exact version
   (first node-native dependency — Node stdlib has no XML); route
   loads the shared schema + payload from the env contract, validates
   in-process, emits the scenario marker.
3. `node-fastify`: same validation route behind a booted-not-listening
   Fastify app; `package.json` pins fastify + xmllint-wasm.
4. READMEs MUST name the engine and its JVM counterpart for
   auditability: `xmllint-wasm` (libxml2 compiled to wasm; chosen for
   buildability — no node-gyp — with wasm overhead as a documented
   caveat) vs JDK Xerces on the JVM side.

**Tests:**
- `xsd-valid-passes`: canonical payload → exit 0, scenario marker on stdout.
- `xsd-invalid-rejected`: mutate the payload to violate the schema (e.g., remove a required element via a temp copy) → fixture exits nonzero with a validation error; restore.
- `xsd-digest-parity`: reads the SAME `shared/bench-payload.xml` the JVM contenders read (byte-pinned) — assert the fixture references only shared assets (`rg -n "shared/" route.mjs` shows the asset paths; no fixture-local XML copies: `ls *.xml *.xsd` in fixture dir → empty).
- `no-listener-fastify` and `syntax-check`: as before.

**Acceptance:**
- All tests pass on a Node host after `npm ci` in both fixture dirs.

- [x] 3.1

Task 3.1 evidence (recorded post-implementation): xmllint-wasm
pinned exact at 5.3.0 (latest; lockfiles committed, clean
`npm ci --omit=dev` in both dirs, node_modules untracked via the
task 1.4 gitignore rule). Both fixtures emit exactly one
`BENCH_ROUTE_READY <unix_ms>` then per-tick
`BENCH_LATENCY <id> <ns>` + `BENCH_XSD_TICK id=<n>` (~42ms/tick —
xmllint-wasm v5.3.0 spawns a fresh worker per validateXML call, so
per-tick worker spin-up + schema parse is the documented wasm
overhead caveat; the one-time module compile sits in the startup
self-test pre-marker, the counterpart of the JVM's once-per-process
Xerces schema compile). Asset defaults anchor via import.meta.url
(`../shared/...`) — the harness launches node cells with no per-cell
env and no cd, so CWD-relative defaults would not resolve; latency
defaults are the harness protocol-B probe paths
`/tmp/v3-protocol-b-xsd-validation-bridge_node-{native,fastify}.log`.
Tests: xsd-valid-passes both (marker=1, ticks/records ≥10),
xsd-invalid-rejected both (required `<meta>` removed in a temp copy
→ exit 1 BEFORE any marker with the libxml2 validity error),
xsd-digest-parity (2 `shared/` refs per route.mjs, zero local
XML/XSD copies), syntax-check both, no-listener-fastify (rg listen =
0), ready-before-marker (route.mjs:50 < :120), harness-dry-run exit
0 with both node cells listed and 2× `<would-build:npm-ci>` (needed
the gitignored `bridges/xml/build/native/xml-bridge` build output
copied from main — branch touches no bridges/ sources). Scenario
smoke extended with SKIP-guarded node legs (node binary + node_modules
guards, absolute-path argv so pkill cleanup matches; log names
`smoke/node-native.log` + `smoke/node-fastify.log` committed):
full run 3 pass / 0 fail — including the LIVE rust-camel-cli cell
(release binary + bridge binary present), which validated the same
shared payload + schema in the same run: cross-runtime validation
parity evidence.

### Task 3.2: xslt-bridge (both contenders, saxon-js)

**Files:**
- `benchmarks/scenarios/xslt-bridge/node-native/route.mjs` (new)
- `benchmarks/scenarios/xslt-bridge/node-native/package.json` (new)
- `benchmarks/scenarios/xslt-bridge/node-native/package-lock.json` (new)
- `benchmarks/scenarios/xslt-bridge/node-native/README.md` (new)
- `benchmarks/scenarios/xslt-bridge/node-fastify/route.mjs` (new)
- `benchmarks/scenarios/xslt-bridge/node-fastify/package.json` (new)
- `benchmarks/scenarios/xslt-bridge/node-fastify/package-lock.json` (new)
- `benchmarks/scenarios/xslt-bridge/node-fastify/README.md` (new)

**Steps:**
1. Read the scenario contract: shared `bench-payload.xml` +
   `identity-transform.xsl`, marker, smoke evidence; in-process
   exemption from the compiled-bridge seam as in Task 3.1.
2. `node-native`: `saxon-js` pinned exact; transform the shared payload
   with the shared stylesheet in-process; emit marker. The transform
   output must be byte-comparable with the JVM contenders' output
   modulo XML serialization differences — if byte-parity is impossible
   (different serializers), the README documents the divergence and
   the digest comparison uses the fixture's own stable output vs its
   smoke evidence.
3. `node-fastify`: same transform behind booted-not-listening app.
4. READMEs: engine auditability — `saxon-js` (Saxon-JS: same vendor as
   JVM Saxon but a DIFFERENT engine — Saxon-JS ≠ Saxon-HE) vs the JVM
   side's engine.

**Tests:**
- `xslt-output-stable`: canonical payload → transform output sha256 equals the fixture's smoke-recorded digest (stability, run twice).
- `xslt-marker-native` / `xslt-marker-fastify`: scenario marker exact.
- `xslt-shared-assets-only`: as in Task 3.1 (`shared/` references, no local XML/XSL copies).
- `no-listener-fastify` and `syntax-check`: as before.

**Acceptance:**
- All tests pass; both READMEs carry the engine note.

- [x] 3.2

Task 3.2 evidence (recorded post-implementation): saxon-js pinned
exact at 2.7.0 (latest; XSLT 3.0 engine — the shared stylesheet
declares `version="3.0"`, so no 1.0-vs-3.0 compatibility concern) +
xslt3 2.7.0 (same-vendor compiler CLI; the saxon-js runtime executes
only compiled SEF stylesheets, so the fixture compiles the shared
stylesheet to a throwaway temp SEF at startup — the engine init slot,
counterpart of the JVM's once-per-process Templates compile — and
transforms per tick via `stylesheetInternal`, no recompile; lockfiles
committed, clean `npm ci --omit=dev` in both dirs, node_modules
untracked). Both fixtures emit exactly one
`BENCH_XSLT_SELFTEST_SHA256=17713b3d54921b7d3c1420252685e94eca4689781258268e6c948ae5ae6742d9`
(identical across cells and across repeated runs — stability, not
cross-runtime parity: Saxon-JS ≠ Saxon-HE serializers, divergence
documented in both READMEs), one `BENCH_ROUTE_READY <unix_ms>`, then
per-tick `BENCH_LATENCY <id> <ns>` + `BENCH_XSLT_TICK id=<n>`
(~1-2ms transform per 10ms tick). Asset defaults anchor via
import.meta.url; latency defaults are the harness protocol-B probe
paths `/tmp/v3-protocol-b-xslt-bridge_node-{native,fastify}.log`.
EXTRA HARNESS STEP (from task 3.1 review): run.sh node-cell wiring
now injects `BENCH_LATENCY_FILE=/tmp/v3-protocol-b-<scenario>_<cell>.log`
into node cell argv via an `env` prefix (GNU time argv re-parse, same
trick as the rust-camel-lib cell) — the fixtures' hardcoded defaults
are now standalone-run fallbacks only; verified by `bash -x` trace of
both dry-runs showing the injected argv. Tests: syntax-check both,
xslt-marker-native/fastify (exact, once), xslt-ticks (records land in
the env-specified latency file), xslt-output-stable (2 runs × 2
fixtures, same digest, matching committed smoke evidence),
xslt-shared-assets-only (2 `shared/` refs per route.mjs, zero local
XML/XSL copies), no-listener-fastify (`rg listen` = 0),
ready-before-marker (route.mjs:64 < :139), harness-dry-run exit 0
with both node cells listed + 2× `<would-build:npm-ci>` (xsd dry-run
regression also green). Scenario smoke extended with SKIP-guarded
node legs (node binary + node_modules guards; log names
`smoke/node-native.log` + `smoke/node-fastify.log` committed): full
run 3 pass / 0 fail — including the LIVE rust-camel-cli cell (release
binary + bridge binary present), which transformed the same shared
payload + stylesheet in the same run: cross-runtime parity evidence
at the semantics level (same engine input bytes; output identity not
asserted per the serializer divergence).

### Task 3.3: COVERAGE node axis + recipes completeness rule + validation

**Files:**
- `benchmarks/scenarios/COVERAGE.md` (modified)
- `benchmarks/scenarios/README.md` (modified)
- `benchmarks/README.md` (modified, only if the contender list line needs it)

**Steps:**
1. COVERAGE.md: add a "Node contender axis" table — 7 scenario rows ×
   2 contender columns — every cell
   `? open-if: first published container-hosted run` (NO measurement
   claimed), with a one-line legend pointing at bd rc-f4po for the
   first container run.
2. `scenarios/README.md`:
   a. Replace the stale "standard set is six contenders" sentence with
      the completeness-declaring family rule (the node family declares
      completeness across all active scenarios; families that do not
      declare remain per-scenario opt-in, with the YAML artifact-set
      pair as the documented existing exemption).
   b. Define "contender family" where the rule lands (a runtime family
      registered as a unit in the harness wiring — e.g., the node
      family = node-native + node-fastify).
   c. In the "Adding a new contender" recipe, add the completeness
      rule step: declare completeness OR cite the documented exemption;
      partial registration aborts.
3. `benchmarks/README.md`: only if it enumerates contenders (check the
   vocabulary section) — keep the 5-word diet; verify with the diet
   test below.
4. Final validation sweep in the worktree: full-suite dry-run (7
   scenarios × 2 node cells each = 14 node cells listed), CI smoke
   command, zone contract (`ls -A benchmarks/` = 7 entries +
   `.gitignore`), README diet test.

**Tests:**
- `coverage-14-open-if`: the table body (7 rows × 2 contender cells) contains exactly 14 `open-if` occurrences — count with `sed -n '/Node contender axis/,/^## /p' benchmarks/scenarios/COVERAGE.md | rg -o '\? open-if' | wc -l` = 14 (range runs to the next `## ` heading so a blank line after the heading cannot truncate it), and keep any legend line free of the literal `open-if` (write "unmeasured until first container run" instead).
- `readme-diet`: `rg -n "M[1-4]\b|T[1-4][a-z]?|v[0-9]|paired|bootstrap|payload axis" benchmarks/README.md` → 0 hits.
- `full-dry-run-14`: `bash benchmarks/bench run --dry-run` (all scenarios) → exactly 14 node cell lines.
- `zone-contract`: `ls -A benchmarks/ | grep -v '^\.gitignore$' | LC_ALL=C sort` prints exactly `README.md attic bench harness records runner scenarios` (7 lines).

**Acceptance:**
- All four tests pass; `openspec validate bench-node --type change --json` valid; tasks.md all boxes ticked.

- [x] 3.3

Task 3.3 evidence (recorded post-implementation): COVERAGE.md gains
the Node contender axis — 7 scenario rows × node-native/node-fastify,
all 14 cells `? open-if: first published container-hosted run`; the
section body counts exactly 14 `? open-if` occurrences and the legend
carries no `open-if` literal ("unmeasured until first container run",
bd rc-f4po). scenarios/README.md replaces the stale six-contender
sentence with the family rule (contender-family definition +
completeness-or-document-exemption, `FAMILY_COMPLETENESS` /
`SCENARIO_ARTIFACT_SET` named) and the contender recipe gains the
completeness step (partial registration aborts via the harness
guard); the two remaining stale "six" counts in the same README were
generalized. benchmarks/README.md untouched: its vocabulary section
lists public words only, no contender enumeration. pin.sh drift guard
(extra folded from the task 1.1 review): the non-report path greps
`ARG NODE_VERSION=`/`ARG NODE_SHA256=` defaults against the pin.sh
literals, `error:` + exit 1 on drift or missing ARG — RED proof:
sed the Dockerfile default to 99.9.9 → exit 1 before any docker
call; restored → green (`--report` unaffected, still prints both
pins). Sweep: explicit-selection dry-run exit 0 with exactly 14 node
cell lines and 52/52 resolved==expected — exact command: `bash
benchmarks/bench run
--scenarios=http-server,startup-minimal,t2-json,t2-realistic-eip,split-aggregate,xsd-validation-bridge,xslt-bridge
--dry-run` (the literal `full-dry-run-14` bare command `bench run
--dry-run` cannot pass: pre-existing auto-discovery aborts on the
inactive multi-step dir — bd rc-dh7t, out of scope); bare
auto-discovery dry-run remains broken pre-existing (bd rc-dh7t). CI
smoke (bench help; --scenarios=t2-json,split-aggregate --dry-run,
16/16 cells; summarize.py --check) all exit 0; zone contract exactly
`README.md attic bench harness records runner scenarios`; README diet
0 hits; `openspec validate bench-node --type change --json` valid.
