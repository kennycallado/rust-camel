# Design: xmlperf-bridge-overhead

## Goal

Reproduce bd rc-dkr1m's 2.3–2.5 ms per-call xml-bridge overhead on current
main (0.49.0), decompose it into measured phases, and close the majority
via the smallest justified fix — or land a numbers-backed no-action ruling
if the residual is inherent engine cost.

## Measured surfaces (decomposition model)

Warm mTLS gRPC loopback between the tonic/rustls client and the native
Quarkus bridge. Four criterion benches in `crates/camel-bench` (new file
`benches/xml_bridge_decompose.rs`, following `direct_decompose.rs`
conventions):

| id | request | measures |
|----|---------|----------|
| `xml_bridge/health_mtls` | `Health.Check`, empty | non-blocking transport floor: TLS record, h2, protobuf, event-loop dispatch |
| `xml_bridge/transform_dispatch_mtls` | `Transform` with ~1 KB payload but UNKNOWN stylesheet id | same wire shape as a real transform on the `@Blocking` path — worker-pool handoff, payload decode, service dispatch, error encode — with zero XML engine work |
| `xml_bridge/transform_mtls` | full `Transform` (1 KB payload, identity stylesheet, precompiled id) | the repro cell ≈ era-2 m2 minus route overhead |
| `xml_bridge/proto_serde` | none (client-side only) | prost encode+decode floor of the same messages |

Attribution arithmetic (all µs, medians):
- `D = transform_dispatch − health` ≈ blocking-dispatch + payload-path
  cost (`@Blocking` worker handoff + protobuf payload decode + error
  encode). Known confound: success vs error response shape and payload
  size — stated in the evidence doc, not silently absorbed.
- `X = transform − transform_dispatch` ≈ in-bridge XML engine work
  (native-Saxon/Xerces tax, parser construction, transform).
- `G = health − proto_serde` ≈ gRPC/TLS/Java-dispatch stack on the
  non-blocking path.
- External anchors (era-2): JVM Saxon in-process 0.74 ms; quarkus-native
  Saxon in-process 2.03 ms (D-9) — the native-image Saxon tax is visible
  in-record and is a live hypothesis for part of `X`.

Hypotheses the four phases cannot separate get targeted follow-up
measurements, never inferred verdicts:
- per-call Rust client copies (bd "double serialization") → component
  wrapper vs raw tonic call comparison (one-off bench variant) if
  `proto_serde` + `health` do not already account for the client budget;
- stream setup vs connection reuse → grpc-socket-level observation
  (connection count under load) if `G` is anomalous.

## Bench mechanics

- Spawn: `BridgeProcess::start_and_connect(&BridgeProcessConfig::xml(binary, 60_000))`
  — the exact production spawn path (camel-bridge public API). Binary
  resolution: `CAMEL_XML_BRIDGE_BINARY_PATH` env override, else
  `{workspace_root}/bridges/xml/build/native/xml-bridge` (workspace-root
  walk copied from `camel-bridge/src/download.rs` convention).
- Absent binary: with `XMLPERF_EVIDENCE_RUN` unset → developer run:
  register no benches, print one skip line naming the missing path, exit
  0. With `XMLPERF_EVIDENCE_RUN` set → exit non-zero (evidence cannot be
  collected silently without the bridge).
- Proto codegen: new `crates/camel-bench/build.rs` compiles the canonical
  `bridges/xml/src/main/proto/xml_bridge.proto` via
  `tonic_prost_build` + `protoc-bin-vendored` (same pattern as
  camel-xslt's build.rs, but referencing the canonical proto — no third
  vendored copy).
- Warmup: compile stylesheet id once, ≥200 warm calls per phase before
  sampling. Statistic: criterion median per repetition, ≥5 repetitions
  per phase, report per-repetition median spread. Criterion does not
  expose p99; medians + spread are the recorded statistics (this is the
  p99-definition ruling).
- Quiet-host guard: bench reads `/proc/loadavg` into the report;
  evidence runs only proceed when 1-min load < cores/2.

## Fix-selection decision tree (pre-committed, single algorithm)

