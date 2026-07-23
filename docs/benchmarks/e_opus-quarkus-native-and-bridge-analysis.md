# Deep Analysis: Quarkus Native Throughput + Bridge Subsystem Tax

> **Status note (post-experiment)**: This analysis was written as a
> pre-experiment hypothesis + action plan. Experiments #1 and #2 were
> subsequently run. **Results**: Experiment #1 (platform-http swap)
> confirmed the direction (+22% improvement on native) but **did not
> close the gap to the predicted 1.2–1.5×** — the residual is 1.77×.
> Experiment #2 (Serial GC A/B) **falsified** Serial GC as a cause
> (JVM-Serial ≈ JVM-G1). Experiment #3 (flamegraph) remains deferred.
> Experiment #4 (PGO) is **blocked** — PGO is GraalVM EE-only on both
> Mandrel CE and GraalVM CE. Read the predictions below as pre-experiment
> hypotheses, some confirmed and some not, not as verified findings.

_e_opus, ULTRATHINK cross-crate analysis. Sources: `quarkus-native-throughput-diagnosis.md`,
`docs/benchmarks/2026-07-22-benchmark-v4-m3-m4.md`, `benchmarks/scenarios/http-server/camel-quarkus/*`,
ADR-0036, `crates/services/camel-bridge/src/*`, `crates/components/camel-xslt/src/*`._

---

## Quarkus Native Throughput Analysis

### Root Cause (refined)

The existing 4-factor diagnosis is **architecture-only, unprofiled**, and it front-loads Serial GC as
"primary suspect" on weak evidence. Reading the fixtures changes the ranking. The dominant factor is
almost certainly **the HTTP stack, not the GC.**

The smoking gun is in `camel-quarkus-dsl/build.gradle.kts`:

```
implementation("org.apache.camel:camel-jetty:4.8.0")
```

with a comment admitting the design compromise:

> _camel-quarkus 3.20's `camel-quarkus-http` extension is CLIENT-side (HTTP Client 5.x), not
> server-side... Pulling `camel-jetty` directly is the documented workaround._

And the route is `from("jetty:http://0.0.0.0:8080/bench")`. This means the Quarkus **native** image is
running a **raw, embedded, thread-per-request blocking Jetty 4.8.0 server, AOT-compiled under Substrate
VM**. This is the worst-case combination for native images, for three compounding reasons:

1. **Thread-per-request blocking model.** Every request parks an OS thread. Under Substrate, thread
   scheduling, park/unpark, and the blocking NIO selector path were never PGO-tuned the way HotSpot's
   JIT tunes them after warmup. The JVM standalone runs the *same* Jetty but gets C2 JIT optimization of
   the hot accept/dispatch loop — the native image does not.
2. **Substrate runs Jetty's reflective/config machinery interpreted-ish.** Jetty was not written to be
   AOT-compiled; its thread pool sizing, buffer pooling, and connector setup lean on runtime class
   loading that Substrate handles conservatively.
3. **This is NOT the Quarkus "fast path."** Quarkus native's headline throughput numbers come from
   `quarkus-rest`/`quarkus-vertx-http` (Vert.x + Netty, reactive, zero-copy, event-loop). The fixture
   bypasses all of it. **We are benchmarking "Camel-on-Jetty compiled to native," not "Quarkus native."**

**Revised confidence ranking:**

| Factor | Old rank | New rank | Confidence | Reasoning |
|---|---|---|---|---|
| Jetty thread-per-request under Substrate (no JIT on hot loop) | #3 | **#1** | **High (0.7)** | JVM gets C2 on the same Jetty; native does not. Blocking model is pathological under AOT. |
| Serial GC (Mandrel CE) | #1 | **#2** | Medium (0.4) | Real, but `-R:MaxHeapSize=512m` already bounds pause length; RSS delta is ~536 KiB median — allocation pressure at saturation is modest for a `setBody("pong")` route. |
| O2 vs O3/PGO | #4 | #3 | Low (0.2) | Second-order. PGO helps 5–15% typically, not 2×. |
| Per-request stdout | resolved | resolved | — | Already removed (commit 003523d7). |

