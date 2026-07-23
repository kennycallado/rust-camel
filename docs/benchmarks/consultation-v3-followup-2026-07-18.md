# Consultation follow-up — coverage matrix + bridge-aware v3 cells (2026-07-18)

**Nature**: continuation of prior consultation (same session). Strategic frame now settled
(survey framing, drop-JVM-from-headline, coverage matrix, bridge-awareness). This pass
validates the meta-structure and the first cells.

**New inputs read this pass**: ADR-0036 (bridge IPC mTLS), bridge IPC design spec, bridge
caller-migration site list, `crates/services/camel-bridge` layout.

**Correction to my own prior mental model, up front**: the bridge is not "gRPC ~1ms RTT on
localhost." It is **mTLS gRPC over HTTP/2 in Quarkus unified mode** (ADR-0036 Decision;
design spec "Architecture: Unified Quarkus mode"), talking to a **separate Quarkus native
process**, not an in-process JNI call. That changes the Q9 cost model materially — see below.

---

### Q7: Is the coverage matrix framing sound?

**Reasoning**: The matrix framing is not just sound — it is the *correct* structural
consequence of adopting survey over argument (prior Q6). Once you stop asserting ICPs
up-front, you need a persistent artifact that records "what has been measured" independent of
"what argument we're making this quarter." That is exactly what `COVERAGE.md` is. It also
matches your existing house style: the parity analysis is maintained as a living versioned
doc with change-log appendices (v1→v7), not as a series of disconnected papers. The coverage
matrix is the benchmark-suite analog of that.

The three failure modes you named are the right three, and each has a known antidote:

1. **Matrix-as-never-completing-TODO.** The antidote is to make cells *decision-triggered*,
   not *completeness-triggered*. A cell exists in the matrix in one of three states:
   `measured` (with version + report link), `won't-measure` (with a one-line reason), or
   `open-if` (with the condition that would make it worth measuring). A cell is NOT a task
   just because it's empty — it's a task only when a pending decision needs it. This kills
   the "must fill every cell" gravity that turns matrices into guilt.

2. **ICP maintenance harder over time.** It's actually *easier* under a matrix, because the
   matrix decouples "metric×scenario coverage" from "ICP claims." ICPs become a thin
   *derived* layer: each named ICP cites the cells that justify it. When a new cell lands,
   you re-derive which ICPs it strengthens/weakens. Without the matrix, ICP claims float free
   of their evidence (which is exactly how v1/v2 ended up asserting the sidecar ICP on the
   wrong metric — prior Q4).

3. **Won't-measure cells: yes, make them explicit.** Empty cells are ambiguous — is it
   unmeasured-because-uninteresting or unmeasured-because-not-yet? An explicit `won't-measure:
   <reason>` cell is a *finding* ("we deliberately don't benchmark M3×CXF because SOAP is
   never throughput-bound"). Explicit non-coverage is the single strongest signal that the
   suite is a positioning instrument and not a highlight reel — it's the structural version of
   your willingness-to-publish-losses.

**Guidance**: Adopt the matrix. Enforce three cell states (`measured` / `won't-measure` /
`open-if <condition>`), never a bare empty cell in a decided row. Make each named ICP a
*derived* section that cites cells, so ICPs can't drift from evidence. The explicit
`won't-measure` cells are load-bearing, not clutter — keep them.

**Open questions back**: Who owns the "open-if condition" per empty cell? If nobody writes the
condition, the cell silently becomes a TODO again. I'd require that no cell enters the matrix
without either a measurement, a won't-measure reason, or a named triggering condition.

---

### Q8: Are the v3 cells the right first cells?

**Reasoning**: The *shape* of the selection is right: one cheap baseline, one no-bridge
head-to-head that could birth an ICP, one bridge that tests the willingness-to-lose. That's a
well-formed first release — it spans the honest range from "where we win" to "where we might
lose." Three specific refinements.

**HTTP-server is the right no-bridge component, but for a subtler reason than "most common."**
HTTP-*server* is the one component where M2 (warm p99) is *the* ICP-defining metric, because
request-serving is the ICP that emerges from warm-latency data (prior Q4). HTTP-*client*,
Kafka, and SQL are all *outbound* — their latency is dominated by the remote system, so M2
would measure the broker/DB, not rust-camel. HTTP-server puts rust-camel *in the request
path* where its own warm p99 is the dependent variable. Keep HTTP-server; it's not just
common, it's the one that adjudicates the request-serving candidate.

**One bridge is enough for v3, but pick the right one — and XSLT may be the wrong single
pick.** The bridge tax is a *ratio* (bridge overhead ÷ Java-side work), so a single bridge
only characterizes one point on that curve. XSLT (~10ms Java work) is the *favorable* end —
the bridge tax looks small there. If you measure only XSLT, you publish the flattering bridge
number and hide the ugly one. XSD validation (<1ms Java work) is where the bridge tax is
worst (could 2-3× the latency). For a positioning instrument, **measure the worst case first
or alongside** — otherwise the "we publish losses" claim is undercut by having picked the
loss that hurts least. I'd measure *two* bridge points (XSLT favorable + XSD worst-case) to
show the tax as a *curve*, not a point. That's the honest bridge story.

**M2×T2 (EIPs): measure it, it's nearly free and it's not noise.** T2 warm-latency exercises
the filter+choice predicate path — and this is where the Pair A predicate deviation
(AST-walk-per-message vs monomorphized closure, v2 report) stops being a caveat and becomes a
*finding*. Under M2, an interpreted-Simple-language predicate evaluated per-message is exactly
the kind of steady-state cost that matters. T2 is the cell where rust-camel's closure
advantage *should* show up if it's real. Not noise — signal you've already paid to build.

**Guidance**: Keep T1-baseline and HTTP-server. Expand the bridge cell from one point to two
(XSLT favorable + XSD worst-case) so the bridge tax is a curve, not a cherry-picked point —
this is required for the loss-publishing claim to be credible. Include M2×T2; it's the cell
that turns the predicate deviation into a finding.

**Open questions back**: For HTTP-server M2, what's the load shape — closed-loop (fixed
concurrency, measure latency) or open-loop (fixed arrival rate, measure latency under
queueing)? They answer different ICP questions. Closed-loop flatters everyone; open-loop
reveals tail behavior under saturation. The request-serving ICP needs open-loop to be
honest.

