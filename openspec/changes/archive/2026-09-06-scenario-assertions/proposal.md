# Proposal: scenario-assertions (Wave D of rc-enbw)

## Why

The scenario tier (ADR-0069, landed Waves A+B) proves wire bytes, but its
assertion vocabulary cannot express what the pilot's `run.sh` suites already
assert. Three issues in this wave, one split out (blessed):

1. **rc-cv8u2 (p2 bug, lands FIRST — it poisons body assertions):**
   `partner_scripts_for` (`document.rs`) unconditionally `serde_json::to_vec`s
   the `partners:` `body:` value. A string body is double-JSON-encoded (served
   as `"text"` with quotes and escapes); explicit `null` serves the 4-byte
   literal `null`; a mapping serves compact-only. The client-role send path
   (`value_to_wire`, `adapters/http.rs`) already has the correct semantics —
   the two roles diverge for the identical `Value`.
2. **rc-s0e5 (p2):** `validate partner` is exact-count-only with an exact
   `path` filter. `run.sh` suites assert at-least/at-most counts, absence
   (`no request ever to /admin`), and survive encoding drift in query-bearing
   paths. Migration of counter suites is blocked. Per the two-expert
   consensus (e_glm counter-report A-3): leniency lives ONLY in the matching
   layer (`pathContains`, `pathMatches`, structured `query:{}` subset) — the
   wire-bytes lane key stays strict, untouched.
3. **rc-1alu (p3):** §5 names timing a normative proof dimension, but the
   grammar can only bound above (deadline). `run.sh` proves waiters actually
   waited (`awk t>=4.5`); the tier cannot assert not-before-X.

**Out of scope (split blessed at spec stage):** rc-tdgh5 (log-content
assertions) — hermetic log capture requires taking the process-global
tracing subscriber seat; that design gets its own change and bless cycle.

## What Changes

- `camel-integration-test/document.rs`: partner body routed through
  `value_to_wire` semantics; partner-expectation grammar gains `atLeast`,
  `atMost` (range with `atLeast <= atMost`; inverted = load error),
  `pathContains`, `pathMatches` (compile-verified), `query:{}` subset;
  absence pinned as `atMost: 0`.
- `camel-integration-test/runner.rs`: bound-aware partner validation —
  exact/atLeast may settle early (monotone count); atMost/range are absence
  claims that wait the full deadline and fail fast only above the maximum;
  filter-aware `matching_requests`; wire-arrival timing for
  `elapsedAtLeast` on `lastReceived` (anchored to scenario start).
- `camel-integration-test/adapters.rs` + `adapters/http.rs`:
  `IncomingMessage` carries the wire-arrival instant captured at transport
  receive; `value_to_wire` exposed `pub(crate)`.
- All new diagnostics printing wire paths route through `redact_wire_path`;
  filter payloads render safely (secret query values redacted,
  regex/substring payloads by kind only) — settled law from Wave A.
- `openspec/specs/integration-tier` deltas; `CONTEXT.md` vocabulary entries.

## Acceptance

- Plain-string partner body arrives verbatim on the wire (exact-wire
  regression + client/partner parity test). Null → empty. Structured →
  compact (unchanged).
- atLeast/atMost/absence/range/pathContains/pathMatches/query-subset each
  red-green tested, including wait-the-window and fail-fast semantics;
  inverted range is a load error; strict lane-key pin tests untouched and
  green; new diagnostics redact.
- `elapsedAtLeast` measures the wire-arrival instant (an early arrival
  consumed late still fails); no §5 amendment — §13.3 is the vehicle.
- `cargo test -p camel-integration-test --features http` AND no-feature
  build green; fmt/clippy/lints clean; existing A/B tests untouched.

## Risk Budget

Grammar growth only — no lane-key, no adapter wire format, no boot changes
(`boot_scenario.rs`, camel-http, camel-endpoint, camel-bundles untouched).
Disclosed pre-1.0 Rust API break: the re-exported `PartnerExpectation`
changes shape and the `ScenarioAction::Validate` variant gains an
`elapsed_at_least` field. The log-capture risk left with rc-tdgh5's own
change.

