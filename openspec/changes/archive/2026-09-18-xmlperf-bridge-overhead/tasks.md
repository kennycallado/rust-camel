# Tasks: xmlperf-bridge-overhead

Phases per design.md `## Phases`. Phase 1 reproduces and attributes;
Phase 2 fixes or rules per the design's decision tree. Phase-2 branch
selection is governed by the row recorded in `bench-evidence.md` after
Task 1.3; every branch below is fully specified — the worker executes
exactly one.

## Phase 1: Reproduce + attribute

### Task 1.1 — Bridge binary resolution helpers + four-phase decomposition bench

Files:
- `crates/camel-bench/src/lib.rs` (modified) — add `find_workspace_root_from_path`,
  `resolve_bridge_binary_from`, `resolve_bridge_binary`,
  `xmlperf_evidence_run` (signatures below)
- `crates/camel-bench/build.rs` (new)
- `crates/camel-bench/Cargo.toml` (modified)
- `crates/camel-bench/benches/xml_bridge_decompose.rs` (new)

Steps:
1. `Cargo.toml`: add dev-dependencies `camel-bridge.workspace = true`,
   `tonic.workspace = true`, `prost.workspace = true`,
   `rustls.workspace = true`, `tempfile.workspace = true`,
   `tracing-subscriber.workspace = true`; add
   build-dependencies `tonic-prost-build.workspace = true`,
   `protoc-bin-vendored.workspace = true`; add `[[bench]] name =
   "xml_bridge_decompose" harness = false`.
2. `build.rs`: set `PROTOC` from `protoc_bin_vendored::protoc_bin_path()`,
   then `tonic_prost_build::configure().compile_protos()` on
   `../../bridges/xml/src/main/proto/xml_bridge.proto` with include dir
   `../../bridges/xml/src/main/proto` (camel-bench sits two levels
   below the workspace root); emit `cargo:rerun-if-changed` for that
   proto.
3. `lib.rs` — add:
   - `pub fn find_workspace_root_from_path(start: &Path) -> Option<PathBuf>`
     — walk up max 10 hops to the dir containing both a
     `[workspace]`-bearing `Cargo.toml` and a `bridges/` dir (same walk
     as `crates/services/camel-bridge/src/download.rs`);
   - `pub fn resolve_bridge_binary_from(start: &Path) -> Result<PathBuf, String>`
     — if env `CAMEL_XML_BRIDGE_BINARY_PATH` is set and the path
     exists, return it; else resolve the root via
     `find_workspace_root_from_path(start)` and return
     `{root}/bridges/xml/build/native/xml-bridge` if it exists; else
     `Err` whose message contains both `CAMEL_XML_BRIDGE_BINARY_PATH`
     and the default path string;
   - `pub fn resolve_bridge_binary() -> Result<PathBuf, String>` —
     delegates to `resolve_bridge_binary_from(Path::new(env!("CARGO_MANIFEST_DIR")))`;
   - `pub fn xmlperf_evidence_run() -> bool` — true iff env
     `XMLPERF_EVIDENCE_RUN` is set and non-empty.
