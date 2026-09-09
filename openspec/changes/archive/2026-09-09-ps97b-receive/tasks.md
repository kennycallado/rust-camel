# Tasks: ps97b-receive

## Phase 1: Per-path receive lane selection

- [x] 1.1

### Task 1.1 — Server-role receive lane from the interpolated reference (rc-ps97b)

**Files**
- `crates/camel-integration-test/src/adapters/http.rs` (modified: `await_arrival` lane-path source + doc comment; possibly its caller/signature seam)
- `crates/camel-integration-test/src/adapters.rs` (modified: only if the seam demands threading — `receive` already passes `interpolated`; doc comments)
- `crates/camel-integration-test/tests/http_partner_scripting_test.rs` (modified: new tests alongside the existing dynamic-reference family)

**Steps**
1. In `HttpPartner`, flip the server-role lane-path derivation: `await_arrival`
   (adapters/http.rs:386) currently parses the lane path from `lane_key`
   (`ParsedTarget::parse(lane_key, &secret_keys).target`). The parse input
   becomes the INTERPOLATED reference (the URI naming the receive's own
   path-and-query); `lane_key` remains only the adapter-lookup key. Thread
   the interpolated URI to the parse call with the smallest honest diff
   (rename/repurpose the parameter as the seam needs); do NOT modify
   `ParsedTarget::parse` itself (its authored-bytes empty-path guard is
   load-bearing). Update the `await_arrival` doc comment (:382-385): the
   path comes from the interpolated reference, not the registered key.
2. Verify the caller chain needs no re-keying: `PartnerRouter::receive`
   (adapters.rs:518-545) already threads `interpolated` into
   `adapter.receive(&lane_key, interpolated, ...)`; `self.adapters.get(
   lane_key)` stays.
3. Add the tests (names contractual, in http_partner_scripting_test.rs
   following the file's existing fixture/document-under-test patterns):
   - `dynamic_receive_sibling_path_drains_own_lane` — one declared
     endpoint + bindVar, route dials `/orders` and `/billing`, receive
     `from: http://${MOCK}/billing` asserts the BILLING arrival (body or
     path extract). Fails before the fix (the 1.5 negative probe), passes
     after.
   - `dynamic_receive_bare_authority_is_apparatus` — receive
     `from: http://${MOCK}` (no path) → apparatus-class error naming the
     declaration (TransportError::Other via ParsedTarget), NOT a
     receive-timeout draining another lane.
   - `dynamic_receive_query_matches_wire_path_and_query` — wire lane
     `/api?x=1`; receive `from: http://${MOCK}/api?x=1` matches; a
     divergent `?x=2` receive times out listing arrived wire paths.
   - `declared_key_receive_unchanged` — regression: an existing-style
     declared-path receive still drains its lane (may assert an existing
     test already covers this — if so, name it as the witness instead of
     duplicating).
   - `bare_authority_secret_query_redacted` — receive
     `from: http://${MOCK}?authPassword=x` → the apparatus error masks the
     value (ADR-0051 continuity).
   - `roundtrip_receive_first_then_take_still_works` — regression:
     dynamic-ref send parks a roundtrip, a standalone receive by the same
     reference drains it (client-role-first path unchanged).
   - `standalone_roundtrip_receives_match_oldest_first_path_blind` —
     characterization: two standalone sends to `/a` `/b` under dynamic
     refs, two standalone receives → oldest-first regardless of path;
     comment references bd rc-cr5yf (guards the deferral).

**Tests** (command for all: `cargo test -p camel-integration-test --tests
--features http -- dynamic_` plus `-- roundtrip_receive_first` /
`-- standalone_roundtrip_receives` / `-- declared_key_receive` /
`-- bare_authority_secret` individually; regressions
`declared_key_receive_unchanged` and `bare_authority_secret_query_redacted`
are also covered by the full-suite acceptance gate below; expected: the
sibling-path and bare-authority tests FAIL before step 1 and pass after;
the rest are regressions/characterizations that must stay green)

**Acceptance**
- `cargo test -p camel-integration-test --lib --tests --features http` exit 0.
- `cargo clippy -p camel-integration-test --all-targets --features http -- -D warnings` exit 0.
- `cargo fmt --all` clean.
- Every new scenario of the MODIFIED requirement witnessed (map them in
  the report: sibling-path, declared-key unchanged/regression,
  bare-authority, query-fidelity dynamic, redaction continuity,
  path-blind roundtrips).
- Report: the exact seam diff shape chosen, test map, exit codes,
  judgment calls.

- [x] 1.2

### Task 1.2 — Docs and example flip to the canonical pattern (rc-ps97b follow-through)

**Files**
- `docs/src/testing/index.md` (modified: the multi-path passage ~:337)
- `examples/integration-testing/partner-multi-path.test.yaml` (modified)
- `examples/integration-testing/partner-multi-path.routes.yaml` (unchanged — verify)
- `crates/camel-integration-test/CONTEXT.md` (modified: arrival-lane entry ~:136)

**Steps**
1. index.md multi-path passage: replace "The receive drains the declared
   endpoint's own lane, so sibling paths assert through exact-count
   partner validates with `path` filters" with the canonical per-path
   receive contract: N dynamic-reference receives, each naming its own
   path (`from: http://${MOCK}/orders`, `from: http://${MOCK}/billing`),
   each draining its own arrival lane on the single listener. Keep the
   N-bindVar anti-pattern warning + pilot incident citation. Add the
   bare-authority note: a dynamic receive must name a path — a bare
   authority is an apparatus error. Note the client-role path-blind
   roundtrip semantics one sentence (standalone roundtrip receives match
   oldest-first; prefer `expectReply` or path-filtered validates;
   rc-cr5yf).
2. `partner-multi-path.test.yaml`: upgrade to TWO dynamic-reference
   receives (`/orders` and `/billing`), each asserting its own arrival
   (path/body extracts). Simplify the validates to what remains meaningful
   (the recorder surface proof) or drop the workaround validate if the two
   receives now carry the assertion. Update the file's comments (remove
   "a sibling-path receive would still drain the declared lane").
3. CONTEXT.md arrival-lane entry: extend with per-path receive semantics
   (a dynamic-reference receive drains its own path's lane; the registered
   key remains the adapter-lookup key; bare authority = apparatus error).
4. Run the example: `cd examples/integration-testing &&
   ../../target/debug/camel test partner-multi-path.test.yaml` — exit 0
   required; record the command + exit code.

**Tests**
- name: example pair runs green with two dynamic receives
- setup: the upgraded test.yaml + unchanged routes.yaml
- action: the README-documented `camel test` invocation in the worktree
- assert: exit 0; both receives drain their own lanes
- command: as recorded in the report
- expected: fails before Task 1.1 (sibling receive times out), passes after

**Acceptance**
- `cargo xtask lint-context-citations` exit 0; `cargo xtask schema --check` exit 0.
- The lint-corpus baseline needs NO new row (routes.yaml unchanged).
- Example run exit code 0 recorded.
