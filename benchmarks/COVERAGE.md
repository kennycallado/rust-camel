# Coverage Matrix — rust-camel benchmark suite

> Living index of what the benchmark program has measured, is explicitly not measuring, or
> will measure conditionally. Each version of the suite adds cells; the matrix accumulates
> coverage over time. Reports are snapshots of which cells were measured in that release.
>
> **Authority note**: this matrix is subordinate to the actual harness output and published
> reports. Where the matrix and a report disagree, the report wins (and the matrix gets
> corrected).
>
> **Scope**: this matrix covers the **comparative cross-runtime** benchmark program under
> `benchmarks/`. The in-repo criterion micro-benchmarks at `crates/camel-bench/` measure
> internal regressions and are out of scope.

## Cell states (load-bearing)

Every cell has exactly one state. **No bare empty cells** — empty means incomplete process,
not "future work".

| Symbol | State | Meaning |
|---|---|---|
| **✓** | `measured` | Measured and published. Linked to the report version that contains it. |
| **✗** | `won't-measure: <reason>` | Explicitly out of scope. Reason must be specific (not "future"). Load-bearing: this is the structural proof the suite is a positioning instrument, not a highlight reel. |
| **?** | `open-if <condition>` | Will be measured if condition becomes true. Condition is the trigger, not a date. |

## Artifact set (per decision post-v2)

Four artifacts, each representing its framework's recommended production mode:

| Pair | Artifact | Mode | Notes |
|---|---|---|---|
| A | `camel-standalone-dsl` | JVM | Camel Main has no first-party native-image path; JVM is its real modal choice. |
| A | `camel-quarkus-dsl-native` | Native | Quarkus's marketed mode (Substrate VM). |
| A | `rust-camel-lib` | Native (Rust) | Public `CamelContext` API, programmatic routes. |
| B | `rust-camel-cli` | Native (Rust) | CLI bootstrap with YAML route file. |

**Removed (kept in appendix as mode-delta reference)**: `camel-quarkus-dsl` (JVM mode) and
`camel-quarkus-yaml` (JVM mode). Their numbers prove the native-vs-JVM mode effect; they
are not part of the head-to-head artifact set going forward. **v3.5 made this harness-official**:
both subprojects dropped from `settings.gradle.kts` `include` lists (directories retained as
source holders for the `-native` variants via Gradle `sourceSets.srcDir`). v1/v2 published
reports still contain the JVM numbers as historical mode-delta reference.

**Camel standalone YAML** (`camel-standalone-yaml`) status: **measured in v3+** (Pair B JVM
baseline). Confirmed in v3.5 — the Pair A/B split survives the v3 metric expansion because
the YAML-parse overhead (~5ms) is a load-bearing measurement for the dev-inner-loop ICP.

**Bridge scenarios asymmetry** (T4a/T4b only — 4 artifacts per bridge scenario, not 6):
the bridge tax is authoring-format-invariant (gRPC subprocess + byte-pinned XML payload are
identical regardless of route authoring format; T1/T2 already isolate YAML-parse overhead).
Adding `camel-standalone-yaml` + `camel-quarkus-yaml-native` for bridges would re-measure a
known-invariant quantity — recorded as `won't-measure` per e_opus round-5 verdict.

## Scenario axis (rows)

Scenarios are route shapes that test a specific dimension of the framework. Each scenario
is a YAML or programmatic route, identical logically across the 4 artifacts.

| ID | Scenario | Bridge? | Status |
|---|---|---|---|
| **T1** | `timer -> log` (trivial) | no | fixture exists (v1) |
| **T2** | `timer -> set_body -> set_header -> filter -> choice -> log` (5 EIPs) | no | fixture exists (v2) |
| **T3** | HTTP server: `http POST /bench -> log -> respond 200` | no | proposed v3 |
| **T4a** | `timer -> set_body(XML) -> xslt(transform) -> log` | **YES** (xml bridge) | proposed v3 |
| **T4b** | `timer -> set_body(XML) -> validator(XSD) -> log` | **YES** (xml bridge) | proposed v3 |
| **T4c** | `timer -> jms:queue:bench -> log` | **YES** (jms bridge) | open-if: T4a/T4b tax curve justifies more bridge characterization |
| **T4d** | `timer -> cxf:bean -> log` | **YES** (cxf bridge) | open-if: SOAP user demand |
| **T5** | Kafka consumer: `kafka:bench-topic -> filter -> log` | no | open-if: streaming ICP emerges |
| **T6** | SQL polling: `sql:SELECT * FROM bench -> log` | no | open-if: batch ICP emerges |
| **T7** | Composite multi-hop route (mix of EIPs + 1 bridge mid-route) | mixed | open-if: composite-route realism becomes a decision input (likely v5+) |

## Metric axis (columns)

