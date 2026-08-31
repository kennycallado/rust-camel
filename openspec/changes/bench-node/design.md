# Design: bench-node

## Approach

Three moving parts, in dependency order:

1. **Node runtime (runner).** No base-image swap and no `COPY --from` glibc
   gamble: the Dockerfile installs Node from the official
   `nodejs.org/dist` tarball with its SHA256 pinned. `pin.sh` learns two
   new entries (`NODE_VERSION`, `NODE_SHA256`) and verifies the tarball
   with `sha256sum -c` (new, stronger machinery than today's JVM
   format-only check). The artifact form is `.tar.gz` (`.tar.xz` would
   need xz-utils, absent from the image). Rationale: the
   runner base is `temurin:21-jdk-jammy`; official node container images
   are bullseye/bookworm-based and copying their binary risks GLIBCXX
   drift, while the dist tarball is self-contained and hash-verifiable.
   `run.sh` resolves `NODE_BIN` from the pinned install dir (env
   override for host smoke).

2. **Fixtures (per scenario × contender).** Standard layout:
   `scenarios/<s>/node-native/` and `scenarios/<s>/node-fastify/`.
   - `node-native`: a single `route.mjs`, zero dependencies for the 5
     stdlib-capable scenarios. For `xsd-validation-bridge` and
     `xslt-bridge` the stdlib has NO XML capability (owner-approved
     2026-08-30): XSLT via `saxon-js` (same vendor as JVM Saxon but a DIFFERENT engine —
     Saxon-JS ≠ Saxon-HE), XSD via `xmllint-wasm` (libxml2 in wasm;
     chosen for buildability — no node-gyp — with wasm overhead as a
     documented caveat; the JVM side uses JDK Xerces). Each XML fixture README names
     the engine so the cross-engine comparison is auditable. Node XML
     fixtures run in-process and are exempt from the compiled-bridge
     subprocess/wrapper/PID contract (they reuse only the shared
     bench-payload/.xsl/.xsd assets — digest parity by construction).
     Mirrors Java stdlib and Rust crates supplying XML to existing
     contenders.
   - `node-fastify`: same route logic behind the Fastify server layer;
     `package.json` + `package-lock.json` committed; built with
     `npm ci --omit=dev` (frozen lockfile, no floating resolution).
     In the six protocol-B scenarios the fixture boots the Fastify app
     WITHOUT binding a listener (module+init cost is the measured
     framework tax); only `http-server` binds per its URL contract.
   - Route contract identical to existing contenders: consume the
     scenario's shared canonical payload, execute the scenario's EIP
     semantics, emit the scenario's marker line, honor the
     latency-file and per-scenario protocol (A/B) exactly as the JVM
     fixtures in the same scenario do. Config via `BENCH_*` env vars.

3. **Harness wiring (completeness-enforced).** `harness/run.sh` gains a
   `build_node_artifact` (no-op without `package.json`, `npm ci` with
   one) and a node family branch in the per-scenario cell wiring that
   emits `add_cell` for both contenders. The wiring enforces the
   **contender completeness rule**, selection-scoped: before registering
   the family it checks a fixture dir exists for every active scenario
   in the run's SELECTION; a family with ≥1 fixture among the selected
   scenarios that misses any other selected scenario aborts the run
   with the explicit missing list (so partial selections like the CI
   gate `--scenarios=t2-json,split-aggregate` work mid-change, while
   canonical all-7 runs get full completeness). The reverse (fixture for an
   inactive scenario such as `multi-step`) is a warning, not an error.
   Dry-run: `node-native` resolves without building (script is the
   artifact); `node-fastify` reports `<would-build:npm-ci>` like the
   native runners do.

## Affected crates

None. Suite-only change: `benchmarks/runner/{Dockerfile,pin.sh}`,
`benchmarks/harness/run.sh`, `benchmarks/scenarios/**` (14 new fixture
dirs), `benchmarks/scenarios/COVERAGE.md` (adds a "Node contender axis" table,
7 scenarios × 2 contenders, every cell `? open-if: first published
container-hosted run` — the matrix is scenario×metric so node cells get
their own axis table rather than rows), `benchmarks/scenarios/README.md`,
`.github/workflows/ci.yml` (smoke picks up node cells via dry-run, no new
step). Root `Cargo.toml` untouched (no cargo fixtures). Records and schema
untouched — node cells only enter `records/` when a human-invoked run
publishes them (bd rc-f4po).

## Architecture boundaries

Data/control plane untouched: nothing crosses into camel runtime crates.
The measurement contract (marker line, latency file, shared payload,
digest parity) is reused verbatim — Node contenders are new probe tips on
an unchanged instrument, which is what keeps cross-contender ratios
meaningful. Zone contract intact (7 entries under `benchmarks/`); the
level-1 README diet is unaffected (guides live in `scenarios/README.md`).

## Phases

### Phase 1: runtime + seam (prove it on one scenario)
- **Goal:** pinned Node in the runner, node family wiring + completeness
  guard, and `node-native` + `node-fastify` fully working end-to-end in
  `http-server` (owner's honest-loss scenario) with smoke digests matching
  the existing contenders.
- **Dependencies:** none. Exits with the seam contract frozen.

### Phase 2: stdlib scenarios
- **Goal:** both contenders in `startup-minimal`, `t2-json`,
  `split-aggregate`, `t2-realistic-eip` — the zero-dependency fixtures.
- **Dependencies:** Phase 1 seam.

### Phase 3: XML scenarios + paperwork
- **Goal:** `xsd-validation-bridge` + `xslt-bridge` (first node-native dependency
  exercise; the npm-ci path itself is proven in Phase 1 by
  node-fastify/http-server), saxon-js/xmllint-wasm engine notes, COVERAGE rows
  (14 cells, all `open-if`), completeness rule added to the contender
  recipe, CI dry-run confirmation, full validation.
- **Dependencies:** Phase 2 (build path needs the seam's build order).

Risk notes: Node winning `http-server` is expected and is the point; fastify
supply chain is lockfile-frozen; process-spawn startup cost is measured
honestly (same shape as JVM contenders); saxon-js fairness documented
per-fixture.