The 2.06× JVM-vs-native gap is **too large for GC alone** on a route that allocates almost nothing per
request. A single-threaded Serial GC only hurts when there is sustained allocation to collect;
`setBody(constant("pong"))` produces near-zero garbage per request. That points the finger squarely at
the **request-dispatch hot path** (Jetty-on-Substrate), not the collector.

### Concrete Action Plan

Ordered by expected-impact-per-effort. Each is a falsifiable experiment.

1. **[Highest impact] Switch the native fixture to Vert.x/`platform-http`.**
   Replace `camel-jetty` + `from("jetty:...")` with `camel-quarkus-platform-http` +
   `from("platform-http:/bench")`. `platform-http` binds to Quarkus's managed Vert.x/Netty reactive
   server — the *actual* native fast path. **Expected: closes most of the gap; native should land within
   1.2–1.5× of JVM, possibly beating it.** This is the single most important experiment. It also makes
   the comparison *fair*: the JVM standalone should keep Jetty (documenting the asymmetry) OR both should
   move to their idiomatic best stack. State the choice explicitly in the report.

2. **[Isolation, cheap] GC A/B on the JVM side.** Force the JVM standalone to `-XX:+UseSerialGC` and
   re-measure. If JVM-Serial ≈ JVM-G1, GC is exonerated as the dominant factor and the diagnosis's #1
   claim is falsified. This is a 5-minute experiment that resolves the biggest open question (matches
   spec issue **I3: "Serial-GC claim lacks profiling evidence"**).

3. **[Confirmation] Capture a native flamegraph under load.** `perf record`/`async-profiler` on the
   native process during M3 steady-state. If the hot frames are Jetty selector/thread-park, factor #1
   is confirmed. If they are GC/safepoint, factor #2 wins. This is the profiling the diagnosis itself
   flags as "deferred to v5+." Do it now — it is the only way to end the speculation.

4. **[Build tuning, moderate] Add `-O3` and PGO.** `quarkus.native.additional-build-args=-O3` plus a
   two-pass PGO build (`--pgo-instrument` then `--pgo`). Expected 5–15%. Worth doing *after* the Vert.x
   switch, not before — optimizing the wrong stack is wasted effort.

5. **[Do NOT bother] `-march=native`.** Marginal for I/O-bound HTTP dispatch; and it makes the image
   non-portable, which conflicts with the K8s-sidecar ICP. Skip.

