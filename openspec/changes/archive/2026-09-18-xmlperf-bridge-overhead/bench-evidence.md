# bench-evidence — xmlperf-bridge-overhead (bd rc-dkr1m)

## Baseline provenance (A/B BASELINE)

- Source commit: `de430583025b3e52a1eecc91c8bfa0bff41faffc`
- Builder image: `quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25`
  (image ID `sha256:6edcfc58e954…`)
- Binary SHA-256:
  `58cec7d58494625e570c11d0a9f356c9c35289cacbec23c36ca736cfb9f0ada4`
- Built: 2026-09-18T12:42:16Z via `cargo xtask build-xml-bridge`
  (gradle BUILD SUCCESSFUL 5m17s)
- Designation: **this binary is the A/B BASELINE.** The historical
  Aug-30 prebuilt is reproduction-check only.

## Reproduction check (historical binary)

Historical binary SHA-256:
`35f466cfa821201ded4628bc66dc75a82c8f9c2e00085dbcd4dec9ddd07b407e`
(run from `/tmp/xmlperf-repro/xml-bridge` via
`CAMEL_XML_BRIDGE_BINARY_PATH`; host quiet: loadavg `0.28 4.81 6.27`,
12 cores)

| phase | median |
|---|---|
| `xml_bridge/health_mtls` | 161.2 µs |
| `xml_bridge/transform_dispatch_mtls` | 258.9 µs |
| `xml_bridge/transform_mtls` | 1.0239 ms |
| `xml_bridge/proto_serde` | 241.5 ns |

**Reproduction criterion NOT confirmed** (tasks.md 1.2 step 3 expected
transform ≥ 2000 µs; measured 1024 µs). Finding, recorded honestly:

- The pure warm mTLS gRPC transform round-trip on a quiet HOST costs
  ~1.02 ms — not ≥2 ms. The era-2 "2.3–2.5 ms bridge tax" was computed
  from full ROUTE-TICK cells (rust-lib xslt-bridge m2 = 3.09 ms) vs
  in-process contender cells (JVM 0.74 ms), measured inside the
  benchmark container (benchmark-runner:v1 digest sha256:1247326c…).
- Therefore the era-2 gap decomposes as (at least): pure bridge call
  (~1.0 ms measured here) + rust-engine tick vs contender-tick
  difference + container-environment effects. The attribution of the
  whole 2.3–2.5 ms to per-call transport+dispatch is REFUTED for a
  quiet host; Task 1.3 records the formal hypothesis dispositions on
  the BASELINE binary with the design anchor.
- Environment deltas vs era-2: host vs container (cgroup/network ns),
  quiet load (0.28) vs 0.5–1.2. Historical-binary equivalence to the
  baseline rebuild rests on the MEASURED near-identity (historical
  1.0239 ms vs baseline 1.031 ms median transform, ~0.7% apart on the
  same quiet host), not on git-log claims (repo history begins at the
  2026-09-17 root import, which cannot witness Aug-30→Sep-17 source
  identity).

(Task 1.1 quick-run preview under build-decay load measured transform
2.05 ms — load-sensitive; all recorded evidence below uses quiet-host
runs only.)

## Task 1.3 — evidence run (BASELINE binary) + attribution

Quiet-host check: reps ran under 1-min load 0.65–4.02 (< 6 = 12 cores/2);
recorded per rep below. Rep 5 coincided with a load blip (2.66) and is
the outlier in dispatch/transform; the median-of-5 is robust to it and
the max−min noise bands conservatively carry it.

| phase | rep medians (µs) | median | N_p (max−min) |
|---|---|---|---|
| health_mtls | 164.14, 162.79, 162.76, 185.36, 170.63 | 164.1 | 22.6 |
| transform_dispatch_mtls | 256.37, 261.84, 255.36, 276.98, 488.15 | 261.8 | 232.8 |
| transform_mtls | 1026.6, 1031.0, 1047.6, 1029.9, 1274.4 | 1031.0 | 247.8 |
| proto_serde | 0.2756, 0.2501, 0.2759, 0.2914, 0.2713 | 0.276 | 0.041 |