4. `benches/xml_bridge_decompose.rs` — structure mirrors
   `direct_decompose.rs` exactly: `fn bench_xml_bridge(c: &mut
   Criterion)` owns the tokio runtime, bridge process, channel, and
   compiled stylesheet id as its locals; inside it, four
   `c.bench_function` calls each using `b.to_async(&rt)`; after the
   last bench, `process.stop().await` runs inside the same function
   (ownership moves); `criterion_group!(benches,
   bench_xml_bridge); criterion_main!(benches);`. Concretely:
   1. In `bench_xml_bridge`: print `/proc/loadavg` (or
      "loadavg unavailable" off-Linux). Call `resolve_bridge_binary()`.
      On `Err`: if `xmlperf_evidence_run()` — `eprintln!` the error,
      `std::process::exit(2)`; else print one skip line naming the
      missing path and the env override, and return without registering
      benches (early return before any `c.bench_function`).
   2. Build `rt = tokio::runtime::Builder::new_multi_thread()
      .worker_threads(2).build()`; install
      `rustls::crypto::ring::default_provider()` (discard result —
      idempotent, same pattern as camel-xslt tests). CONDITIONALLY
      install a tracing subscriber BEFORE any channel construction:
      `if std::env::var("RUST_LOG").is_ok() {
      tracing_subscriber::fmt()
      .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
      .with_writer(std::io::stderr).init(); }` — RUST_LOG is set ONLY
      in Task 2.2 Branch 3's diagnostic run (logging at trace level
      would pollute timed iterations); default evidence runs keep
      RUST_LOG unset → zero logging overhead. Without this init,
      RUST_LOG alone configures nothing (no subscriber in camel-bench
      or camel-bridge).
   3. `let (process, channel) = BridgeProcess::start_and_connect(
      &BridgeProcessConfig::xml(binary, 60_000)).await` — call inside
      `rt.block_on(...)` before benches register.
   4. Health readiness: issue `Health.Check` via the generated
      `health_client::HealthClient` until `status == "SERVING"` or 60 s
      deadline (reuse the wait loop shape from
      `crates/services/camel-bridge/src/health.rs`).
   5. Immediately after the spawn + health readiness steps, print one
      stdout line `XMLPERF_BRIDGE port=<port>` using
      `process.grpc_port()` (the only public accessor on
      `BridgeProcess` — child PID is private; the port is what Task 2.1
      Branch B and Task 2.3's socket proof need).
   6. Compile once: stylesheet bytes from
      `include_str!("../../../benchmarks/scenarios/xslt-bridge/shared/identity-transform.xsl")`
      (include paths resolve relative to `benches/`, three hops up to
      the workspace root); id = `XsltBridgeClient::stylesheet_id_for`
      algorithm (sha256 hex, `xslt-{hex}`) computed locally with the
      `sha2` crate (dev-dependency `sha2.workspace = true`); send
      `CompileStylesheet`; assert response has no `error`.
   7. Payload: `include_str!("../../../benchmarks/scenarios/xslt-bridge/shared/bench-payload.xml")`
      as `&'static str` (~1 KB).
   8. Warm: ≥200 iterations of health, dispatch-transform, and
      transform calls (inside `rt.block_on`).
   9. Register, with `b.to_async(&rt)`:
      - `xml_bridge/health_mtls` — `Health.Check` round-trip.
      - `xml_bridge/transform_dispatch_mtls` — `Transform` with the
        payload but stylesheet id `"xslt-unknown-id-bench"`; assert the
        response error kind is RESOURCE_NOT_FOUND each iteration.
      - `xml_bridge/transform_mtls` — `Transform` with the compiled id;
        assert `response.error.is_none()` and result non-empty.
      - `xml_bridge/proto_serde` — no network:
        `TransformRequest { stylesheet_id: <compiled id>,
        document: payload.as_bytes().to_vec(), parameters: empty,
        output_method: "xml" }.encode_to_vec()`, then
        `TransformResponse::decode(result_with_same_size.as_slice())`
        where the decode input is a pre-built `TransformResponse`
        (result = payload bytes, error = None) encoded once outside the
        measured closure; the iteration encodes the request AND decodes
        the response.
   10. After the fourth bench: `rt.block_on(process.stop())`.
5. Run `cargo fmt`; run `cargo clippy -p camel-bench --all-targets -- -D
   warnings` and fix findings.

Tests (the three tests that touch process env vars share one
`static ENV_LOCK: Mutex<()>` so they cannot race inside a single
`--lib` process):
- name: `resolve_binary_prefers_env_override`
  setup: under `ENV_LOCK`, env var `CAMEL_XML_BRIDGE_BINARY_PATH` points
  at an existing `tempfile::tempdir()`-resident file; `start` = a
  different `tempfile::tempdir()` path with no workspace root above it
  action: call `resolve_bridge_binary_from(start.path())`
  assert: returns the env-var path
  command: `cargo test -p camel-bench --lib resolve_binary_prefers_env_override`
  expected: pass after implementation
- name: `resolve_binary_errors_when_no_binary_found`
  setup: under `ENV_LOCK`, env var unset; `start` = `tempfile::tempdir()`
  path whose ancestor chain contains no `[workspace]`+`bridges/` root
  action: call `resolve_bridge_binary_from(start.path())`
  assert: `Err` whose message contains `CAMEL_XML_BRIDGE_BINARY_PATH`
  (env-independent — does not depend on whether the real worktree
  binary exists, because `start` cannot resolve the real root)
  command: `cargo test -p camel-bench --lib resolve_binary_errors_when_no_binary_found`
  expected: pass after implementation
- name: `evidence_run_flag_reads_env`
  setup: under `ENV_LOCK`, env `XMLPERF_EVIDENCE_RUN=1`
  action: call `xmlperf_evidence_run()`
  assert: returns true; after unsetting, returns false
  command: `cargo test -p camel-bench --lib evidence_run_flag_reads_env`
  expected: pass after implementation

Acceptance:
- With a binary present: `cargo bench -p camel-bench --bench
  xml_bridge_decompose -- --list` prints the four ids above; a
  `--warm-up-time 1 --measurement-time 1` run completes, and after it
  exits `pgrep -f "bridges/xml/build/native/xml-bridge"` prints nothing
  (bridge stopped, no orphan).
- With no binary resolvable and `XMLPERF_EVIDENCE_RUN` unset: the bench
  prints one skip line and exits 0.
- With no binary resolvable and `XMLPERF_EVIDENCE_RUN=1`: exits 2.
- `cargo clippy -p camel-bench --all-targets -- -D warnings` exits 0.
- `cargo test -p camel-bench --lib` exits 0.
- `cargo fmt --check` clean for the touched files.

- [x] 1.1

### Task 1.2 — Baseline native binary rebuild from current main + provenance record

Files:
- `openspec/changes/xmlperf-bridge-overhead/bench-evidence.md` (new)

Steps:
1. Run `cargo xtask build-xml-bridge` from the worktree root (docker
   builder `quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25`,
   ~20 min). Prefix the cargo invocation with `RUSTC_WRAPPER=`.
2. Record in `bench-evidence.md`: source commit (`git rev-parse HEAD`),
   builder image ID (`docker inspect --format '{{.Id}}'
   quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25`), binary
   SHA-256 (`sha256sum bridges/xml/build/native/xml-bridge`), build
   timestamp, and the statement "this binary is the A/B BASELINE; the
   historical Aug-30 prebuilt is reproduction-check only".
3. Reproduction check: copy the historical prebuilt from the main
   checkout (`/home/kenny/dev/rust-camel/bridges/xml/build/native/
   xml-bridge`) to `/tmp/xmlperf-repro/xml-bridge`, then run
   `CAMEL_XML_BRIDGE_BINARY_PATH=/tmp/xmlperf-repro/xml-bridge cargo
   bench -p camel-bench --bench xml_bridge_decompose -- --warm-up-time
   2 --measurement-time 3` and record the four phase estimates under a
   "reproduction check (historical binary)" heading. Overhead
   reproduction is confirmed when the `transform_mtls` median ≥ 2000 µs
   on the historical binary.
4. Leave `/tmp/xmlperf-repro` in place; do not mutate any file in the
   main checkout.

Tests:
- name: `baseline-binary-runs`
  setup: rebuilt binary at `bridges/xml/build/native/xml-bridge` in the
  worktree
  action: one bench invocation with default binary resolution
  assert: four phase estimates printed, exit 0
  command: `XMLPERF_EVIDENCE_RUN=1 cargo bench -p camel-bench --bench
  xml_bridge_decompose -- --warm-up-time 1 --measurement-time 1`
  expected: pass

Acceptance:
- `bench-evidence.md` exists with the five provenance fields (commit,
  image ID, SHA-256, timestamp, baseline designation).
- The reproduction-check section records all four phase estimates for
  the historical binary.
- `git -C /home/kenny/dev/rust-camel status --short` output unchanged
  by this task (main checkout untouched).

- [x] 1.2

### Task 1.3 — Four-phase evidence run + attribution + decision-tree row selection

Files:
- `openspec/changes/xmlperf-bridge-overhead/bench-evidence.md` (modified)

Steps:
1. Quiet-host check: read `/proc/loadavg` and `nproc`; proceed only
   when 1-min load < cores/2; record both numbers.
2. Evidence run against the BASELINE binary (default resolution), 5
   separate invocations of `XMLPERF_EVIDENCE_RUN=1 cargo bench -p
   camel-bench --bench xml_bridge_decompose`, so each phase has 5
   independent criterion medians.
3. Compute per phase: median of the 5 rep medians; noise `N_p` =
   max − min of the 5 rep medians. Record the table.
4. Compute derived quantities with their bands (`N_X`, `N_D`, `N_G`,
   `N_S` per design formulas): `X = transform − transform_dispatch`,
   `D = transform_dispatch − health`, `G = health − proto_serde`, and
   `O = transform_median − 740 µs` (design anchor).
5. Apply the design `## Fix-selection decision tree` rows 1–5 with the
   algorithm exactly as written (collect firing rows, projected
   reduction, 20% tie formula, row order); record which row(s) fired
   with their threshold arithmetic, numbers substituted.
6. Disposition every bd rc-dkr1m hypothesis against the numbers:
   "per-call stream setup" (compare `G`; if anomalous, note the
   socket-observation follow-up), "Java dispatch queueing" (`D`),
   "double serialization" (`proto_serde` share). Each gets CONFIRMED /
   ELIMINATED / NOT-SEPARABLE with the supporting number.
7. Commit bench-evidence.md.

Tests:
- name: `evidence-doc-complete`
  setup: bench-evidence.md after step 6
  action: check sections exist — provenance, reproduction check, host
  quiet-check numbers, 5-rep phase table with N_p, derived table with
  bands, row-selection arithmetic, hypothesis dispositions
  assert: all sections present; exactly one row selected (or row 5)
  command: `test -f openspec/changes/xmlperf-bridge-overhead/bench-evidence.md && grep -c "CONFIRMED\|ELIMINATED\|NOT-SEPARABLE" openspec/changes/xmlperf-bridge-overhead/bench-evidence.md`
  (expect ≥ 3)
  expected: pass

Acceptance:
- bench-evidence.md contains all sections above; the selected row's
  threshold arithmetic is shown with the actual numbers substituted.
- All three bd hypotheses have a disposition word with numbers.
- Evidence run executed with 1-min load < cores/2 (numbers recorded).

- [x] 1.3

## Phase 2: Fix or rule

### Task 2.1 — Sub-decomposition for the selected row

The row selected in Task 1.3's bench-evidence.md governs; exactly one
branch executes.

**Branch A — Row 1 (X-path) or Row 2 (D-path) selected: Java timing
instrumentation.**

Files:
- `bridges/xml/src/main/java/org/rustcamel/xmlbridge/Timing.java` (new)
- `bridges/xml/src/main/java/org/rustcamel/xmlbridge/XsltTransformerService.java` (modified)
- `bridges/xml/src/test/java/org/rustcamel/xmlbridge/TimingTest.java` (new)

Steps:
1. `Timing.java`: public final class with:
   - `@FunctionalInterface public interface ThrowingSupplier<T, E extends
     Exception> { T get() throws E; }`
   - `public static <T, E extends Exception> T span(String label,
     ThrowingSupplier<T, E> body) throws E` — when disabled
     (`enabled()` false), returns `body.get()` with zero extra
     allocation and no side effects; when enabled, nanoTime-brackets and
     writes ONE line to `System.err`:
     `XMLPERF_TIMING span=<label> ns=<elapsed>` then returns the value
     (exceptions propagate unchanged after the line is written);
   - `public static boolean enabled()` reading env `XML_BRIDGE_TIMING`
     once into a cached `boolean`, plus package-private `static void
     overrideForTests(Boolean v)` clearing the cache (test seam).
   (System.err, not a logger, so lines reach the bridge's stderr
   unconditionally and are capturable by the wrapper redirect below.
   The ThrowingSupplier shape is required because `secureSaxSource` and
   `Transformer.transform` throw checked exceptions.)
2. `XsltTransformerService.transform`: wrap the whole method body in
   `Timing.span("method_total", ...)`; inside it wrap
   `lookup` (`stylesheetHashById.get` + cache fetch), `setup`
   (`secureSaxSource`), `engine` (`transformer.transform`), and `encode`
   (response build) each in its own `Timing.span`. No behavior change
   when disabled.
3. Rebuild a BASELINE+TIMING binary: same builder image and xtask
   command as Task 1.2; record provenance (commit, image ID, SHA-256)
   in bench-evidence.md under "instrumentation build".
4. Capture: `BridgeProcess` launches the configured binary path with
   NO arguments, so the wrapper must self-resolve the real binary (the
   existing `bridge-wrapper.sh` pattern). Write `/tmp/xmlperf-timing-wrapper.sh`:
   ```
   #!/usr/bin/env bash
   set -euo pipefail
   exec "${CAMEL_XML_BRIDGE_REAL_BINARY:?wrapper needs real binary}" "$@" 2>/tmp/xmlperf-timing.log
   ```
   (`chmod +x` it.) Then run the bench with env:
   `CAMEL_XML_BRIDGE_BINARY_PATH=/tmp/xmlperf-timing-wrapper.sh
   CAMEL_XML_BRIDGE_REAL_BINARY=<worktree>/bridges/xml/build/native/xml-bridge
   XML_BRIDGE_TIMING=1` (env propagates to the child), one invocation
   (`--warm-up-time 2 --measurement-time 5`); parse
   `/tmp/xmlperf-timing.log` with `grep '^XMLPERF_TIMING' | sort |
   awk` to compute per-span MEDIAN ns over the steady-state window
   (drop the first 200 lines per span). Repeat 3 times → per-span
   medians ×3; `N_sub` = max − min of the 3 per-span medians.
5. Arithmetic (recorded in bench-evidence.md):
   - Row 1 quantity `X` sub-shares: `setup`, `engine`, `encode`
     (in-method spans; `lookup` recorded but expected ≈ 0).
     `pre-method = X − method_total_median` recorded as a residual
     line, NOT a share (it belongs to the dispatch path).
   - Row 2 quantity `D` sub-shares: `handoff_estimate = (D −
     method_total_median) − proto_serde_median` (pre-method remainder
     minus the client-measured decode floor; wire decode on the server
     is ≈ the client serde floor for the same payload);
     `error_encode` share from a span wrapping the error-response
     build in the unknown-id branch (add `Timing.span("err_encode",
     ...)` there too); remainder = payload decode, not further
     separated at this instrumentation level.
6. Apply the design sub-decomposition predicate (CAUSAL iff share >
   0.5·quantity AND > 3·N_sub) to the row's quantity; record the
   outcome: the CAUSAL mechanism named, or else → no-action.
7. REVERT the instrumentation before any fix work, with the correct
   git mechanics per file state: `git restore -- bridges/xml/src/main/
   java/org/rustcamel/xmlbridge/XsltTransformerService.java` (tracked,
   modified) and `git clean -f -- bridges/xml/src/main/java/org/
   rustcamel/xmlbridge/Timing.java bridges/xml/src/test/java/org/
   rustcamel/xmlbridge/TimingTest.java` (untracked, new). Verify with
   `git status --short -- bridges/xml` printing nothing. Task 2.2's
   fix branch then applies to source that differs from the baseline by
   the fix ONLY (design's provenance rule). The instrumentation
   binary, its provenance, and the recorded numbers stay in
   bench-evidence.md — the revert removes the SOURCE, not the
   evidence.

**Branch B — Row 3 (transport stack) selected: reuse observation.**
Files: no source files; artifact recorded in bench-evidence.md.
1. Start a held bench run in the background and read its
   `XMLPERF_BRIDGE port=...` line:
   `XMLPERF_EVIDENCE_RUN=1 cargo bench -p camel-bench --bench
   xml_bridge_decompose -- --warm-up-time 2 --measurement-time 30 &`
   then `wait` for the line on stdout (or capture the run's output to a
   file with `> /tmp/xmlperf-held.log 2>&1` and poll it with grep).
2. Two signals, with the port-scoped one PRIMARY (snapshot counting
   alone is blind to sequential reconnects — a new short-lived
   connection per call keeps the instantaneous count at one; and
   `nstat -az TcpPassiveOpens` is SYSTEM-WIDE with no port filter, so
   it is a secondary, contamination-acknowledged signal):
   - Signal 1 (primary, port-scoped): capture
     `ss -tnp state established '( sport = :PORT )'` at T0 and T1
     (≥10 s apart, spanning ≥1000 calls inside the measurement-time
     window); record the CLIENT's ephemeral local port from the ESTAB
     line's peer address column.
   - Signal 2 (secondary): `nstat -az TcpPassiveOpens` delta over the
     same T0→T1 window.
   Verdict rules (in order):
   - Client ephemeral port CHANGED between T0 and T1 → reuse broken
     (definitive regardless of Signal 2) → record for Task 2.2
     Branch 3.
   - Port stable AND Signal 2 delta == 0 → same connection survives
     → reuse correct → no-action exit for row 3.
   - Port stable AND Signal 2 delta > 0 → AMBIGUOUS (delta may be
     background traffic on other ports): re-run the window once. If
     the client port is still stable on the second run, attribute the
     delta to background (record both runs' numbers with the caveat)
     → reuse correct with caveat. If the port changed on re-run →
     reuse broken.

**Branch C — Row 4 (client serde) selected: copy audit.**
Files:
- `crates/camel-bench/src/lib.rs` (modified — adds the probe below;
  manual-measurement tool, `#[ignore]`-annotated so it never runs in
  CI gates)
- `openspec/changes/xmlperf-bridge-overhead/bench-evidence.md`
  (modified — audit table)
1. Enumerate the copy classes on `XsltBridgeClient::transform` →
   `GrpcXsltBackend::transform`: (a) per-call `Vec<(String,String)>` →
   `HashMap` conversion, (b) `stylesheet_id.clone()` into the protobuf
   field, (c) `document` move and response `result` move (expected
   copy-free — verify, do not assume). Add to `lib.rs`:
   `#[ignore] #[test] fn copy_audit_probe()` timing each class over
   10 000 iterations on the 1 KB payload with an empty params Vec
   (match the bench: empty parameters), printing µs/op per class.
   Run it 3 times manually (`cargo test -p camel-bench --lib
   copy_audit_probe -- --ignored --nocapture`); `N_sub` = max − min
   of the 3 runs per class.
2. Apply the predicate: copies CAUSAL iff copy_total > 0.5·
   `proto_serde` AND > 3·N_sub; record outcome. Note: class (b) is
   protobuf-inherent (owned `String` field in `TransformRequest`) and
   is NOT eliminable client-side; only class (a) has an elimination
   path (Task 2.2 Branch 4).
3. REVERT the probe before Task 2.2: `git restore -- crates/camel-
   bench/src/lib.rs`; verify `git status --short -- crates/camel-bench`
   prints nothing. The recorded numbers stay in bench-evidence.md —
   the revert removes the probe SOURCE, keeping fix-only provenance
   intact.

**Branch D — Row 5 selected:** no sub-decomposition. Append "row 5 —
no sub-decomposition required" to bench-evidence.md; Tasks 2.2/2.3
execute their no-action branches.

Tests (Branch A only):
- name: `TimingDisabledByDefault` (Java,
  `bridges/xml/src/test/java/org/rustcamel/xmlbridge/TimingTest.java`)
  setup: `Timing.overrideForTests(null)` (cache cleared, env unset)
  action: call `Timing.enabled()` and `Timing.span("x", () -> 42)`
  assert: `enabled()` false; returns 42; no `XMLPERF_TIMING` line on
  stderr (capture stderr via redirect in the test)
  command: `docker run --rm --user root --volume="$(pwd)/bridges/xml:/project:z" --workdir=/project --env=GRADLE_USER_HOME=/tmp/gradle-home --env=HOME=/tmp --env=APP_HOME= --entrypoint bash quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25 -c "rm -rf /project/build && ./gradlew test --no-daemon --project-cache-dir /tmp/gradle-project-cache --tests 'org.rustcamel.xmlbridge.TimingTest' 2>&1"`
  expected: pass
- name: `TimingEnabledEmitsSpans`
  setup: `Timing.overrideForTests(Boolean.TRUE)`
  action: `Timing.span("lookup", () -> 7)`
  assert: returns 7; exactly one stderr line matching
  `XMLPERF_TIMING span=lookup ns=`
  command: `docker run --rm --user root --volume="$(pwd)/bridges/xml:/project:z" --workdir=/project --env=GRADLE_USER_HOME=/tmp/gradle-home --env=HOME=/tmp --env=APP_HOME= --entrypoint bash quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25 -c "rm -rf /project/build && ./gradlew test --no-daemon --project-cache-dir /tmp/gradle-project-cache --tests 'org.rustcamel.xmlbridge.TimingTest' 2>&1"`
  expected: pass
- name: `transformBehaviorUnchangedWithTiming` (Java, same file)
  setup: existing integration fixtures
  action: existing `XsdValidationIntegrationTest` + a new single test
  calling `transform` twice via the gRPC test harness
  assert: both outputs identical to the pre-instrumentation expected
  values
  command: `docker run --rm --user root --volume="$(pwd)/bridges/xml:/project:z" --workdir=/project --env=GRADLE_USER_HOME=/tmp/gradle-home --env=HOME=/tmp --env=APP_HOME= --entrypoint bash quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25 -c "rm -rf /project/build && ./gradlew test --no-daemon --project-cache-dir /tmp/gradle-project-cache --tests 'org.rustcamel.xmlbridge.*' 2>&1"`
  expected: pass

Acceptance:
- bench-evidence.md gains a sub-decomposition section with per-span µs,
  `N_sub`, predicate arithmetic, and exactly one outcome (CAUSAL
  mechanism or no-action).
- Branch A: `TimingTest` + full existing suite pass under the canonical
  gradle-in-container invocation (filter removed); `./gradlew
  spotlessCheck` passes in the same container.
- Branch B/C: the observation/audit artifact is in bench-evidence.md;
  no source file changed.
- Branch D: the row-5 line appended; no source file changed.

- [x] 2.1

### Task 2.2 — Implement the selected fix or land the no-action ruling

Task 2.1's recorded outcome governs; exactly one branch executes.

**Branch 1 — Row 1 setup CAUSAL: thread-local SAXParserFactory hoist.**
Files:
- `bridges/xml/src/main/java/org/rustcamel/xmlbridge/XsltTransformerService.java` (modified)
- `bridges/xml/src/test/java/org/rustcamel/xmlbridge/SecureSourceReuseTest.java` (new)

Steps:
1. In `XsltTransformerService`, add:
   `static final AtomicLong FACTORY_CONSTRUCTIONS = new AtomicLong();`
   (package-private, for the pin) and
   `private static final ThreadLocal<org.apache.xerces.jaxp.SAXParserFactoryImpl> PARSER_FACTORY = ThreadLocal.withInitial(() -> {
   FACTORY_CONSTRUCTIONS.incrementAndGet(); var f = new
   org.apache.xerces.jaxp.SAXParserFactoryImpl(); f.setNamespaceAware(true);
   f.setFeature(XMLConstants.FEATURE_SECURE_PROCESSING, true);
   f.setFeature(FEATURE_LOAD_EXTERNAL_DTD, false);
   f.setFeature(FEATURE_EXTERNAL_GENERAL_ENTITIES, false);
   f.setFeature(FEATURE_EXTERNAL_PARAMETER_ENTITIES, false);
   f.setFeature("http://apache.org/xml/features/disallow-doctype-decl", true);
   return f; });`
   `secureSaxSource` uses `PARSER_FACTORY.get()` instead of
   constructing; it still creates a NEW `SAXParser` + `XMLReader` per
   call (readers are stateful). Make `secureSaxSource` package-private
   static for testability.
2. `SecureSourceReuseTest.java` with the two tests below.
3. Full gradle suite + spotlessCheck via the canonical container
   invocation.

Tests:
- name: `factoryConstructedOncePerThread`
  setup: `FACTORY_CONSTRUCTIONS.set(0)`
  action: spawn a FRESH dedicated test thread (`new Thread(() -> { for
  (int i = 0; i < 5; i++) { sources[i] =
  XsltTransformerService.secureSaxSource(payload); } }).start()` +
  `join()` — fresh thread guarantees the `ThreadLocal` is
  uninitialized, so the counter is deterministic regardless of which
  tests ran before on the JUnit thread)
  assert: `FACTORY_CONSTRUCTIONS.get() == 1` after the thread joins;
  each of the 5 returned `SAXSource`s (collected into an array via an
  `AtomicReference`/`CountDownLatch` handoff or a static capture list)
  parses the 1 KB bench payload to completion through the identity
  transform path used by the service
  command: `docker run --rm --user root --volume="$(pwd)/bridges/xml:/project:z" --workdir=/project --env=GRADLE_USER_HOME=/tmp/gradle-home --env=HOME=/tmp --env=APP_HOME= --entrypoint bash quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25 -c "rm -rf /project/build && ./gradlew test --no-daemon --project-cache-dir /tmp/gradle-project-cache --tests 'org.rustcamel.xmlbridge.SecureSourceReuseTest' 2>&1"`
  expected: pass
- name: `existingSecurityTestsStayGreen`
  action: full gradle suite
  assert: SecurityTest, XsdValidationIntegrationTest,
  HealthIntegrationTest pass
  command: `docker run --rm --user root --volume="$(pwd)/bridges/xml:/project:z" --workdir=/project --env=GRADLE_USER_HOME=/tmp/gradle-home --env=HOME=/tmp --env=APP_HOME= --entrypoint bash quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25 -c "rm -rf /project/build && ./gradlew test --no-daemon --project-cache-dir /tmp/gradle-project-cache 2>&1"`
  expected: pass

**Branch 2 — Row 2 handoff CAUSAL: worker-executor sizing.**
Files:
- `bridges/xml/src/main/resources/application.yml` (modified)

Steps:
1. Add `quarkus.vertx.worker-pool-size: <2 × nproc of the bench host,
   recorded value>` under the existing `quarkus.vertx` key (create the
   key block if absent) with a YAML comment citing the measured
   `handoff_estimate` µs and `N_sub` from Task 2.1 Branch A.
2. Full gradle suite via the canonical container invocation (config
   load is exercised by the integration tests).

Tests:
- name: `applicationConfigLoads`
  action: full gradle suite (integration tests boot the server with the
  modified application.yml)
  assert: all green
  command: `docker run --rm --user root --volume="$(pwd)/bridges/xml:/project:z" --workdir=/project --env=GRADLE_USER_HOME=/tmp/gradle-home --env=HOME=/tmp --env=APP_HOME= --entrypoint bash quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25 -c "rm -rf /project/build && ./gradlew test --no-daemon --project-cache-dir /tmp/gradle-project-cache 2>&1"`
  expected: pass

**Branch 3 — Row 3 reuse broken: diagnose disconnect cause, then
keepalive pin ONLY if idle-linked.**
Files (diagnostic first; the code change applies only on the
idle-linked outcome):
- diagnostic artifact: trace log + verdict recorded in
  bench-evidence.md (no source)
- `crates/services/camel-bridge/src/channel.rs` (modified — only if
  idle-linked)
- `crates/services/camel-bridge/src/channel.rs` tests module (modified
  — only if idle-linked)

Steps:
0. Diagnostic (before any code change): re-run the held bench with
   transport tracing: `RUST_LOG=tonic=trace,hyper=trace,h2=trace
   XMLPERF_EVIDENCE_RUN=1 cargo bench -p camel-bench --bench
   xml_bridge_decompose -- --warm-up-time 2 --measurement-time 30 >
   /tmp/xmlperf-trace.log 2>&1`. DIAGNOSTIC-VALIDITY PIN (run first):
   `grep -cE 'tonic|hyper|h2' /tmp/xmlperf-trace.log` MUST be > 0 —
   zero matches means the subscriber init failed (Task 1.1's
   conditional install), not that no events occurred; in that case fix
   the init and re-run the diagnostic before interpreting anything.
   Then extract connection lifecycle events
   (`grep -iE 'connection (closed|closing|error)|goaway|reset'`).
   Verdict rule: reconnect events correlating with idle gaps between
   bench phases (warm-up→measurement boundary) = IDLE-LINKED →
   proceed to the keepalive pin (step 1). Reconnect events during
   sustained in-flight calls = NOT idle-linked → keepalive does not
   apply → no-action exit with the trace evidence (inherent transport
   defect; follow-up documented). No events but port changed →
   ambiguous → no-action exit with both artifacts.
1. (idle-linked only) In `connect_channel`, after building the endpoint, add
   `.http2_keep_alive_interval(Duration::from_secs(10))
   .http2_keep_alive_timeout(Duration::from_secs(5))
   .keep_alive_while_idle(true)` (tonic `Endpoint` builder methods) —
   this pins connection liveness so the shared channel cannot silently
   drop to per-call reconnects. No TLS changes.
3. Extract the endpoint construction (currently inline in
   `connect_channel`) into `pub(crate) fn bridge_endpoint(uri: &str) ->
   Result<Endpoint, BridgeError>` applying the keepalive settings;
   `connect_channel` calls it. Add the unit test below to channel.rs's
   existing `#[cfg(test)] mod tests`. (This extraction runs only on the
   idle-linked path; on the not-idle-linked path Branch 3 ends at
   step 0's no-action exit.)

Tests:
- name: `channel_endpoint_configures_keepalive`
  setup: none (no connection is made)
  action: call `bridge_endpoint("https://127.0.0.1:1")` and build a
  `ClientTlsConfig` separately as `connect_channel` does
  assert: the endpoint constructs without error (the keepalive
  settings are applied in `bridge_endpoint`'s body — tonic `Endpoint`
  does not expose getters, so the unit test pins construction success;
  the functional proof that one ESTAB connection survives ≥1000 calls
  is Task 2.3 step 4's socket re-observation)
  command: `cargo test -p camel-bridge channel_endpoint_configures_keepalive`
  expected: pass

**Branch 4 — Row 4 copies CAUSAL: eliminate the Vec→HashMap conversion.**
Files:
- `crates/components/camel-xslt/src/client.rs` (modified)
- `crates/components/camel-xslt/src/producer.rs` (modified)
- `crates/components/camel-xslt/src/bridge_client_test.rs` (modified)

Exact API chain (the only eliminable copy class is (a), the per-call
Vec→HashMap conversion; class (b) is protobuf-inherent and stays):
1. `XsltTransformBackend::transform` signature becomes
   `async fn transform(&self, channel: Channel, stylesheet_id: &str,
   document: Vec<u8>, parameters: &HashMap<String, String>,
   output_method: &str) -> Result<(Vec<u8>, Option<String>), XsltError>`
   (borrows instead of owning; the single `stylesheet_id.to_owned()`
   and `parameters.clone()` happen AT the `TransformRequest`
   construction site inside `GrpcXsltBackend` — the protobuf-inherent
   minimum, no intermediate copies).
2. `XsltBridgeClient::transform` passes `id.as_str()`, the document,
   `&params_map`, and the output method slice.
3. `producer.rs`: the producer builds its params `HashMap<String,
   String>` ONCE (at compile/first-call, stored in the producer struct
   behind `Arc<HashMap<String, String>>`) instead of passing
   `Vec<(String, String)>` per call; the per-call Vec→HashMap
   conversion disappears.
4. Update `bridge_client_test.rs` mocks to the new trait signature;
   add the behavioral test below.
5. `cargo clippy -p camel-xslt --all-targets -- -D warnings`;
   `cargo test -p camel-xslt`.

Tests:
- name: `transform_requests_identical_across_calls`
  setup: mock `XsltTransformBackend` recording every
  `TransformRequest` (fields: stylesheet_id, document bytes,
  parameters, output_method) in a `Mutex<Vec<TransformRequest>>`
  action: 5 sequential `XsltBridgeClient::transform` calls with the
  same id/document/params through a `BridgeState::Ready` mock channel
  assert: all 5 recorded requests are field-identical (borrowed path
  did not alter request semantics)
  command: `cargo test -p camel-xslt transform_requests_identical_across_calls`
  expected: pass

**Branch 5 — no-action (any row's else, or row 5).**
Files:
- `openspec/changes/xmlperf-bridge-overhead/bench-evidence.md` (modified)

Steps:
1. Append the ruling section quoting the Task 2.1 numbers, the design
   row that fired, and the follow-up options as text only (opt-in
   JVM-mode bridge binary, engine alternatives). No source changes.

Commit gate (product-changing branches 1-4 only): commit the fix
before Task 2.3 begins — `git add <this branch's files> && git commit
-m "<conventional subject>"` so the FIXED-build provenance in Task 2.3
records a real fix commit (`git rev-parse HEAD` post-commit). Branch 5
changes no source and commits nothing.

Acceptance (all branches):
- Exactly one branch executed; `git status` shows only that branch's
  files changed (before the commit gate runs).
- Owning test suite green (gradle full suite + spotlessCheck for Java
  branches; clippy + cargo test for Rust branches).
- No change under `crates/camel-cli` or
  `openspec/specs/cli-feature-profiles` (forbidden zones).

- [x] 2.2

### Task 2.3 — Fixed-binary rebuild, same-toolchain A/B, closure verdict, records

Files:
- `openspec/changes/xmlperf-bridge-overhead/bench-evidence.md` (modified)
- `.opencode/fleet/inbox/xmlperf-park.json` (new, git-ignored scratch)

Steps:
1. If Task 2.2 changed bridge source (Branches 1/2): `RUSTC_WRAPPER=
   cargo xtask build-xml-bridge` again (same builder image); record the
   new SHA-256 + commit as FIXED build. If Task 2.2 changed Rust client
   code only (Branch 3/4): record `git rev-parse HEAD` (post-fix
   commit) as the FIXED build; no native rebuild needed. If Branch 5:
   skip to step 5 with verdict (c).
2. A/B evidence run: 5 invocations against the FIXED build under the
   same quiet-host rule as Task 1.3; same phase table computation. The
   before/after pair = Task 1.3 baseline table vs this table.
3. Apply the design closure rule with arithmetic shown: (a) majority
   closed iff post-fix `transform` median ≤ pre-fix `transform` −
   0.5·O; (b) else partial closure — record numbers, STOP (no further
   fixes this change); Branch 5/else outcomes are (c) no-action.
4. Record the fixed-binary regression baseline in bench-evidence.md:
   phase medians + per-phase noise band; the regression criterion = a
   phase median regressing beyond max(15%, recorded spread) over this
   baseline is flagged (this satisfies the spec's baseline-relative
   regression scenario). Branch 3 additionally re-runs the Task 2.1
   Branch B observation against the FIXED build, applying the SAME
   ordered verdict rules (inlined here): capture
   `ss -tnp state established '( sport = :PORT )'` at T0/T1 (≥10 s
   apart, ≥1000 calls) recording the client ephemeral port, plus
   `nstat -az TcpPassiveOpens` delta as the secondary signal. Verdict:
   port stable AND delta == 0 → keepalive fix functionally proven;
   port stable AND delta > 0 → re-run once, still stable → proven with
   background caveat recorded; port changed → fix failed, exit (b)
   partial closure with numbers.
5. Post the before/after summary + closure verdict as a bd comment
   (`bd update rc-dkr1m --comment "<summary>"` from the MAIN checkout
   root). Write the same numbers (four phase medians before/after,
   provenance SHAs, closure verdict, selected row) to
   `/home/shared/rust-camel-worktrees/xmlperf/.opencode/fleet/inbox/
   xmlperf-park.json` (`.opencode/fleet/` is git-ignored; the conductor
   consumes it at park time).

Tests:
- name: `ab-table-complete`
  setup: bench-evidence.md after step 4
  action: `grep -c provenance openspec/changes/xmlperf-bridge-overhead/bench-evidence.md`
  assert: ≥ 2 (baseline + fixed), or ≥ 1 plus a no-action skip note;
  exactly one of "majority closed" / "partial closure" /
  "no-action ruling" present
  command: as in action
  expected: pass

Acceptance:
- bench-evidence.md has the A/B table (or no-action skip note), the
  closure verdict with arithmetic, and the fixed-binary regression
  baseline with noise band.
- `bd show rc-dkr1m --json` reports comment_count ≥ 1.
- `.opencode/fleet/inbox/xmlperf-park.json` exists with the numbers.

- [x] 2.3
