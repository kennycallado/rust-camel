# Consultation — benchmark suite v3+ direction (2026-07-18)

**Nature**: mid-design strategic counsel, not a blessing gate. Reasoning over verdict.
**Inputs read**: AGENTS.md, v1 report, v2 report (full), `benchmarks/CONTEXT.md`, v2 spec, parity §3.12.
**Consulted by**: project owner, post-v2, three critiques on the table.

---

## Framing note before Q1

All three critiques share one root: **v1/v2 measured cold-start well but let cold-start
stand in for "performance," and let a two-ICP argument stand in for a landscape the data
does not yet cover.** Critiques 2 and 3 are the same error at two zoom levels — 2 is "EIPs
don't belong in M1," 3 is "M1 doesn't own the whole performance claim." Critique 1 is a
separate, cleaner call about artifact-set honesty. I answer them in the order asked but the
Q6 survey/argument decision is the load-bearing one; Q1-Q5 mostly fall out of it.

---

### Q1: Drop Quarkus JVM — second-order consequences?

**Reasoning**: The v2 data itself is the argument for dropping it *from the headline* and
against dropping it *from the record*. The single most defensible number v2 produced is not
"rust-camel beats everything" — it is "Quarkus native is ~30× faster on cold-start and
~3.3× smaller on RSS than Quarkus JVM" (v2 Findings narrative). That number is the proof
that v2 fixed v1's central unfairness. If you delete Quarkus JVM entirely, you delete the
evidence that you *were* fair — a reader can no longer see that you measured the JVM mode,
found it slow, and correctly attributed the slowness to mode not framework. Deleting it
reads as cherry-picking even though the motive is the opposite.