---

### Q9: Bridge-asymmetry framing — is "expected loss" the right framing?

**Reasoning**: **Your cost model is directionally right but numerically optimistic**, because
the transport is more expensive than you assumed. It's not raw gRPC; it's **mTLS gRPC over
HTTP/2 in Quarkus unified mode** (ADR-0036), to a *separate process*. Per-call cost is:
protobuf serialize + HTTP/2 framing + TLS record encryption + loopback syscall + context
switch to the Java process + Java deserialize + execute + the same path back. TLS handshake
is amortized (persistent channel), but per-call encryption and HTTP/2 framing are not. On
loopback this is plausibly 0.3-2ms of pure overhead per call, not sub-millisecond. So your
ratio intuition holds but the crossover is worse: for XSLT (~10ms) the tax is ~5-20%; for XSD
(<1ms) the bridge could easily 2-4× latency, not 2×. Measure before quoting any number —
including in internal docs, because the mTLS overhead makes the naive "gRPC is ~free"
estimate wrong.

**On the framing — "interoperability tax" is the honest frame, and it's not spin.** The key
fact: **Camel-standalone cannot mix a Java component into a non-JVM route at all** — the
comparison "rust-camel loses to Camel on XSLT" is category-confused, because the *only* way
rust-camel offers XSLT is by bridging to the same Java library Camel uses in-process. The
honest comparison is three-way, not two-way: (a) Camel does XSLT in-process in a JVM you're
already paying for; (b) rust-camel does XSLT via a bridge to a Java process you spawn *on top
of* a Rust runtime; (c) rust-camel does *non-bridged* components (http, kafka, sql, EIPs)
with no JVM at all. The bridge tax is the price of putting one Java component into an
otherwise-JVM-free deployment. That is an *interoperability* property, not a *performance
regression* — but only if you present it as "the cost of not needing a JVM for the other 90%
of your route."

**However** — don't let "interoperability tax" become an excuse that suppresses the number.
The tax is *also* an internal-roadmap input (your point 4). If XSD-over-bridge is 4× slower,
that's a signal to consider a native-Rust XSD validator and retire the bridge for that
component. So: honest external framing (interop tax, three-way comparison) AND unflinching
internal number (the tax curve is the roadmap).

**Guidance**: Reframe from "asymmetric / expected loss" to **"interoperability tax on a
three-way comparison"** — but publish the actual tax curve, don't hide behind the framing.
Correct your quoted transport cost upward (mTLS gRPC over HTTP/2 to a separate process, not
bare localhost gRPC) and measure before quoting. Present bridge cells as "the price of one
Java component in a JVM-free route," with the tax magnitude as an explicit internal-roadmap
signal.

**Open questions back**: Is there a per-route amortization story? If the bridge process is
spawned once and serves the whole route lifetime, cold-start pays the spawn but warm p99 pays
only per-call transport. M2 measures the warm side — which is the *favorable* side for the
bridge. A complete bridge story needs both M1 (spawn cost) and M2 (per-call cost). Are you
measuring bridge M1 too, or only M2?

---

### Q10: Has anything from prior counsel changed?