Per-rep 1-min loadavg at invocation start: 0.65, 1.40, 4.02, 3.06,
2.66 (all < 6; rep 5's load blip aligns with its dispatch/transform
outliers).

Derived (design formulas):

| quantity | value µs | noise band | 3×band |
|---|---|---|---|
| X = T − Dph | 769.2 | N_X = 480.6 | 1441.8 |
| D = Dph − H | 97.7 | N_D = 255.4 | 766.2 |
| G = H − S | 163.9 | N_G = 22.6 | 67.9 |
| O = T − 740 | 291.0 | — | — |

Decision-tree row evaluation (design §Fix-selection):

- Row 1 (X-path): share `X ≥ 0.5·O` = 769.2 ≥ 145.5 ✓; noise
  `X > 3·N_X` = 769.2 > 1441.8 ✗ → does NOT fire.
- Row 2 (blocking dispatch): share `D ≥ 0.4·O` = 97.7 ≥ 116.4 ✗ →
  does NOT fire.
- Row 3 (transport stack): share `G ≥ 0.4·O` = 163.9 ≥ 116.4 ✓; noise
  `G > 3·N_G` = 163.9 > 67.9 ✓ → **FIRES**.
- Row 4 (client serde): `S ≥ 0.3·O` = 0.276 ≥ 87.3 ✗ → does NOT fire.
- Row 5: not reached (row 3 fired; single qualifying row → no tie
  algorithm needed).

**Selected row: 3** (transport-stack defect, non-blocking path).
Projected actionable reduction = G − N_G = 141.2 µs.

Headline attribution (quiet host, BASELINE binary): the warm bridge
call costs ~1.03 ms total, of which ~0.77 ms is in-bridge XML work
(≈ the 740 µs JVM in-process anchor — the native-image Saxon tax for
this warm identity transform is small), ~0.10 ms blocking dispatch,
~0.16 ms non-blocking transport floor. The total per-call overhead vs
the in-process anchor is O ≈ 291 µs — NOT the era-2 2.3–2.5 ms, which
was measured on full route-tick cells inside the benchmark container.

bd rc-dkr1m hypothesis dispositions:

1. "per-call stream setup" — NOT-SEPARABLE at this instrumentation
   level: G = 163.9 µs (56% of O) is the whole non-blocking floor
   (TLS record + h2 + stream setup + both protobuf hops); row 3's
   Branch-B reuse observation separates connection-level reuse only.
   Follow-up defined (Task 2.1 Branch B).
2. "Java dispatch queueing" — ELIMINATED as dominant: D = 97.7 µs
   (34% of O), below the row-2 share threshold.
3. "double serialization" — ELIMINATED: client serde = 276 ns
   (0.09% of O).

Confounds recorded (design attribution-arithmetic requirement):
dispatch error-response vs transform success-response shapes differ;
both carry the ~1.66 KB payload client→server; the D-path response is
tiny (error only) while the transform response carries the result —
the D quantity therefore underestimates a success-path dispatch by the
result-encode share, which lands in X (`encode` span would own it in a
row-1 sub-decomposition; not measured here because row 1 did not
fire).

## Task 2.1 — Branch B (row 3): connection-reuse observation

Held run: `--warm-up-time 2 --measurement-time 30`, loadavg at start
5.13 (< 6), port from `XMLPERF_BRIDGE port=43505`.

| signal | T0 | T1 (12 s later) |
|---|---|---|
| `ss -tnp state established '( sport = :43505 )'` client ephemeral port | 48540 (xml-bridge fd=33) | 48540 (same socket, same fd) |
| `nstat -az TcpPassiveOpens` (system-wide, secondary) | 20414 | 20414 (delta 0) |