| ID | Metric | What it measures | Statistical regime |
|---|---|---|---|
| **M1** | Cold-start + RSS | Wall-clock from process launch to "ready" marker; peak RSS via GNU time | n=50 + 3 warmup, nearest-rank p95. Process launch per sample. |
| **M2** | Warm p99 latency | Per-request latency after warmup (p50/p95/p99) | n≥10,000 messages after 30s warmup. Per-message sampling. |
| **M3** | Sustained throughput | Messages/sec over 60s steady-state | Single 60s run, msgs/sec computed. |
| **M4** | Memory growth under load | RSS delta from start of load to end | Periodic sampling during 60s M3 run. |

M2/M3 require a load generator (separate process) and steady-state detection — harness
complexity approximately doubles vs M1-only. M3/M4 are typically measured together (same
load run).

## The matrix

Each cell shows state + version + report link (if measured).

| | M1 cold-start + RSS | M2 warm p99 | M3 sustained throughput | M4 memory growth |
|---|---|---|---|---|
| **T1** timer+log | ✓ v1+v2 — [v1 report](2026-07-18-startup-minimal-benchmark.md), [v2 report](2026-07-18-benchmark-v2.md) | ✓ v3.5 — baseline warm | ✗ won't-measure: timer-driven, fixed throughput (100 msgs/sec at 10ms period) | ✗ won't-measure: same |
| **T2** 5 EIPs (no-bridge) | ✓ v2 — [v2 report](2026-07-18-benchmark-v2.md) | ✓ v3.5 | ✗ won't-measure: timer-driven, fixed throughput | ✗ won't-measure: same |
| **T3** HTTP server (no-bridge) | ✓ v3.5 — listener spin-up cost | ✓ v3.5 — request-serving viability | ✓ v4 (platform-http) — 78,628 req/s (rust-camel-lib), 37,713 (Quarkus native), 66,808 (Camel JVM); 1.18× rust-camel vs JVM, 2.08× rust-camel vs Quarkus native, 1.77× JVM vs Quarkus native | ✓ v4 (platform-http) — RSS delta (median, 5 rounds): rust-camel-lib +52 KiB, rust-camel-cli +44 KiB, camel-standalone-dsl +540 KiB, camel-standalone-yaml +448 KiB, camel-quarkus-dsl-native −1,672 KiB (RSS shrinks — GC releasing), camel-quarkus-yaml-native −284 KiB |
| **T4a** XSLT bridge (~10ms Java work) | ? v3 — bridge process spawn cost | ✓ v3 — [v3 report](../docs/benchmarks/2026-07-21-benchmark-v3.md) (2 pairs: standalone + Quarkus native, both Saxon-HE) | ✗ won't-measure: throughput dominated by XSLT engine, not bridge overhead | ✗ won't-measure: same as M3 |
| **T4b** XSD validation bridge (<1ms Java work) | ? v3 — bridge process spawn cost | ✓ v3 — [v3 report](../docs/benchmarks/2026-07-21-benchmark-v3.md) (2 pairs) | ✗ won't-measure: same rationale as T4a | ✗ won't-measure: same |
| **T4c** JMS bridge | ? v4-if: T4a/T4b tax curve motivates more bridge coverage | ? v4-if: same | ✗ won't-measure: JMS broker dominates | ✗ won't-measure: same |
| **T4d** CXF bridge (SOAP) | ? v5-if: SOAP user demand emerges | ? v5-if: same | ✗ won't-measure | ✗ won't-measure |
| **T5** Kafka consumer | ? v4 — Kafka is widely used; bootstrap cost matters | ? v4 — broker round-trip vs in-process comparison | ? v5-if: streaming ICP emerges | ? v5-if |
| **T6** SQL polling | ? v4-if: SQL users emerge as audience | ? v4-if: same | ✗ won't-measure: DB dominates | ✗ won't-measure |
| **T7** Composite multi-hop | ? v5-if: composite-route realism becomes decision input | ? v5-if: bridge tax composition matters | ? v5-if | ? v5-if |

**Cumulative measured cells**: 10 — T1×M1/M2, T2×M1/M2, T3×M1/M2/M3/M4, T4a×M2, T4b×M2. v4 added T3×M3+M4 (sustained throughput + memory growth, 6 http-server artifacts).

## ICP layer (derived, not asserted)

ICPs are **derived from measured cells**, not asserted upfront. Each ICP must cite the cells
that support it. ICPs without supporting cells are "candidates" — not yet claimable.

