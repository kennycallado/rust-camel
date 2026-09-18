# xml-bridge-performance Specification (delta)

## ADDED Requirements

### Requirement: Bridge dispatch decomposition benchmark

The workspace MUST provide a criterion benchmark that spawns the real
xml-bridge native binary through the production `camel-bridge` spawn
path and measures warm per-call latency for four phases:
`xml_bridge/health_mtls` (non-blocking dispatch floor),
`xml_bridge/transform_dispatch_mtls` (same payload as a real transform
sent with an unknown stylesheet id — exercises the blocking-dispatch and
payload path without XML work), `xml_bridge/transform_mtls` (full
transform round-trip), and `xml_bridge/proto_serde` (client-side-only
protobuf encode/decode).

#### Scenario: benchmark reports all four phase medians

- **Given** an xml-bridge native binary resolvable via env override or
  `{workspace_root}/bridges/xml/build/native/xml-bridge`
- **When** an evidence run of at least 5 invocations of `cargo bench -p
  camel-bench --bench xml_bridge_decompose` completes on a quiet host
  (1-min loadavg below half the core count at every invocation,
  recorded in the evidence doc)
- **Then** the evidence doc contains, for each of the four phases, the
  median over the ≥5 invocation medians, plus the per-repetition
  median spread, each measured after the gRPC channel and stylesheet
  cache are warm

#### Scenario: absent binary skips loudly in developer runs

- **Given** no xml-bridge binary is resolvable and the env var
  `XMLPERF_EVIDENCE_RUN` is unset
- **When** the benchmark is invoked
- **Then** it registers no measurements, prints a single skip line
  naming the missing path and the env override, and exits without error

#### Scenario: evidence runs fail when the binary is absent

- **Given** no xml-bridge binary is resolvable and the env var
  `XMLPERF_EVIDENCE_RUN` is set
- **When** the benchmark is invoked
- **Then** it exits non-zero with an error naming the missing path, so
  evidence collection cannot silently proceed without the bridge

### Requirement: Overhead attribution is reproducible

Evidence produced by the decomposition benchmark MUST be reproducible:
the binary provenance and the attribution arithmetic are recorded with
the numbers so any conclusion can be re-derived.

#### Scenario: evidence doc records provenance and arithmetic

- **Given** an evidence run of the benchmark
- **When** its results are recorded in the change's `bench-evidence.md`
- **Then** the doc states the binary's SHA-256 and how it was built
  (source commit + builder image), the measured medians for all four
  phases, the derived quantities (dispatch-payload delta =
  `transform_dispatch − health`; XML-work estimate =
  `transform − transform_dispatch`; gRPC stack estimate =
  `health − proto_serde`), and the in-process anchor used as
  denominator for the relative-overhead claim

#### Scenario: before/after comparisons use same-toolchain binaries

- **Given** a fix changes bridge or client code
- **When** before/after numbers are compared
- **Then** both binaries are built from the same builder image and
  build flags, differing only by the fix commit, and both provenances
  are recorded in `bench-evidence.md`; comparisons against the
  historical prebuilt binary are labeled as reproduction checks only,
  never as the A/B pair

#### Scenario: baseline-relative regression criterion

- **Given** a fixed binary whose evidence run is recorded
- **When** a later change re-runs the benchmark on the same host class
- **Then** a phase median that regresses beyond the noise band defined
  in `bench-evidence.md` (per-phase: max of 15% or the recorded
  per-repetition spread) over the recorded fixed-binary baseline is
  flagged as a regression in the change's evidence doc
