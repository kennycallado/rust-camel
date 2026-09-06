# Design: scenario-assertions

## Context

ADR-0069 §4 (hermeticity), §5 (wire proof dimensions), §7 (bounded waits);
ADR-0051 (secret redaction). Wave A settled the lane-key law: arrival lanes
key on strict wire bytes, NEVER canonicalized — leniency lives only in the
matching layer and diagnostics (e_glm counter-report A-3, two-expert
consensus). Wave B landed the shared composition root. All work stays in
`camel-integration-test`: `document.rs` (grammar), `runner.rs`
(matching/timing), `adapters/http.rs` (expose `value_to_wire`; arrival
timestamps), `adapters.rs` (`IncomingMessage` arrival instant). No changes
to `boot_scenario.rs`, camel-http, camel-endpoint, camel-bundles.

## Decision 1 — partner body parity (rc-cv8u2, bug)

`partner_scripts_for` routes `response.body` through `value_to_wire`
semantics (`adapters/http.rs`, currently private): `String` → raw bytes,
`Null` → empty, other → compact JSON. Expose `value_to_wire` as
`pub(crate)`; the single `serde_json::to_vec` call site goes away. Explicit
`body: null` currently serves the 4-byte literal `null` — same fix covers
it. Regression: plain-string body asserted byte-for-byte on the wire
(exact-wire test — existing JSON-content-type assertions can mask
double-encoding through decode fallback); parity test: partner script body
and client send body produce identical wire bytes for the same `Value`.

## Decision 2 — matcher expressiveness (rc-s0e5)

Grammar (`partner_expectation_from_value`), document-back-compatible
(note: the Rust type `PartnerExpectation` is publicly re-exported; this is
a pre-1.0 API break, disclosed here, constructor tests updated):

- `count: n` stays exact. NEW `atLeast: n`, `atMost: n` (combinable into a
  range requiring `atLeast <= atMost` — inverted ranges are `doc-validation`
  at load; exclusive with `count`; one bound form required). Absence is
  pinned as `atMost: 0`.
- `path` stays exact (strict bytes). NEW `pathContains: s` (substring of
  the recorded path-and-query), `pathMatches: <regex>` (compile-verified at
  load, like `Expectation::Regex`). At most one path-kind filter.
- NEW `query: {k: v, ...}` — subset filter: parse the recorded
  path-and-query's query into percent-decoded pairs (`form_urlencoded`,
  already a dependency); every declared pair must be present
  (order-independent, encoding-independent). Composes by AND with method
  and path filter. Values are strings.

`PartnerExpectation` becomes `{ bound: CountBound, method, path:
Option<PathFilter>, query: Option<BTreeMap<String,String>> }`.
`matching_requests` filters through the new types (matching layer only —
the lane key and Wave A pin tests are untouched).

Poll semantics per bound (arrivals only add; the filtered count is
monotone non-decreasing):

- No deadline: one immediate snapshot decides, for every bound.
- With deadline:
  - `Exact` — unchanged (poll until equal; above never passes).
  - `AtLeast(n)` — early success at `count >= n` (sound: monotone).
  - `AtMost(n)` — absence claim over the window: wait the FULL deadline,
    fail immediately on any snapshot above `n`, decide on the final
    snapshot. Early success is invalid — a passing early snapshot cannot
    prove the count stays within bounds.
  - Range — fail immediately above the maximum; otherwise wait the full
    deadline and decide on the final snapshot within `[min, max]`.

Diagnostics: `partner_mismatch_detail` names the bound kind (e.g.
`expected at least 3`), filters BY KIND, and expected/actual counts.
Redaction (ADR-0051 law, extended to filter payloads): recorded paths via
`redact_wire_path`; declared `query` pairs render `key=value` except keys
in the harness secret set, which render redacted; `pathContains` /
`pathMatches` payloads never render raw — kind only.

## Decision 3 — minimum-elapsed assertion (rc-1alu)

Action-level `elapsedAtLeast: <humantime>` on `validate`, valid only with a
`lastReceived` target (mirrors the deadline-partner-only rule; any other
pairing is `doc-validation`). The runner anchors a scenario-start
`Instant`. Each adapter captures the WIRE-ARRIVAL instant — the monotonic
time at which the transport finished receiving — carried on
`IncomingMessage` (adapters.rs) — NOT the later time a `receive` action
consumes the message from the queue (a message may arrive early and be
consumed after an unrelated `sleep`; the assertion must measure the wire,
per ADR-0069 §5). The assertion passes iff `arrival − scenario_start >=
bound`. Failure is `validation-mismatch` naming the endpoint, the bound,
and the actual elapsed. No §5 amendment is required: §5 already names
timing a normative proof dimension and §13.3 admits new assertion kinds
through their own changes — this change IS that vehicle. (Confirmed by the
spec-bless.)

## Decision 4 — log-content assertions (rc-tdgh5): SPLIT (blessed)

The spec-bless confirmed the split: rc-tdgh5 leaves Wave D and becomes its
own change. Rationale: the tier installs no tracing subscriber today (SUT
logs are dropped in tests); hermetic capture requires taking the
process-global subscriber seat (scoped `set_default` guards do not cover
SUT tasks spawned on other threads), with window-based attribution and a
parallel-test serialization caveat — a design deserving its own bless
cycle. It is p3 with no migration blocker. rc-tdgh5 stays open in bd,
unclaimed-by-D, cited in the proposal as out of scope. Wave D closes three
issues: rc-cv8u2, rc-s0e5, rc-1alu.

## Phases

- Phase 1 (rc-cv8u2): body parity bug. Exit: exact-wire regression +
  parity tests green; `serde_json::to_vec` gone from `partner_scripts_for`.
- Phase 2 (rc-s0e5): bounds + filters + query subset. Exit: each matcher
  and each bound's poll behavior red-green (including atMost/range
  wait-the-window and fail-fast); inverted range load error; Wave A pin
  tests untouched and green; diagnostics redacted per the rules above.
- Phase 3 (rc-1alu): elapsedAtLeast on wire-arrival instants. Exit:
  red-green including the early-arrival-consumed-late regression; load-error
  pairings named.

## Risks

Upper-bound window semantics lengthen runs (mitigated: atMost/range with
deadline intentionally wait — that IS the absence proof; no-deadline form
decides immediately); query decode edge cases (percent-decoding via
`form_urlencoded`, pinned by tests); regex filter cost (compile-once at
load); pre-1.0 Rust API break of `PartnerExpectation` (disclosed, tests
updated).
