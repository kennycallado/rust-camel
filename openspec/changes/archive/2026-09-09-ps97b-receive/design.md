# Design: ps97b-receive

One design question pair received a papal (e_opus) ruling before spec-bless;
the verdicts are recorded verbatim below and govern the delta.

---

## 1. Q1 — lane-path derivation shape (papal verdict governs)

### PAPAL VERDICT (e_opus, recorded verbatim, abridged only in whitespace)

> **Option A (uniform — always derive the lane path from the interpolated
> URI).**
>
> Option A is correct because the interpolated URI is already the ground
> truth the rest of the pipeline honors, and Option B's branch buys nothing
> but a second code path that can rot. For a declared-key receive the
> placeholders resolve the SAME URI the declaration authored, so
> `interpolated path == declared path` by construction — deriving from the
> interpolated URI is a provable no-op there, which is exactly why every
> existing green test stays green without a special case. For a dynamic
> reference, deriving from the interpolated URI is the ONLY shape that
> aligns receive lane selection with the wire-fidelity canon (:773): wire
> sends preserve the path via `wire_target`, arrivals key on the strict
> wire `path_and_query` in `lane_for` (http.rs:889), and
> `ParsedTarget::target` IS the `path_and_query` — so feeding
> `await_arrival` the interpolated URI makes the receive drain its own path
> lane, query included, with zero new keying logic.
>
> The bare-authority dynamic receive (`from: http://${MOCK}`, no path) MUST
> be an apparatus-class error, and Option A gets this for free:
> `ParsedTarget::parse` reads AUTHORED bytes (http.rs:767 `split_once("://")`)
> and already rejects an empty/absent authored path (:774-783) as
> `TransportError::Other`, redacted per ADR-0051. Routing the interpolated
> URI through the SAME parse means a bare-authority receive fails naming
> the declaration instead of silently draining the declared lane — this is
> the ADR-0069 apparatus-vs-verdict split working as designed (harness
> misuse ≠ system-under-test verdict, exit 2 not a receive-timeout). The
> one required change: `await_arrival`/`receive` must carry the
> interpolated URI down to the parse call rather than the `lane_key`; the
> `lane_key` stays the ADAPTER LOOKUP key (`self.adapters.get(lane_key)` in
> adapters.rs:538), and only the PATH derivation switches source. Query
> fidelity is preserved because `parse` returns the full
> `path_and_query` and `lane_for` keys on that same string.

Papal implementation constraints carried into the tasks: path-derivation
source flips to the interpolated URI; `lane_key` remains the adapter-lookup
key (do NOT re-key `self.adapters.get`); thread the interpolated URI into
`await_arrival` — change the parse INPUT, not `ParsedTarget::parse` itself
(its authored-bytes empty-path guard is load-bearing and untouched); no new
dependency, no new error variant; update the `await_arrival` doc comment
(:382-385).

### Q2 — client-role roundtrip path-agnosticity (papal verdict governs)

### PAPAL VERDICT (e_opus, recorded verbatim, abridged only in whitespace)

> **Defer (file a follow-up bd; document the path-agnostic roundtrip
> semantics).**
>
> Defer is correct because the cross-match hazard requires a construction
> that real scenarios do not write, and folding a client-lane re-keying
> into this change would break the "additive, every green test stays
> green" constraint for a phantom. The hazard needs TWO standalone
> roundtrip receives (not `expectReply`) naming DIFFERENT paths of ONE
> partner under dynamic references, resolved oldest-first and path-blind
> through `take(lane_key)` (http.rs:651). But scenario `send` actions are
> near-universally paired with `expectReply` on the SAME action (the reply
> travels the action's own result slot), and the client lane's FIFO is
> bounded and drains oldest-first by wire arrival — the documented,
> blessed semantics of the archived scenario-assertions and
> harness-ergonomics changes. Making roundtrip parking path-aware NOW
> would touch `launch`/`take`/`await_parked` keying, colliding with the
> already-canon FIFO-arrival-order contract and risking the rc-qogy
> bounded-FIFO fix — high blast radius for a rare pattern.
>
> Deferring is also the ADR-0069-clean choice: the two questions are
> genuinely separable. Q1 fixes SERVER-ROLE receive lane selection
> (arrivals keyed on wire path) — a real, reproduced verdict/apparatus
> bug. Q2's client-role path-agnosticity is a latent cross-match that
> only manifests under the rare standalone-roundtrip shape, and shipping
> Q1 does not worsen it: a receive-first-then-roundtrip flow still works
> via `take` by registered key exactly as today. The delta MUST make the
> client-role semantics EXPLICIT in the spec so the deferral is honest.

Follow-up bd rc-cr5yf filed (path-aware client_lane keying, P3,
discovered-from rc-ps97b). The delta states the path-blind oldest-first
client-role semantics explicitly.

## 2. Derived design

- `PartnerRouter::receive` (adapters.rs ~:520): the adapter receive call
  becomes `adapter.receive(&lane_key, interpolated, deadline)` — it already
  passes `interpolated`; the CHANGE is inside `HttpPartner::receive` /
  `await_arrival` (adapters/http.rs :386): parse the lane path from the
  INTERPOLATED URI instead of the lane_key. Signature seam:
  `await_arrival` currently takes `(lane_key, source_uri, deadline)` and
  parses `lane_key`; it must parse the interpolated reference. Whether the
  parameter is renamed/repurposed is an implementation detail the worker
  resolves with the smallest honest diff; the CONTRACT is: adapter lookup
  by registered key, lane path from the interpolated reference.
- Docs/example flip: index.md:337 passage rewritten (per-path dynamic
  receives are the canonical pattern; limitation text removed); CONTEXT.md
  arrival-lane entry extended; example comments + the example itself
  upgraded to TWO dynamic receives (billing receive no longer needs the
  count-validate workaround — keep the validates as belt-and-suspenders
  ONLY if they remain meaningful, otherwise simplify to the canonical
  shape and keep one validate that proves the recorder surface).
- Spec delta home: MODIFIED "Wire-fidelity lane key and mismatch
  diagnostics" (integration-tier :773) — per papal, the empty-path
  apparatus scenario (:801) extends to the interpolated-authority-only
  form, the query-fidelity scenario extends to dynamic references, new
  scenarios for the sibling-path fix and the path-blind client-role
  characterization.

## Phases

### Phase 1: Per-path receive lane selection

One deliverable: server-role receives under dynamic references drain their
own path lane; docs/example teach the canonical pattern; the client-role
path-blind semantics documented; rc-cr5yf carries the deferred keying.

Dependencies inside the phase: Task 1.1 (code + tests) precedes Task 1.2
(docs/example flip documents shipped behavior).

Externally-visible behavior changes: `HttpPartner::await_arrival` lane-path
source flips to the interpolated reference (private fn — no public API
change); bare-authority dynamic receives now fail apparatus-class instead
of draining the declared lane.

Exit criteria: papal test matrix green (sibling-path drain,
bare-authority apparatus, query fidelity dynamic, declared-key regression,
secret redaction continuity, roundtrip-first regression, path-blind
characterization); full gate table exits 0; example runs green with two
dynamic receives.
