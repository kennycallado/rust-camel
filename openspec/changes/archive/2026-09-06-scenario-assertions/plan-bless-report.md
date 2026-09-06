# Plan-bless review: scenario-assertions (Wave D, rc-enbw)

Reviewer: plan-blessing expert (substituting e_gpt) · Mode: self-grill-proposals · Date: 2026-09-06
Object: openspec/changes/scenario-assertions/{proposal,design}.md, specs/integration-tier/spec.md, tasks.md

## Verdict: BLESS-WITH-FIXES

## Hash integrity

Stated plan hash `sha256:d18a70e9…1329292` does NOT reproduce. `sha256sum tasks.md` =
`562ba6358781b9529f17cb823cfa5a827d243b822630e6475704ba976d5f3aba`. Tested
canonicalizations (spec+tasks, tasks+spec, no-trailing-NL, CRLF, all-file concat,
git hash-object): none match. tasks.md is untracked in the worktree git; mtime 17:20
postdates design.md (16:52). Likely the hash predates the round-1 fix application.
→ Finding 1 (blocking for recording, not for content): re-hash the blessed bytes.

## Self-grill record (4 techniques × key claims)

1. [glossary] Do NEW symbols conflict with existing vocabulary? — No. `CountBound`,
   `PathFilter`, `render_bound`, `render_filters`, `query_pairs`, `arrival`,
   `elapsed_at_least`, `ScenarioTiming` are fresh; design.md Decision 2/3 and CONTEXT.md
   steps align with ADR-0069 §5 "wire is the proof" language (design.md:76-87).
2. [sharpen] Is any term overloaded? — "parity test" in T1.1 is overloaded: the unit
   test compares value_to_wire against value_to_wire (identical-by-construction).
   Finding 3. "Exact-equivalent read" (T2.1 step 4) is under-specified for Range —
   contributor to Finding 2.
3. [scenario] Constructed inputs stress the red claims. Post-T2.1-shim state (grammar
   accepts bounds; runner does Exact logic): `atLeast:3` fixture settling at 3 PASSES
   under Exact; `atMost:2` with 2 recorded PASSES; `atLeast:1` e2e with 1 match PASSES;
   `atMost:0` with 0 records passes immediately. Four T2.3 "fails before (grammar
   rejects atLeast)" claims are therefore false. Finding 2. Timing assertions: the
   `elapsed >= 1.9s` floor is guaranteed (correct behavior waits ≥ 2s window; CI slowness
   only raises elapsed; early-exit lands ≪ 1.9s) — flake risk LOW, verified sound.
4. [cross-ref] Code grounding (worktree crates/camel-integration-test):
   - value_to_wire http.rs:966 private, exact semantics as claimed ✓
   - partner_scripts_for document.rs:225, serde_json::to_vec at :240 ✓
   - PartnerExpectation :297, partner_expectation_from_value :1031, as_u64 :1048 ✓
   - partner_validate_action :690 (cfg http) AND :761 (cfg not http — error stub, binds
     `expected` to `_`, signature-compatible with the type change) ✓
   - partner_mismatch_detail :782 takes `secret_keys: &[String]`; call site :720 has
     router.secret_query_keys() ✓ (T2.3 step 3 claim verified)
   - matching_requests :662 current signature `(Option<&str>, Option<&str>)` — T2.1
     shim (derive &str from PathFilter::Exact) is a genuine compile bridge ✓
   - IncomingMessage :57 derives Debug/Clone/PartialEq, all implemented by Instant ✓
   - FakeAdapter::scripted ~:585 ✓; enqueue_arrival http.rs:824 ✓; wire_body_to_value
     :1015 (masking mechanism real) ✓; redact_wire_path adapters.rs:705 ✓
   - form_urlencoded NOT in [workspace.dependencies]; lockfile carries 1.2.2 via url
     2.5.8 ✓ (T2.2 step 1 claim verified); regex is a non-optional workspace dep ✓
   - vars.remember only at runner.rs:520 (receive path) — "sends don't populate
     lastReceived" ✓; receive deadline mandatory (document.rs:820-828) ✓; validate
     deadline partner-only rule :843 ✓ (precedent for elapsedAtLeast pairing)
   - partner_script.rs has NO comment "the body is the JSON serialization (empty when
     absent)" — T1.1 step 3 anchor stale. Finding 4.
   - Existing tests own spec scenarios: immediate/mismatch/filters-narrow/deadline-polls/
     never-settles (partner_verification_test.rs:336-456), undeclared-partner-uri load
     error (doc_parse_test.rs:493) ✓

## Spec coverage walk (all scenarios → owning test)

- Partner declarations 3/3 → T1.1 verbatim / null / parity ✓
- Partner verification 24/24 → existing e2e five (regression clause T2.3), T2.3 eight
  integration, T2.1 nine parse, T2.2 six unit, T2.3 render tests. pathContains covered
  at unit level only; e2e name says "contains" but exercises pathMatches — Finding 5.
- Minimum-elapsed 5/5 → T3.2 (3 parse + 3 integration) ✓. variable-target pairing
  specified but untested — Finding 6 (note only).

## Prior 9 findings spot-check: ALL PRESENT

Receive-pairing preamble (T3.2 header + T1.1 action) ✓; content-type masking note
(T1.1 setup) ✓; equals "" not equals:null ✓; #[non_exhaustive] with ADR-0049 +
lint-non-exhaustive rationale (T2.1 step 2) ✓; minor five (error messages naming
fields, CONTEXT.md entries, clippy/fmt gates, lane-key no-touch check, Exact rendering
byte-identical) ✓.

## Phases vs design.md ## Phases: match (1↔T1.1, 2↔T2.1-2.3, 3↔T3.1-3.2); every
design exit criterion has an owning acceptance line. T2.1 shim intermediate state
verified compile-safe against actual signatures.

## Findings

1. BLOCKING-PROCEDURAL: plan hash mismatch (above). Re-hash tasks.md after fixes and
   record the new hash with this blessing; do not record d18a70e9… .
2. IMPORTANT: T2.3 false red claims (4 tests pass before via the Exact shim).
   Fix fixtures so Exact semantics fail: at_least_settles_early → let count overshoot
   to 4+ (Exact never passes above); at_most_decides_immediately_without_deadline →
   hold 1 request against atMost 2 (Exact(2)≠1); query e2e → 2 matching requests
   against atLeast 1; at_most_zero → add an elapsed-measurement assert (≥ ~0.9s of the
   1s window) mirroring at_most_waits_full_window. Update the four expected-lines.
3. MINOR: T1.1 partner_client_body_parity is vacuous as "parity" (same fn both sides;
   red only as pre-step-1 compile error). Reframe to assert literal byte constants
   (b"a\"b", b"", b"{\"k\":1}") as a value_to_wire semantics pin; parity proof rests
   on the e2e verbatim test.
4. MINOR: T1.1 step 3 stale anchor — no such doc comment exists in partner_script.rs;
   reword to "extend the `body` field doc comment (:63)".
5. MINOR: pathContains lacks an integration-path test; e2e test name mentions
   "contains" while exercising pathMatches only. Add pathContains to the e2e filter or
   rename the test.
6. NOTE: elapsedAtLeast on `variable` targets specified (T2.2 step 1) but untested;
   same code path as partner-target test. Acceptable as-is.

Fixes 1-5 are localized edits to tasks.md; no design, spec, or architecture change
required. With fixes applied and re-hashed: BLESS.