The asymmetry (Camel-standalone=JVM-only, Quarkus=native-only, Rust=Rust-only) is **honest
if and only if each artifact is the framework's *own* recommended production shape**, and
that condition is *nearly* met but not cleanly: Camel Main genuinely has no first-party
native story (v2 spec §3-OUT confirms), so JVM is its real modal choice. Quarkus's marketed
modal choice is native. Rust has one shape. So the asymmetry mirrors reality — but only if
you *say so in a footnote per row*. Silent asymmetry is the misleading version; labelled
asymmetry ("each row is the framework's recommended deployment mode; Camel Main ships no
native-image path") is the honest version. The dishonesty risk is not the asymmetry — it is
an unlabelled asymmetry that lets a skimmer think you compared like-for-like when you
compared modal-choice-for-modal-choice.

On the "most production Camel is JVM" point: this is real and Quarkus-JVM is the closest
proxy for it in your set (Camel-standalone-JVM is the *actual* proxy and you're keeping it).
So the "hides that prod Camel is JVM" worry is largely already covered by keeping
Camel-standalone-JVM. Quarkus-JVM's unique contribution is the mode-delta proof, not the
prod-JVM representation.

Pairing model (CONTEXT §3): unaffected in structure. Pair A/B is the embedded-vs-YAML axis,
orthogonal to JVM/native. Dropping Quarkus-JVM just removes one row from each pair. The
classpath-isolation fairness property is untouched.

**Guidance**: Drop Quarkus-JVM from the *headline tables*; keep it in a **"legacy / mode-delta
reference" appendix table** with a one-line note that it exists to prove the native win is a
mode effect, not a framework effect. Label the surviving asymmetry explicitly per row ("each
artifact = that framework's recommended production mode"). Do not delete the data — deletion
converts an honesty win into a cherry-pick smell.

**Open questions back**: Is Camel-standalone-JVM staying in the *headline* or also moving to
appendix? If it stays headline and Quarkus-JVM goes appendix, you have two JVM-comparison
policies in one report — that needs a stated rule, not a case-by-case call.

---

### Q2: T2 fate — was v2 wasted? what to do with T2 fixtures?

**Reasoning**: T2 was not wasted; it was *mislabelled*. As a cold-start experiment it is
tautological — you (correctly) predicted EIPs are per-message and the data confirmed <1 ms /
~0.2 MiB drift (v2 Pair A T2). But three durable assets came out of it. (1) It **falsified a
plausible objection**: "your 8 ms only holds because timer→log is trivial." You can now say
"we added 5 EIPs and it stayed 8 ms" — that is a real, publishable negative result, not a
tautology *to a reader who hadn't done the category analysis*. The category error is only
obvious in hindsight. (2) The **T2 fixtures are the exact route you need for M2/M3** — a
5-EIP route with a filter and a choice is a legitimate per-message workload. You already paid
the fixture-authoring cost across 8 artifacts. (3) T2 surfaced the **Pair A predicate
deviation** (Simple-language AST-walk vs Rust closure), which is precisely the kind of
per-message cost difference M2 will *want* to measure — v2 documented it as a caveat, but
under M2 it becomes a *finding*.

Option (a) alone undersells the assets. Option (c) throws away paid-for fixtures. Option (b)
is the value-capture move but needs honesty about *why* T2 gets re-measured.

**Guidance**: (b) with (a) as the interim framing. Keep T2's M1 numbers with the honest,
narrow label "route-definition complexity is negligible on cold-start (falsifies the
trivial-route objection)" — that is a true finding. Then **retarget the T2 fixtures as the
M2/M3 workload** when those land. The Pair A predicate deviation stops being a caveat and
becomes the headline M2 comparison (AST-walk-per-message vs monomorphized closure). Do not
drop T2; you'd be discarding the one workload you've already built 8-ways.

**Open questions back**: For M2/M3, does the Pair A predicate deviation *invalidate* the
comparison or *become* it? If M2's whole point is per-message cost, an AST-walk-vs-closure
gap is a real product difference, not an unfairness — but only if the *route semantics* are
identical. Confirm the two predicates are semantically equivalent (same accept/reject set),
not just superficially similar.

---

### Q3: M2/M3 addition — right next step, or premature?

**Reasoning**: The honest statement of your current position is: **you have measured one
corner (cold-start) of a four-corner space and you cannot presently claim rust-camel is
"faster" in any steady-state sense.** That is a real gap for a project whose parity analysis
(§3.12) calls non-functional benchmarks "the strongest differentiator." *But* M2/M3 roughly
double harness complexity along exactly the axes where benchmarks most often become
dishonest: load generation, warmup, steady-state detection, and p99 sampling are each a place
where a naive harness produces confident wrong numbers. p99 needs n well past 50 (you'll want
tens of thousands of messages, not 50 process launches) — a *different statistical regime*
than M1, not a bigger version of the same one.

The sequencing question is the real one. Building M2/M3 *before* deciding what deployment
shape you're arguing for risks measuring throughput for an ICP nobody will claim. But
building the *ICP argument* before M2/M3 risks the v1/v2 error again — committing to ICPs on
data you don't have. The resolution is that **M2 specifically is cheap-ish and high-leverage
because it re-opens a *named, already-analyzed* decision** (scale-to-zero, CONTEXT §5, is
explicitly M1-scoped and re-openable). M3 (sustained throughput + load gen + steady-state) is
the genuinely expensive one and is only worth it if a throughput-dominant ICP (batch,
request-serving) is actually in play — which you haven't decided (Q4/Q6).

**Guidance**: Split M2 from M3. **Do M2 (warm p99) next** — it re-opens scale-to-zero on
already-scoped terms and the T2 fixtures + predicate deviation give it an immediate finding.
**Defer M3** until Q6/Q4 name a throughput-dominant ICP, because M3's cost (load gen,
steady-state detection) is only justified by such an ICP. Don't build the expensive corner
speculatively.

**Open questions back**: For M2, in-process latency measurement or sidechannel? In-process is
simpler but co-locates the measurement harness with the SUT (measurement contention). Given
your bare-metal-single-wall-clock M1 discipline, I'd expect you to want an in-band timestamp
per message with the harness reading a marker stream — but that needs the route to *emit*
per-message timing, which changes the fixture. Flag this before committing.

---

### Q4: ICP reframing — do M1-only ICPs survive M2/M3 data?

**Reasoning**: Walk each. **Dev inner-loop survives unconditionally** — it is a pure
cold-start ICP (edit→run→inspect), M2/M3 are irrelevant to it, and the native-init caveat
(v2: changing a route = 64 s Quarkus rebuild vs a `camel run` restart) actually *strengthens*
it beyond raw cold-start into an iteration-cost argument. This is your strongest ICP and it
is M1-native; M2/M3 cannot weaken it. **K8s sidecar is the fragile one**: if the sidecar is
in the request path, warm p99 (M2) is the metric that matters, and you have *no* M2 data. If
rust-camel loses M2 to Quarkus native (plausible — Quarkus native is AOT-compiled, no
interpreter, and your own Findings narrative admits "a future workload could close" the gap),
the sidecar ICP weakens or inverts. So the sidecar ICP is currently **asserted on the wrong
metric** — it was justified on RSS density (M1-adjacent) but its real-world value depends on
M2. **Scale-to-zero re-opens** the moment M2 lands, by construction (CONTEXT §5 self-grill Q3
made this explicit). **Request-serving is a genuinely new ICP** M2 would create, not
validate — it wasn't considered in v1/v2 and it's the natural home for the sidecar-in-request-
path shape. **Batch** needs M3 and is unjustifiable until then.

The meta-pattern: your two current ICPs were both chosen on M1, but only *one* (dev
inner-loop) is genuinely an M1 ICP. The sidecar was M1-*adjacent* (RSS) but is really an M2
question wearing M1 clothing.

**Guidance**: Commit *now* only to **dev inner-loop** as a validated, M1-owned ICP. Demote
K8s-sidecar to **"candidate, pending M2"** — do not present it with the same confidence as
dev inner-loop until warm p99 exists. Treat scale-to-zero and request-serving as **M2-gated
candidates**, batch as **M3-gated**. Design M2 explicitly to adjudicate the sidecar and
scale-to-zero candidates — i.e., let the metric validate the ICP, not the reverse.

**Open questions back**: Is the K8s sidecar you care about *in the request path* (needs M2)
or a *dynamic-route-loading control-plane sidecar* (M1 cold-start + RSS suffice)? CONTEXT §5
self-grill called it "one ICP, two facets" — but M2 splits those facets into two different
ICPs with different survival conditions. Which facet is the real target?

---

### Q5: Report strategy — revise v2 or publish v3?

**Reasoning**: v1 is immutable historical (correctly). The question is whether v2 earns the
same immutability. It doesn't *yet* — v2 is one day old, bd-closed, but its central defect
(the category error) is now known. There are two different edits conflated in the options:
(1) *dropping Quarkus-JVM* is a **presentation** change, and (2) *the category-error framing*
is a **finding-interpretation** change. Revising a published report in place (option a)
rewrites history and breaks the audit trail your self-grill discipline depends on — you'd lose
the ability to show *how the understanding evolved*, which for a benchmark suite is part of
the credibility. But proliferating a v3 for a presentation tweak (option b) is heavy.

The convention that matches your existing discipline (v1 immutable, parity analysis versioned
v1→v7 with change-log appendices) is: **reports are immutable once published; understanding
evolves in the next version with an explicit change log.** That is literally how the parity
analysis is maintained (Appendices A-F are v1→v7 change logs). Apply the same rule.

**Guidance**: (c) + (b) combined, following the parity-analysis precedent. **Add a
deprecation header to v2** ("superseded by v3; T2 M1 numbers are correct but the cold-start
framing of EIP complexity is a category error — see v3") preserving link integrity, and
**publish v3** with a change-log section (like parity Appendix F) stating exactly what
changed and why. Never edit v2's body. Immutability + versioned change logs is already your
house style; don't invent a new convention for benchmarks.

**Open questions back**: Does v3 re-run measurements or only re-frame + drop-JVM? If it's
pure re-framing with no new numbers, "v3" may be too heavy a label — consider "v2.1
erratum/reframe." If M2 lands in it, v3 is right.

---

### Q6: The meta-question — survey vs argument

**Reasoning**: This is the decision the other five hang from. The argument framing (pick ICPs,
measure to validate) is what produced the v1/v2 error — it selects metrics to serve a
conclusion, and when the conclusion was chosen on partial data (M1), the metric selection
inherited the partiality. Critique 3 is precisely the owner noticing that the argument was
built before the survey. The survey framing (measure M1-M4 × artifacts, name ICPs that
emerge) is more honest *when you don't yet know where you win* — and you've now admitted, via
the Findings narrative's "a future workload could close the gap," that you **don't know where
you win on steady-state.** You know M1. You are guessing everywhere else.

But a pure survey has a failure mode too: a four-corner × N-artifact matrix with no thesis is
a data dump nobody markets. The parity analysis §3.12 conditioning is the tie-breaker — it
insists a benchmark is "only a weapon if the ICP is named." That's an *argument* mandate. So
the two pulls are real: honesty pulls survey, marketing/positioning pulls argument.

The resolution is sequencing, and you've already half-built it. You have a *validated* M1
argument (dev inner-loop). Don't throw it away for a survey. Instead: **treat M1 as a closed
argument, and treat M2/M3/M4 as an open survey whose results will *generate* the next
arguments.** Concretely — publish the dev-inner-loop argument as settled (it survives all
critiques), and run M2 (then maybe M3/M4) as an explicitly *exploratory survey* whose stated
purpose is "find out where rust-camel wins under load, then name the ICP the data supports."
This is the hybrid, and it's the only framing consistent with both the §3.12 named-ICP
mandate *and* Critique 3's demand that cold-start be "one of the data, not the only."

**Guidance**: **Hybrid: argument where the data is closed (M1 → dev inner-loop, publish it),
survey where the data is open (M2 first, then M3/M4 if an ICP demands them).** Frame v3 as
"one settled argument + an open survey," not as a second argument. Let M2's results *earn*
the sidecar/scale-to-zero/request-serving ICPs rather than asserting them. This directly
satisfies Critique 3 (performance ⊋ cold-start) without discarding the one honest argument you
have.

**Open questions back**: Is there an *external* deadline (a launch, a blog post, a
conference) forcing an argument now? If yes, ship the dev-inner-loop argument alone (it's
bulletproof) and survey later. If no, the survey-then-argument sequence is strictly better and
there's no reason to rush ICP commitments you'll have to walk back.

---

### Meta-observation

The question you haven't asked: **what would make rust-camel *lose*, and are you willing to
publish that?** Every critique so far sharpens *how you measure your win* — but the survey
framing you're circling toward only has integrity if it can produce a loss. Quarkus native is
AOT-compiled with no interpreter; on warm p99 (M2) it is entirely plausible rust-camel ties or
loses, and your own Findings narrative already hedges that "a future workload could close the
gap." The real strategic decision isn't survey-vs-argument or which ICPs — it's whether the
suite is a **marketing instrument** (in which case you'll quietly not-run the metrics where you
might lose, and the survey framing is theater) or a **positioning instrument** (in which case a
published M2 loss that *sharpens* the dev-inner-loop / cold-start ICP by conceding the
warm-path to Quarkus native is more credible than an unbroken win streak). Decide that first;
it determines whether M2 is worth running at all, and it's the one thing none of the three
critiques directly names.