| ICP | Supporting cells | Status |
|---|---|---|
| **Dev inner-loop / CI** | T1×M1, T2×M1 (rust-camel 8ms vs Java 18-300ms cold-start) | ✓ **Validated** — pure M1 ICP, survives all critiques. |
| **K8s sidecar in request path** | T3×M2 (HTTP server warm p99 — rust-camel in request path) | ? **Candidate** — pending v3 M2 data on HTTP server |
| **Scale-to-zero / function cold-start** | T1×M1 + T1×M2 (cold-start + warm p99) | ? **Candidate** — explicitly data-scoped to M1 alone in v1; re-opens with M2 (per `benchmarks/CONTEXT.md` §5) |
| **Mostly-no-bridge integration** | T3×M2 + T4a/b×M2 (bridge tax curve defines the boundary) | ? **Candidate** — pending v3 bridge tax curve; emergent from no-bridge vs bridge comparison |
| **Request-serving / API gateway** | T3×M2 + T3×M3 (HTTP server warm latency + saturation) | ✓ **Validated** — v4 M3 measured: rust-camel-lib 78,628 req/s vs Camel JVM 66,808 (1.18×) vs Quarkus native 37,713 (2.08×). Native trades ~1.8× throughput for ~7× lower RSS and ~30× faster cold-start. Ratios are single-host descriptive (see v4 report limitations). |
| **Batch processing** | T6×M3 or T5×M3 | ✗ **Not yet claimable** — no M3 data; batch ICP is M3-gated |
| **Edge computing** | (would need: T1×M1 + binary size + edge-deployment shape) | ✗ **Rejected** (v1, on M1+binary-size data) — CLI 86MB too fat for typical edge; rust-camel-lib 4.8MB fits but edge rarely needs integration frameworks. Re-opens only if a new edge-shaped artifact emerges (e.g., minimal-feature CLI build). |

## Decision triggers (when to measure `open-if` cells)

The matrix is **decision-triggered, not completeness-triggered**. Each `open-if` cell names
the condition that would make measuring it worthwhile:

- **T4c (JMS bridge)**: measure if T4a/T4b tax curve shows bridge tax is variable enough
  that another data point would refine the curve.
- **T4d (CXF bridge)**: measure if SOAP user demand emerges as a real audience signal
  (GitHub issue, blog comment, sales conversation).
- **T5 (Kafka)**: measure if streaming/ingest ICP becomes a target (likely requires blog
  post or user request to motivate).
- **T6 (SQL)**: measure if SQL-heavy audience emerges (likely a user request).
- **T7 (Composite)**: measure if bridge tax composition (position in route, hit rate)
  becomes decision-relevant — likely v5+ when single-component cells are stable.
- **M3 cells**: measure if an ICP cares about sustained throughput (batch, request-serving
  saturation).
- **M4 cells**: measure if memory growth becomes a differentiator (long-running sidecars).

## Process for adding a cell

1. Cell is currently `open-if <condition>` or `won't-measure: <reason>`.
2. Condition triggers (decision input needed, condition met).
3. Spec written referencing the cell(s) to fill.
4. Harness + fixture work.
5. Measurement run.
6. Cell state transitions to `measured` with version + report link.
7. ICP layer re-evaluated — does this cell validate, kill, or open a candidate ICP?

## Process for `won't-measure` (just as load-bearing as `measured`)

`won't-measure` cells are the structural proof this is a positioning instrument. Each must
have a specific reason (not "future"). Examples in this matrix:
- **T4a/T4b × M3/M4**: "throughput/memory dominated by XSLT engine or Java binary, not
  bridge overhead — measuring tells us about XSLT, not about rust-camel".
- **T4c/T4d × M3/M4**: same rationale as T4a/T4b.
- **T6 × M3/M4**: "DB dominates — measuring tells us about Postgres, not about rust-camel".

If a `won't-measure` reason later invalidates (e.g., a user requests the specific data
point), the cell transitions to `measured` via the standard process.

## Versioning

Each coverage release (v1, v2, v3, ...) corresponds to a published report at
`docs/benchmarks/YYYY-MM-DD-benchmark-vN.md`. Reports are immutable once published. The
matrix is the **index** across all reports — it shows the cumulative state of the program,
not any single report.

When a new version is published:
1. Update the relevant cells' state to `✓ vN — link`.
2. Re-evaluate ICP layer — does new data validate, kill, or open candidates?
3. Update `open-if` conditions if vN data changes them.
4. Note in the vN report which cells were added and which ICPs shifted.

## Cross-references

- Methodology + domain language: `benchmarks/CONTEXT.md`
- v1 report: `docs/benchmarks/2026-07-18-startup-minimal-benchmark.md`
- v2 report: `docs/benchmarks/2026-07-18-benchmark-v2.md`
- v2 spec: `docs/superpowers/specs/2026-07-18-rc-p9ki-benchmark-v2-design.md`
- v2 plan: `docs/superpowers/plans/2026-07-18-rc-p9ki-benchmark-v2.md`
- Consultation (initial, 6 Qs): `docs/benchmarks/consultation-v3-direction-2026-07-18.md`
- Consultation (follow-up, Q7-Q11): `docs/benchmarks/consultation-v3-followup-2026-07-18.md`
- Harness: `benchmarks/harness/run.sh`
- Spike results (native-image validation): `benchmarks/spike-results.md`
