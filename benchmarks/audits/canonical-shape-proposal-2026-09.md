# Canonical Fixture-Shape Proposal — Comparative Benchmark Suite (2026-09)

> **STATUS: PROPOSAL — NOT BLESSED.** Fleet mission 98 (bd rc-h42s6) parks
> this at the Phase B checkpoint. No fixture is aligned until the master
> and the owner bless a shape. Evidence base:
> `benchmarks/audits/fixture-fairness-2026-09.md` (same worktree, commit
> `c1816e90`). Pre-flight oracle (e_opus) approved the proposal direction
> with the pairing guardrail folded into §2.

## 1. Proposed canonical shape

**Minimal per-request / per-tick work for every measured cell; trace
observability lives outside the measurement path.**

Concretely, for each scenario family:

- **http-server (T3, protocol A, m1–m4):** every contender route is the
  era-2 bare shape `from(http://0.0.0.0:8080/bench) → set_body("pong")` —
  no per-request log lines, no per-request counter, no body drain beyond
  what the runtime's HTTP stack inherently does. This means:
  - revert the rc-am22 smoke-trace restore in `rust-camel-lib`
    (`http-server.rs` log + process steps);
  - remove the `received` + `id=<n>` lines from `node-native`,
    `node-fastify`, and the `axum-bare` reference cell;
  - the 4 Java cells and `rust-camel-cli` are already canonical.
  - Within the node family, body handling must ALSO be equalized:
    `node-fastify` buffers the request body to a string per request
    (catch-all content-type parser, audit F5) while `node-native` never
    touches it. Under the minimal shape either equalize (both drain, or
    neither) or declare the residual body-parse confound explicitly if
    node ever enters m3/m4 (see D5).
- **Warm-tick scenarios (t2-json, t2-realistic-eip, split-aggregate,
  protocol B):** keep the current pipeline shapes (they are the scenario's
  point) and canonicalize the divergences the audit flagged: uniform
  `delay=0` timer URIs (lib), one marker line per process (lib t2r emits
  two), full semantic output assert everywhere (cli's len+contains
  aligns to the six-cell majority), single O(1) once-only marker gate.
- **Bridge scenarios (xsd, xslt):** unchanged route shapes (bridge tax is
  the measurement); align the cli latency window (§3) and note — not
  fix — the node engine caveats (libxml2 / Saxon-JS ≠ Xerces-J /
  Saxon-HE), which are declared family properties, not shape defects.
- **startup-minimal:** already canonical (cold-only; per-process work IS
  the measurement).

## 2. Guardrails (binding for any blessed variant)

- Both the throughput scenarios and any future trace scenario retain the
  **full Pair A / Pair B contender split**; a trace scenario exists as A
  and B variants or it does not exist.
- Minimal-work normalization is applied **identically to every cell
  within each comparison pair** (Pair A cells together, Pair B cells
  together, the node family together) — never equalized ACROSS pairs:
  Pair A-vs-B differences in parse/authoring are the experimental
  design.
- Marker contract unchanged: exactly one `BENCH_ROUTE_READY` line per
  process; uniform `<unix_ms>` suffix where the harness parses it.
- Protocol A cells are never compared with protocol B cells.

## 3. Canonical latency window (protocol B)

All cells in a scenario must bracket the **same work categories**.
Proposed anchor set (the Java/lib shape today): body supply EXCLUDED,
core pipeline (parse/filter/choice/split/aggregate/bridge call) INCLUDED,
trailing log step EXCLUDED. Concretely: the cli `bench_instrument`
route-mode window (route entry → last step) is wider than every peer's —
align it to the anchor set (reposition the stamp or anchor the module to
post-body-supply), or accept-and-document as a declared deviation. This
is alignment-scope work, decided with the shape.

## 4. Where trace observability goes

The smoke's `id=1` contract is currently satisfied by stale logs for the
bare Java cells (audit C2 — a fresh smoke run is latent red). With the
minimal shape, per-request trace lines do not exist during measurement,
so the smoke must verify the contract another way. Options for the
owner (ranked by this mission):

1. **Body-assert smoke (recommended):** smoke asserts the response body
   `pong` (it already POSTs one request); drop the `id=1` assertion to
   WARN-only. Cheap, keeps smoke green, no measurement tax.
2. **Dedicated trace scenario:** add `http-server-trace` (Pair A + B
   split) where every contender emits `received` + `id=<n>` per request
   and the smoke asserts them. The processing path is the point there;
   it is never measured for m3/m4.

Option 1 and 2 are combinable (1 now, 2 when a trace claim is wanted).

## 5. Rationale (why minimal, not equivalent-heavy)

- **Era-2 precedent:** at `9e8f36f` both rust http-server cells were
  symmetric-bare; the published era-2 record was produced by that shape.
  Minimal is the suite's own documented intent ("canonical minimal"
  comments in the Java fixtures, the cli route, and the era-2 lib).
- **Throughput semantics:** at m3 saturation (~10⁵ req/s), one or two
  stdout writes per request measure the stdout pipe, not the runtime.
  Every cell that logs is measured against its logging, not its stack.
- **The audit's asymmetry is not one cell but a family split:** today
  lib + node×2 + axum pay 2 lines/request while 4 Java cells + cli pay
  zero. Aligning "up" (everyone logs) taxes every m1–m4 number and
  changes the suite's question; aligning "down" (nobody logs) restores
  era-2 comparability and removes the confound.
- **Smoke trace was never a measurement feature:** it verifies the
  artifact once per smoke run; paying it 10⁵ times per second in
  measurement buys nothing the smoke (or a trace scenario) cannot.

## 6. Blast radius if blessed (master-scope, NOT this mission)

- Fixtures: lib http-server revert; node×2 + axum per-request line
  removal; node-family body-handling equalization (fastify parser);
  lib timer `delay=0` ×3; lib t2r double-marker; cli t2-json assert +
  body-supply/cache-step review; smoke `id=1` policy (§4).
- Residual audit flags outside D1–D5 that alignment scope should
  sweep once the shape is blessed: t2-json `benchOutLen` header
  divergence (lib vs JVM, F1), split-aggregate cli hardcoded assert
  value (F2), startup-minimal node early-exit vs idle-until-killed
  (WRAPPER-ASYM), and the per-tick marker-gate idiom spread
  (COUNTER-ASYM flags — O(1) branches, likely accept-and-document).
- Harness/docs: run.sh:1443-1445 and :1785-1786 stale comments (audit
  C3/C4) corrected independently of the shape; smoke run.sh "7/8"
  comment; `Camel.toml` header comment (C5).
- Re-run: http-server m3/m4 cross-contender arm + affected m2 arms, new
  era-3 record (master-scheduled: daytime, host 8080 free,
  `BENCH_SCRATCH_DIR` on /home/shared). Era-2 record stays sealed.

## 7. Decision points for master + owner

| # | Decision | Options | Mission recommendation |
|---|---|---|---|
| D1 | http-server canonical shape | minimal-bare (era-2) vs trace-everywhere | minimal-bare |
| D2 | smoke `id=1` contract | hard body assert + WARN-only `id=1` vs new trace scenario vs restore lines | hard body assert + WARN-only id=1 (trace scenario later if wanted) |
| D3 | cli route-mode window | re-anchor to Java/lib set vs accept-and-document | re-anchor (comparability) |
| D4 | t2-json assert strength | full semantic re-parse (majority) vs len+contains | full re-parse |
| D5 | node family in m3/m4 | never vs allowed under minimal shape | allowed only under minimal shape AND with within-family body handling equalized (fastify parser, §1) — otherwise the confound remains despite log removal |
