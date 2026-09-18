# Proposal: xmlperf-bridge-overhead

## Why

bd rc-dkr1m (P2): every gRPC call routed through the XML sidecar bridge
(`bridges/xml`) pays ~2.3–2.5 ms on top of the in-process transform
baseline. Era-2 records are stable across both runs (20260903T084658Z,
20260915T093128Z): rust xslt-lib 3.01/3.09 ms vs JVM Saxon in-process
0.74 ms; rust xsd-lib 2.80/2.91 ms vs quarkus-native Xerces in-process
0.41 ms. A warm loopback mTLS gRPC call with a ~1 KB payload should cost
well under 1 ms, so the bridge tax multiplies every bridge-backed
component's hot path by ~4x and makes them impractical where the
framework competes on latency.

The record's own suspicion (per-tick template recompile) was disproven by
code read in mission 116-eraudit: the client compiles once
(`Arc<OnceCell<StylesheetId>>`), the bridge caches server-side
(`StylesheetCache`, `SchemaCache`). The residual is per-call
transport + dispatch: TLS record layer, HTTP/2 framing, protobuf,
Java service dispatch, response path. One contender-internal data point
(quarkus-native xslt 2.03 ms, D-9) hints part of the gap may be
native-image Saxon cost inside the bridge rather than transport — the
decomposition must separate these before any fix is chosen.

## What changes

1. **Decomposition microbenchmark** (new criterion bench in
   `crates/camel-bench`, following the `direct_decompose` control-bench
   convention): spawns the real xml-bridge native binary via the
   existing `camel-bridge` spawn/mTLS machinery, warms the channel,
   then measures (a) full `Transform` round-trip — the repro cell,
   (b) `Transform` with an unknown stylesheet id — same payload and
   blocking-dispatch path, zero XML work (isolates `@Blocking` handoff
   + payload decode from engine cost), (c) `Health.Check` round-trip —
   non-blocking transport floor, (d) client-side-only protobuf
   encode/decode — serialization floor. XML work ≈ (a)−(b); blocking
   dispatch ≈ (b)−(c); gRPC+TLS stack ≈ (c)−(d). Residual confounds
   (response-shape differences between error and success paths,
   payload-size effects) are stated with the numbers, and hypotheses the
   four phases cannot separate get targeted follow-up measurements
   rather than inferred verdicts.
2. **Root cause** from measured phases — no guessing. Each hypothesis
   from bd rc-dkr1m (per-call stream setup, Java dispatch queueing,
   double serialization) gets confirmed, eliminated, or explicitly
   marked not-yet-separated by the numbers.
3. **Smallest fix that closes the majority of the overhead**, selected
   by the ordered decision tree pre-committed in design.md. Candidate
   surfaces: `bridges/xml` Java dispatch (worker-executor tuning,
   per-call parser construction), Rust client path, or a documented
   no-action ruling if the residual is inherent native-image engine
   cost (with numbers proving it).
4. **TDD pins** for the fixed path plus the microbenchmark itself as
   the before/after evidence artifact. Before/after A/B pairs always
   use same-toolchain rebuilds (same builder image, differing only by
   the fix); the historical prebuilt binary serves only as a
   reproduction check.

## Affected crates

- `crates/camel-bench` — new decomposition bench (certain)
- `bridges/xml` — Java-side fix + rebuilt native binary (conditional on
  evidence)
- `crates/components/camel-xslt`, `crates/components/camel-validator`,
  `crates/services/camel-bridge` — client-side fix (conditional on
  evidence)

## Constraints

- mTLS posture unchanged (ADR-0036); `sidecar-xml-security` and
  `bridge-transport-security` specs keep all scenarios green — no
  plaintext shortcut, no cert-cache weakening.
- camel-cli and `openspec/specs/cli-feature-profiles` are a forbidden
  zone (external agent owns them).
- The canonical bench suite is NOT re-run; in-mission microbenchmarks
  only (owner rules).

## Acceptance criteria

- Microbenchmark reproduces the ≥2 ms bridge overhead against the
  historical prebuilt binary (reproduction check) and against a fresh
  same-toolchain rebuild of untouched current main (the true baseline
  for A/B).
- Decomposition attributes the overhead to named phases with medians;
  every bd hypothesis has a measured confirm/eliminate verdict or an
  explicit not-separable note with the follow-up measurement that would
  separate it.
- Outcome (three-valued, per the design's closure rule): (a) majority of
  the overhead closed by the selected fix, or (b) partial closure with
  numbers and documented follow-up options (no fix stacking), or (c)
  no-action ruling with numbers proving the residual is not actionable.
  Whichever lands is recorded in `bench-evidence.md` and bd rc-dkr1m;
  the mission park json carries the same numbers.
- Gates: fmt, clippy on touched crates `--all-targets -D warnings`,
  touched-crate test suites, `cargo doc` build if pub docs change, the
  12 xtask lints.

Risk budget: Java-side changes require a GraalVM native rebuild (docker,
~20 min) — acceptable; benchmark noise mitigated with criterion medians
and a quiet-host check, not p99-only reads.