**Reasoning**: Three deltas, all refinements not reversals.

**Q4 (design M2 to adjudicate candidates) — the candidate set changes.** Prior counsel had
M2 adjudicating {sidecar, scale-to-zero, request-serving}. Bridge-awareness adds a *fourth*
candidate M2 can adjudicate: **"JVM-free integration except where you explicitly opt into
Java."** That's arguably the truest rust-camel ICP and it's *defined by the bridge boundary*:
you win everywhere no-bridge, you pay a tax where you bridge, and the ICP is "routes that are
mostly no-bridge." M2 across no-bridge (HTTP, EIPs) vs bridge (XSLT, XSD) is precisely the
data that names this ICP. So the bridge doesn't just add a loss surface — it defines the ICP
boundary. That's a strengthening of Q4, not a change.

**Q5 (deprecate v2 + change log) — the coverage matrix supersedes the deprecation dance.**
Under a coverage matrix, versions are *coverage releases*, not competing publications. v2
isn't "deprecated by v3" — v2 measured M1 cells, v3 adds M2 cells, both are live rows in the
matrix. The category-error framing from v2 gets a correction *note in the matrix* (the M1×T2
cell is annotated "cold-start-of-EIP-complexity is a category error; the informative T2 metric
is M2"), not a deprecation header. This is *cleaner* than prior Q5 — the matrix makes
each report immutable-and-additive by construction. Adopt the matrix convention; it subsumes
the deprecate-and-republish convention.

**Meta (publish losses) — confirmed, and it now has teeth.** The willingness-to-publish-losses
is operationalized by (a) the explicit `won't-measure` cells (Q7), (b) the two-point bridge
tax curve including the worst case (Q8), and (c) the three-way interop framing that presents
the tax without suppressing it (Q9). The user saying "yes, positioning not marketing" is only
real if these three structural commitments hold. They're the difference between a suite that
*can* publish a loss and one that *does*.

**Guidance**: Q4 stands, strengthened — add "mostly-no-bridge integration" as the ICP the
bridge boundary defines, and let M2 adjudicate it. Q5 is *superseded* by the coverage matrix:
versions are additive coverage releases, correction-notes annotate cells, no deprecation
header needed. The publish-losses commitment is now concrete via won't-measure cells + the
worst-case bridge point + the interop framing.

**Open questions back**: none new; the deltas are refinements.

---

### Q11: The thing we're still not asking

**Reasoning**: The prior meta-observation asked "will you publish a loss?" — answered yes. The
new unasked question is one level up: **what is the unit of the win?** Every cell so far
measures a *component or route in isolation* — HTTP-server warm p99, XSLT bridge tax, EIP
predicate cost. But nobody deploys a component; they deploy a *route*, which is a *composition*
of components, some no-bridge and some bridged. The interesting number for a real user is not
"HTTP is fast, XSLT bridge is slow" in isolation — it's "a route that does HTTP-in → EIP →
XSLT-transform → Kafka-out has *this* end-to-end p99, and *this* fraction of it is bridge tax."
The suite is measuring *cells* but users live in *compositions*, and a composition's
performance is not the sum of its cells (the bridge tax's impact depends on where in the route
it sits, how often it's hit, whether it's on the hot path). The matrix's greatest risk is that
it perfectly characterizes every isolated cell and still never answers the question a user
actually asks: "how fast is *my route*?"

**Guidance**: This isn't a v3 problem — v3 should measure isolated cells first (you can't
characterize compositions without characterizing parts). But the matrix should reserve a
dimension for **composite/realistic routes** (a Tier that mixes no-bridge + bridge components),
and the ICP-derivation layer should eventually answer "what does a *realistic mixed route*
cost," not just "what does each component cost." Name it now as an `open-if` cell so it's not
forgotten.

---

### Updated meta-observation

Given survey + coverage matrix + bridge-awareness are settled, the new unasked question is
**compositional**: the suite is built to measure cells (component × metric × scenario), but
performance in the real world is a property of *routes*, which are compositions where the
bridge tax's cost depends on its position in the pipeline and its hit-rate on the hot path.
The matrix will, if you're not careful, achieve perfect per-cell coverage while never
answering "how fast is a realistic mixed route, and how much of that is the one Java component
I bridged to?" — which is the exact question a user evaluating rust-camel-with-one-legacy-XSLT
will ask. The deeper strategic risk isn't measuring the wrong cells; it's that a matrix of
perfectly-measured isolated cells can still fail to compose into an answer about the thing
people actually deploy. Reserve the composite-route dimension now, even if you don't measure
it until v5 — because the moment the bridge tax exists, the interesting question stops being
"how much does the bridge cost" and becomes "how much does the bridge cost *in a route where
it's one hop of six*."
