# Ruling Proposal — startup-minimal WRAPPER-ASYM (e_opus ruling item 4)

> **STATUS: PROPOSAL — needs an oracle/owner ruling before era-3.**
> Produced by mission 98 phase 2 per the e_opus ruling's item 4:
> "WRAPPER-ASYM (startup-minimal node early-exit vs idle) needs its own
> ruling before era-3: M1 RSS sample-point semantics (both idle to the
> sample point, or a marker both reach identically)."

## The asymmetry (audit evidence)

`benchmarks/audits/fixture-fairness-2026-09.md` scenario 1: every
framework fixture (Java ×4, rust-camel-lib, rust-camel-cli) idles
after emitting `BENCH_ROUTE_READY` until the harness KILLs it; the two
node cells (`node-native/startup-minimal.mjs:20-24`,
`node-fastify/startup-minimal.mjs:20-33`) **exit 0 immediately** after
the marker. Both node cells are currently UNMEASURED (`open-if` in
COVERAGE.md), so the divergence is latent — it gates their admission,
not a live number.

Why it matters for M1: the harness measures under GNU `time -v` and
KILLs at the marker ("KILL immediately after the marker" policy — peak
RSS excludes teardown). For idle cells, `time -v` reports at the
harness's kill; for self-exiting cells, at natural exit. The MEASURED
LIFETIME SHAPE differs (idle-until-killed vs run-to-completion) even
though both reach the marker identically — a wrapper/semantic asymmetry
in the M1 sample point.

## Options

- **(A) Both idle to the sample point (RECOMMENDED).** The node startup
  cells park after the marker (e.g. `setInterval(() => {}, 1 << 30)` or
  a signal await) so every cell in the scenario is idle-until-killed
  and the M1 clock + RSS window end at the SAME event (harness KILL) by
  construction.
  - Cost: two .mjs edits (~2 lines each), zero runtime deps.
  - Risk: none identified — the marker emission point is unchanged;
    the park only extends the post-marker lifetime to match peers.
- **(B) A marker both reach identically + define the sample point AT
  the marker.** Keep node's exit-0 and redefine M1 RSS/clock semantics
  to "at marker observation" for all cells.
  - Cost: harness measurement change (the single-clock + `time -v`
    design would need an at-marker RSS sampling mechanism that does not
    exist today) — a methodology change, not a fixture tweak.
  - Risk: touches the v1-arbitrated "single harness-side clock + GNU
    time -v" design (CONTEXT.md §2, six blessing rounds); reopens a
    settled arbitration for a latent-only divergence. NOT recommended.

## Recommendation

**Rule (A):** admitted startup-class cells must remain alive after the
marker until externally terminated; the M1 sample point is the
harness's post-marker KILL for every cell uniformly. Implement only
when the node startup cells are actually admitted to the roster
(their `open-if` state); era-3 startup-minimal disposition (CARRY
era-2) is unaffected unless the ruling lands WITH that admission.