6. **[Reject] G1GC via `-R:+UseG1GC` on Mandrel CE.** It will not link — G1 is genuinely EE-only in
   Mandrel CE; the flag is silently ignored or errors. **GraalVM CE proper *does* ship G1 for native
   images** — so if GC turns out to be dominant (experiment #2/#3), the fix is *switch the builder from
   Mandrel CE to GraalVM CE*, not a flag. That is the real answer to the "would GraalVM CE give us G1?"
   question: **yes, GraalVM CE has G1 for native-image; Mandrel CE does not.**

### Feasibility Assessment

**Can Quarkus native be made competitive (within 1.5× of JVM)? Almost certainly yes — but only by
fixing the fixture, not by tuning knobs.** The current numbers do not measure "Quarkus native"; they
measure "Camel-on-blocking-Jetty compiled to native," which is a known-bad configuration. With
`platform-http` (Vert.x/Netty), community reports and Quarkus's own benchmarks routinely show native
*matching or beating* JVM steady-state throughput while using a fraction of the memory.

**Ceiling:** With the reactive stack + G1 (via GraalVM CE) + PGO, native could plausibly reach
**0.9–1.1× of Camel JVM** — i.e., roughly at parity. It is unlikely to beat *rust-camel* (no GC, no
managed runtime), but it should stop being an embarrassment. **The 0.42× result is a fixture artifact,
not a fundamental Substrate limitation.**

**Honest caveat:** everything above marked "expected" is a hypothesis until experiments #1–#3 run. The
current diagnosis's core sin is asserting a root cause without profiling. Do not repeat it — ship the
flamegraph.

---

## Bridge Subsystem Parallel

### Bridge Tax Estimation

**The bridge is architecturally the *opposite* of the Quarkus-native problem, and it is already
well-optimized.** The task framed both as "boundary-crossing tax," but the mechanisms differ
fundamentally:

- **Quarkus native tax is per-request and unavoidable** within the process: every request pays the
  GC/dispatch cost on the hot path.
- **Bridge tax is per-call but amortized and off the steady-state hot path** for the HTTP benchmark
  (which makes no bridge calls). Reading the code, the bridge is engineered to *minimize* the
  crossing cost:

  1. **Long-lived multiplexed channel.** `BridgeState::Ready { channel }` (`camel-xslt/producer.rs`)
     holds a persistent tonic `Channel`. tonic runs HTTP/2 — **one connection, many concurrent streams,
     no per-call TLS handshake.** The 10×100ms retry loop in `channel.rs` is startup-only, not per-call.
  2. **Compile-once, cache.** `StylesheetCache` (LRU, 256 entries) compiles each XSLT stylesheet *once*
     on the Java side and reuses the compiled `Templates`. Per request it sends only
     `TransformRequest{stylesheet_id, document, params}` — an ID reference, not the stylesheet.
  3. **Bytes, not objects.** The proto (`xml_bridge.proto`) passes `bytes document` / `bytes result`.
     Protobuf framing over a Unix-local TCP loopback (127.0.0.1) is a memcpy + syscall, not a
     serialization-heavy JSON round-trip.

  So the real per-call bridge tax ≈ **(protobuf encode) + (loopback write syscall) + (HTTP/2 framing) +
  (Java-side Saxon transform) + (return path)**. The *transform itself* (Saxon) dominates by 1–3 orders
  of magnitude over the IPC framing for any non-trivial document.

### Comparison to Quarkus overhead

| Axis | Quarkus native tax | Bridge tax |
|---|---|---|
| Where it lands | Every HTTP request, on the hot dispatch path | Only routes that use XSLT/XSD/JMS |
| Amortizable? | No — intrinsic to the runtime | Yes — connection reuse + compile cache |
| Handshake per call? | N/A | **No** (HTTP/2 multiplex, handshake once at startup) |
| Dominant cost | Jetty-on-Substrate dispatch (hypothesis) | The Java engine work (Saxon/JAXP), not the IPC |

The v4 report's decision to mark T4a/T4b XSLT throughput **"won't-measure — throughput dominated by XSLT
engine, not bridge overhead"** is **defensible but untested, and I would challenge it.** It is *probably*
true for large documents (Saxon dominates), but for **small documents at high request rates** the fixed
per-call IPC cost (syscall + framing + tokio task hop + Java thread hop) can become the dominant term.
That is exactly the regime where a "bridge tax" would show up — and it is unmeasured.

### Should We Benchmark the Bridge?

**Yes — but a narrow, targeted micro-benchmark, not a full M3 cell.** The question worth answering is not
"how fast is XSLT" (that's Saxon's problem) but **"what is the fixed per-call IPC floor of the bridge?"**

Proposed fixture — **"bridge null-transform" / IPC-floor probe:**

- Route: `from(http) → to(xslt:identity.xsl) → setBody`, where `identity.xsl` is the identity transform
  on a **tiny (~100 byte) document**. This makes Saxon's work negligible so the measured latency is
  ≈ the IPC floor.
- Compare three points:
  1. **Direct in-process baseline:** same route with XSLT replaced by a no-op Rust processor (measures
     the route framework overhead with zero bridge).
  2. **Bridge identity transform** (the probe above).
  3. **Bridge with a realistic 10 KB document** (measures where Saxon starts to dominate).
- **Metric:** `p50/p99 added latency` = (2) − (1), and throughput ceiling of the bridge path.
- **Expected result:** a fixed per-call cost on the order of **tens of microseconds** (loopback RPC +
  two runtime hops). If it is much higher, that's a finding and an optimization target.

This is cheap (reuses the existing `xslt-bridge` scenario dir) and it converts the "won't-measure"
assumption into evidence.

### Optimization Opportunities

Ranked by value, **only pursue if the IPC-floor probe shows the tax is material:**

1. **Unix Domain Socket instead of TCP loopback.** ADR-0036 uses `https://127.0.0.1:{port}` (TCP + TLS).
   A UDS (`http+unix`) skips the TCP/IP stack and, more importantly, **removes the need for TLS
   entirely** — a UDS with `0700` perms is already restricted to the same UID, satisfying the ADR-0033
   threat model (local process sniffing) via filesystem permissions rather than mTLS handshakes. This
   removes both the TCP overhead *and* the per-connection TLS crypto. tonic supports UDS transport.
   **This is the highest-value bridge optimization** and it *simplifies* the security story rather than
   weakening it. (Caveat: the "shared network namespace container" case in ADR-0036 needs the socket on
   a shared volume with correct perms — verify against the deployment model.)
2. **Batch/pipeline requests.** For fan-out routes calling the bridge N times, HTTP/2 already multiplexes
   concurrent streams — ensure the producer issues concurrent RPCs rather than awaiting serially.
3. **[Do NOT do] JNI instead of gRPC.** Tempting for "zero-copy," but it **collapses the process
   isolation** that is the entire point of the bridge (crash isolation, independent GC, security
   boundary, the ability to run the Java engine as an untrusted subprocess). JNI would put Saxon's heap
   *inside* the rust-camel process — a GC pause or a Saxon segfault would now take down the router. The
   process boundary is a feature, not a bug. **Reject.**
4. **[Do NOT do] Shared memory.** Same objection as JNI plus lifecycle/cleanup complexity. The
   protobuf-over-UDS path is 95% of the benefit at 5% of the risk.

---

## Strategic Recommendation

**Invest in Quarkus first (higher ROI, resolves a live embarrassment), then a cheap bridge probe.**

1. **Fix Quarkus native (do this):** The 0.42× number is misleading and, if published, damages
   credibility ("native slower than JVM" is a red flag reviewers will seize on). The fix — swap
   `camel-jetty` for `camel-quarkus-platform-http` (Vert.x) — is a **~20-line fixture change** that
   likely recovers most of the 2× gap. Pair it with the JVM-Serial-GC A/B (#2) and a flamegraph (#3) so
   the *next* report states a **profiled** root cause instead of a suspected one. This directly closes
   spec issue I3. **Effort: ~1 day. Impact: high — turns a misleading result into a defensible one.**

2. **Add the bridge IPC-floor probe (do this, cheaper):** Convert the "won't-measure" assumption into a
   real number. Reuses existing `xslt-bridge` fixtures. If the floor is tens of µs (likely), you get a
   *positive* story ("bridge adds <50µs/call, negligible vs engine work"). If it's larger, you found an
   optimization target and the UDS work becomes justified. **Effort: ~half a day. Impact: medium — closes
   an evidentiary gap and de-risks the architecture claim.**

3. **UDS migration (conditional):** Only if the probe shows the tax is material. It is the right
   long-term move (removes TLS handshake cost, simplifies security), but it touches ADR-0036 and the
   security model — don't do it speculatively. **Gate on probe results.**

**Do not** chase JNI/shared-memory (destroys isolation), `-march=native` (non-portable, marginal), or
G1-flag-on-Mandrel-CE (won't link). The two concrete wins are: **Vert.x fixture** for Quarkus, and a
**one-shot bridge probe** to replace an untested assumption with data.

### The one-line synthesis

> The Quarkus number is a fixture bug (blocking Jetty on Substrate), not a Substrate limitation; the
> bridge is the *well-designed* version of the same boundary problem (persistent HTTP/2 channel +
> compile cache) and its only untested risk is the fixed per-call IPC floor — measure it with a
> null-transform probe, and if it bites, move to a Unix domain socket.