The window spans all four benches (thousands of calls at 0.24–1.8 ms
each under load 5.13 — absolute times in this run are load-inflated
and are NOT evidence numbers; the reuse verdict is structural).

Verdict (design row-3 ordered rules): client port stable AND passive-
opens delta == 0 → **reuse correct → no-action exit for row 3.** The
single tonic channel reuses one persistent mTLS connection; no
per-call stream/connection setup defect exists at the connection
level.

Row-3 sub-attribution note: with reuse correct, the measured
G = 163.9 µs floor is persistent-connection transport (TLS record
layer + h2 framing + stream setup + both protobuf hops + Java event-
loop dispatch), inherent to the mTLS gRPC design (ADR-0036 posture).
No further decomposition is available without instrumenting inside
the TLS/h2 stack, which the design does not authorize.

## Task 2.2 — Branch 5: no-action ruling (closure verdict c)

Design row 3 fired on the evidence (G = 163.9 µs ≥ 0.4·O = 116.4 µs,
G > 3·N_G = 67.9 µs); its Branch-B sub-decomposition returned "reuse
correct" (client port stable, TcpPassiveOpens delta 0) — the row's
no-action exit. No product fix is justified by the measured evidence:

- The warm per-call overhead vs the in-process anchor is
  O ≈ 291 µs, not the era-2 2.3–2.5 ms. The bridge call itself
  (1.03 ms) decomposes as 0.77 ms XML work (≈ the JVM in-process
  anchor), 0.10 ms blocking dispatch, 0.16 ms persistent-connection
  mTLS transport floor.
- Connection reuse is structurally correct (no per-call setup).
- Dispatch queueing (D = 97.7 µs, 34% of O) and client serialization
  (0.28 µs, 0.09% of O) are both eliminated by the pre-committed
  thresholds.

Follow-up options (documented, not actioned — require owner
decisions):

1. Container-vs-host bridge tax quantification: re-run this
   decomposition bench inside the benchmark-runner container to
   attribute how much of the era-2 2.3–2.5 ms cell gap is container
   environment vs route-tick composition (candidate bd follow-up;
   mission order forbids canonical-suite runs, and this quantification
   belongs to the harness owner).
2. Era-2 cell-composition audit: the rust-lib xslt-bridge m2 cell
   (3.09 ms) vs this pure-call measurement (1.03 ms) leaves ~2 ms in
   route-tick + container + contender asymmetries outside the bridge;
   if the era-2 CAVEATS wording "bridge tax" matters for records, the
   analysis correction should be filed against the bench docs
   (on-disk, owner-owned).
3. Opt-in JVM-mode bridge binary: X = 769 µs is already at the
   in-process anchor, so a JVM-mode bridge would not reduce XML work
   meaningfully; not recommended.

## Task 2.3 — closure + regression baseline

Closure verdict: **(c) no-action ruling** (row 3 negative branch;
Branch 5 executed; no fixed build — A/B skipped per tasks.md 2.3 step
2). Closure arithmetic n/a (no post-fix transform to compare);
premise-level outcome: era-2 2.3–2.5 ms per-call bridge tax NOT
reproduced on a quiet host — measured pure-call overhead vs anchor
O = 291 µs with every bd hypothesis dispositioned by the pre-committed
algorithm.

Fixed-binary regression baseline (for future changes re-running this
bench on this host class; no fix landed, so the BASELINE table IS the
regression baseline):

| phase | baseline median µs | noise band (recorded spread) | regression threshold = max(15%, spread) |
|---|---|---|---|
| health_mtls | 164.1 | 22.6 | 24.6 µs |
| transform_dispatch_mtls | 261.8 | 232.8 | 232.8 µs |
| transform_mtls | 1031.0 | 247.8 | 247.8 µs |
| proto_serde | 0.276 | 0.041 | 0.041 µs |

A later evidence run flags a regression when a phase median exceeds
the baseline median + its threshold above (quiet-host rule applies to
both runs).