Definitions: total overhead `O = transform − A`, where `A = 740 µs`
(the era-2 JVM Saxon in-process anchor, xslt family) — used
unconditionally: the bench's `transform` phase measures the xslt family,
so `O` is defined before any row is evaluated (no circularity). The xsd
family shares the identical transport + dispatch path (same bridge
process, same `@Blocking` dispatch, same client stack), so a fix
selected on xslt evidence transfers to xsd; this transfer assumption is
recorded in the evidence doc, and the anchor is NOT re-derived per
family. Per-phase noise `N_p` = per-repetition median spread of phase
`p`. Derived noise bands (differences add their phases' bands):
`N_X = N_transform + N_dispatch`, `N_D = N_dispatch + N_health`,
`N_G = N_health + N_proto`, `N_S = N_proto`. Exact formulas: per-phase
noise `N_p = max(rep medians of p) − min(rep medians of p)`; the tie
comparison "within 20%" means `|a−b| / max(a,b) ≤ 0.20`.

Sub-decomposition predicate (used by rows 1, 2, 4; exact): a
sub-mechanism is CAUSAL iff its measured share `> 0.5·quantity` AND
`> 3·N_sub`, where `N_sub` = instrumentation-run noise band
(`max − min` over ≥3 instrumentation repetitions). Any other
outcome — including split sub-shares, unattributed remainders, or
shares inside the noise band — takes the row's unconditional
`else → no-action` exit (numbers recorded). This makes every branch
exhaustive: exactly one of {causal → fix, else → no-action} can hold.

Rows (premise must hold including its noise test):

1. XML-path tax: `X ≥ 0.5·O` and `X > 3·N_X`. `X` covers per-call
   success-path work: parser-factory construction (`secureSaxSource`
   builds a `SAXParserFactoryImpl` per call and runs only on the
   success path — its cost lands in `X`, not `D`), transform engine
   work, result encode. Sub-decompose `X` on the Java side BEFORE
   ruling (timing instrumentation behind an env flag or a one-off
   in-container Java timing harness), measuring setup (parser factory,
   resolver wiring) vs engine vs result-encode shares. If setup is
   CAUSAL (predicate above) → fix = hoist that setup to thread-local
   caches (SAXParserFactory is not thread-safe; hoists must be
   thread-local). Else (engine cost, result-encode dominance, split
   shares, or within noise) → no-action exit with the sub-decomposition
   numbers; bd follow-up options documented (opt-in JVM-mode bridge
   binary, engine alternatives). Stop.
2. Blocking-dispatch/payload defect: `D ≥ 0.4·O` and `D > 3·N_D`.
   Sub-decompose on the Java side (timing instrumentation behind an
   env flag if needed), measuring worker-pool queueing/handoff vs
   payload decode vs error-encode shares. If worker handoff is CAUSAL
   (predicate above) → fix = worker-executor sizing/tuning only. Else
   (payload/error encoding, split shares, or within noise) → no-action
   exit with the sub-decomposition numbers. Dropping `@Blocking`
   entirely is NOT a candidate (SAX parsing of untrusted documents must
   stay off the event loop); only evidence-backed executor tuning is
   permitted. (Factory hoisting is NOT available here — it lives in
   row 1's success-path domain.)
3. Transport-stack defect (non-blocking path): `G ≥ 0.4·O` and
   `G > 3·N_G`. First verify connection/stream reuse (one socket-level
   observation). If reuse is broken (e.g. a new connection per call) →
   smallest reuse fix within ADR-0036 (no plaintext, no cert-cache
   weakening). If reuse is correct and the residual is inherent stack
   cost → no-action exit with the observation. Stop after one fix.
4. Client serialization defect: `proto_serde ≥ 0.3·O` and
   `> 3·N_S`. Inspect the Rust path for double copies. If measured
   copies are CAUSAL (predicate above) → eliminate them; else →
   no-action exit with the copy audit.
5. No row fires → no single dominant contributor → no-action ruling
   with the full decomposition table; per-share remainder documented as
   follow-up options.

Algorithm: collect every row whose premise holds. If none → row 5. If
one → that row. If several → compute each row's projected actionable
reduction (its phase µs minus its noise band); the row with the largest
projected reduction wins; if two are within 20% of each other (exact
formula above), the larger absolute µs wins; if still tied, row order
breaks the tie.

Closure rule (coherent with selection): success = post-fix `transform`
reduced by ≥ 50% of `O` under same-toolchain A/B. Because a single
selected row's share may be below 0.5·O, the exit is three-valued:
(a) majority closed → done; (b) fix landed, measured, but closure <
   50% of `O` → record partial-closure numbers, then STOP (no fix
   stacking; remaining contributors documented as follow-up options);
(c) no-action ruling → numbers recorded. No-action exits exist at:
   row 1 (engine-cost residual), row 2 (mechanism not executor), row 3
   (reuse correct, stack cost inherent), row 4 (no double copies
   found), and row 5. Every exit lands in `bench-evidence.md` and bd
   rc-dkr1m.

## TDD pins

- Rust: unit tests pin whatever client-side mechanism changes (if any).
- Java: pins for hoisted factories (behavioral: repeated transforms
  produce correct output; factory reuse observable via a package-private
  counter test), plus existing `XsdValidationIntegrationTest` /
  `SecurityTest` / `HealthIntegrationTest` must stay green — they are the
  sidecar-xml-security regression net.
- Bench itself is the evidence pin: committed phase medians recorded in
  `openspec/changes/xmlperf-bridge-overhead/bench-evidence.md` (before +
  after rebuild).

## Architectural boundaries respected

- No data/control-plane change; component crates keep ownership of
  reconnect orchestration (camel-bridge stays primitives-only).
- ADR-0036 mTLS posture: no plaintext transport, no persistent cert
  caching, fail-closed guard untouched.
- `sidecar-xml-security` + `bridge-transport-security` specs: all
  existing scenarios must remain green; any Java-side change re-runs the
  gradle test suite in the GraalVM container before native rebuild.
- camel-cli and openspec/specs/cli-feature-profiles: untouched.

## Binary provenance + rebuild discipline

The historical prebuilt binary (Aug 30) is a reproduction check ONLY:
its toolchain/deps/build flags are not reconstructible from source
alone, so it never serves as an A/B baseline. Evidence pairs are:

1. **Baseline build**: rebuild native binary from untouched current-main
   source (`cargo xtask build-xml-bridge`, docker
   `quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25`,
   recorded in evidence doc). Phase-1 evidence + hypothesis verdicts run
   against this binary.
2. **Fixed build**: same builder image + flags, source differing only by
   the fix commit. Before/after table = baseline vs fixed.

Both binaries' SHA-256 + build recipe are recorded in
`bench-evidence.md`; regression noise band = max(15%, recorded
per-repetition spread) per phase.

## Phases

- **Phase 1 — Reproduce + attribute**: bench lands; baseline binary
  rebuilt from current main; evidence doc records all four phase medians
  + provenance + derived quantities + hypothesis verdicts. Exit: exactly
  one decision-tree row selected (or row 5), documented.
- **Phase 2 — Fix or rule**: selected fix lands with TDD pins; native
  rebuilt same-toolchain; before/after recorded; gates + reviews. Exit
  (three-valued, per the closure rule): majority closed, partial closure
  (numbers recorded, stop, follow-ups documented), or no-action ruling —
  each with numbers in `bench-evidence.md` and bd rc-dkr1m.
