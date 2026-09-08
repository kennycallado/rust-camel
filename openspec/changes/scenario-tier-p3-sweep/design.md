# Design: scenario-tier-p3-sweep

Two design judgments received papal (e_opus) rulings before spec-bless; the
verdicts are recorded verbatim below and govern the deltas. All other
decisions are derived from them or from established canon.

---

## 1. rc-tdgh5 — log-content assertions (papal verdict governs)

### PAPAL VERDICT (e_opus, recorded verbatim, abridged only in whitespace)

> **Seam A (harness-owned capture Layer installed at tier entry before any
> boot), attribution A2 (time-window registry), with the log assertion as a
> document-level block evaluated after the action list.**
>
> Rationale: The tier is the only component that can legitimately own the
> global subscriber seat, because boot already tolerates seat-loss by
> warn-and-skip (verified: `init_tracing_subscriber` try_init warning path,
> pinned by camel-bundles parity_test.rs:135). Installing the capture layer
> at tier entry before the first boot means the harness wins the seat
> first-wins, and every subsequent boot degrades gracefully to its existing,
> already-blessed no-op path — no new behavior, no new ambient reads (the
> `RUST_LOG` read stays inside boot's composition root, untouched). Seam B is
> rejected outright: it is target-filtered to `camel_tracer` (misses route
> and harness general logs, so it cannot satisfy the level-cap vocabulary),
> it is config-gated (breaks hermetic default-on capture), and it shares one
> interleaved file across parallel tests with no window separation. Among A's
> attribution strategies, A1 is disqualified because it silently MISSES
> multi-thread CLI worker-thread emissions (the CLI runtime is multi-thread —
> a correctness hole, not a caveat), and A3 requires threading
> `.instrument()` through partner listeners, accept loops, and
> context-spawned route tasks, which blows the P3 "small design pass" budget
> and is fragile (any un-instrumented `tokio::spawn` drops events). A2 is
> thread-agnostic and its only failure mode — cross-talk when multiple
> documents run concurrently in one process — is CONSERVATIVE
> (over-attribution, never silent loss), which satisfies the "must not
> corrupt windows silently" constraint provided the spec documents it
> honestly.

Papal edge cases carried into the spec scenarios: seat contention
(foreign subscriber already installed → assertions HARD-ERROR at window
open, never silently pass); level-cap spans ALL targets (capture sits below
EnvFilter composition); window = document start → end of action list;
action-indexed `target: logs` deferred; empty-window level-cap passes;
parallel cross-talk = conservative-superset semantics with documented escape
hatch. Papal implementation constraints: compose via
`registry().with(capture)[.with(fmt)].try_init()` at tier entry (never
`set_global_default`); no new ambient env reads; time-window registry keyed
by `Instant` range with bounded storage; one code path for itest and CLI.

### Derived design

**Grammar** — document-level block, evaluated after the action list:

```yaml
logs:
  contains: ["cache served HIT"]     # optional; every listed substring must
                                     # occur in >=1 captured event message
  regex: ["^processor .*emitted"]    # optional; same, regex semantics
  noLevelAbove: info                 # optional; every captured event must be
                                     # at or below this level (warn forbids
                                     # WARN and ERROR: "no WARN+")
```

At most one `logs:` block per document. Levels: `trace|debug|info|warn|error`
(tracing's five). A document failing its `logs:` block fails the document
(verdict class), with a diagnostic naming the violated clause; for
`noLevelAbove` the diagnostic names each offending event (level + target +
message).

**Capture machinery** — new `log_capture` module in camel-integration-test:

- `LogCapture` layer: `tracing_subscriber::Layer` recording
  `(timestamp: Instant, level: Level, target: &str, message: String)` for
  every event. Message = rendered `message` field; fields beyond message are
  not asserted (P3 vocabulary).
- Window registry: process-global map of open windows
  `(id, opened_at) -> shared buffer view`; `on_event` appends the event to
  every window whose [start, now) range contains the event timestamp.
  Buffers bounded (cap; over-cap drops OLDEST events and records a marker —
  bounded memory across long suites; the spec does not promise unbounded
  retention).
- Seat install — **at the driver seams, BEFORE any boot** (the composition
  root installs its subscriber at boot, and boot is caller-owned): the
  itest `tests/common` run helper and the `camel test` integration-tier
  driver entry (`crates/camel-cli/src/commands/test/scenario.rs`) call
  `ensure_capture_subscriber()` first; it composes
  `registry().with(capture).try_init()`. On `try_init` loss it checks
  whether the installed subscriber is the tier's own (AtomicBool set on
  own success; idempotent re-entry); if a FOREIGN subscriber holds the
  seat, window open fails hard with a NEW apparatus-class variant
  `ScenarioFailure::LogCaptureUnavailable` — apparatus per the ADR-0069
  failure taxonomy (the environment is not the hermetic tier's).
- Window open/close live in `run_scenario_document` (start of the action
  loop / after logs evaluation). Evaluation failure is a verdict-class
  failure carried by a new `DocumentOutcome::logs_failure: Option<String>`
  field with `verdict: None`.
- The composition root's own `init_tracing_subscriber` is NOT modified: it
  keeps trying `try_init`, keeps losing to the tier (or winning when no
  capture is in play), keeps its warn-and-skip. Human stdout visibility of
  route logs inside `camel test` is unchanged from today (whatever the
  composition root installs still installs, or not, exactly as now).
- Fixture semantics (from the log component's contract): `to:
  log:<category>?level=<LEVEL>` — level comes from the `level` URI
  parameter (UPPERCASE), the URI path is the category, and the rendered
  event is a composite (`format_exchange`), so `contains` markers are
  substrings and `regex` patterns are unanchored.

**Fixtures layout** — new tests land under `tests/common/` (shared harness
module) + a new `tests/log_assertion_test.rs`. The three >1k-line legacy
files are NOT rewired (scope ruling; follow-up filed).

**Known honest caveats (spec-encoded)**: under concurrent documents in one
process (parallel `cargo test`), an event is attributed to every open
window: `contains`/`regex` are at-least semantics (a sibling's event may
satisfy them) and `noLevelAbove` is conservative-superset (a sibling's WARN
fails the document). Escape hatch documented in docs: serialize
log-asserting documents or keep them on the current-thread itest path.

---

## 2. rc-vf7z7 — resolve_url bridge-arm verbatim assembly (papal verdict governs)

### PAPAL VERDICT (e_opus, recorded verbatim, abridged only in whitespace)

> **Direction A — escape the round-trip; the bridge arm becomes verbatim
> string assembly (base + `?` + resolved query), extending authored-byte
> canon to all arms.**
>
> Rationale: Direction B is confirmed WRONG. It re-introduces re-encoding
> (percent-case folding, form-encoding drift) on emission, directly
> contradicting the landed RAW()/verbatim canon
> (`resolve_url_raw_wrapper_not_re-encoded`,
> `resolve_url_preserves_authored_query_order_and_bytes`) and would re-break
> the wave-C pins those tests guard. The work-order constraint is
> dispositive: the literal path MUST be preserved for dot-segment-sensitive
> upstreams, and `url::Url` normalization collapses `/a/../b`→`/b` — so B is
> not merely inconsistent, it is a correctness regression against an explicit
> requirement. Direction A makes the bridge arm the same string-assembly
> shape as the CamelHttpUri/Query/base arms (all four verbatim), which is the
> only way to get consistency WITHOUT sacrificing byte fidelity. The bridge
> arm is already the SOLE remaining round-trip (the bd-named "config-params
> arm" is stale — wave C made it string-based), so the blast radius is one
> arm. The existing bridge pins that assert normalized output (default-port
> strip, lowercasing, dot-collapse) are now asserting the WRONG contract and
> must be flipped to verbatim expectations; they are not load-bearing
> regressions, they are the artifact of the very round-trip we are removing.

Papal risk note carried into spec wording: the flipped bridge pins are a
visible behavior change for any downstream that relied on default-port
stripping / lowercasing from the bridge arm — the spec calls it an
intentional canon alignment, not a bug.

### Derived design

- Bridge arm replaces `url::Url::parse(base) → set_query → to_string` with
  split-at-first-`?` + push assembly, mirroring the CamelHttpUri arm; the
  resolved query (from `resolve_endpoint_query`, already string-based) is
  appended with the same `?`-marker rules as the other arms (authored
  empty-marker preserved; no dangling `?`).
- The malformed-base error path (rc-ph7z2 pin) is preserved: the arm SHALL
  keep the parse for validation only (error, never re-emission) — validation
  output is never returned, so no normalization reaches the wire. When no
  query exists the early return emits `base_url` verbatim (unchanged
  behavior).
- Stale in-code comment ("keeps the Url base normalization the bridge pins
  expect") replaced with verbatim-intent wording.
- Test updates: bridge pins asserting normalized output flip to verbatim
  expectations; new both-arms matrix per papal (dot-segments, default port,
  scheme/host case, no-query verbatim, cross-arm byte-identity for same
  base + resolved query; IPv6 authorities and empty-base-path-with-query
  are implementation-level guards beyond the spec scenarios — tested in
  code, no spec scenario); regression guards
  `resolve_url_raw_wrapper_not_re_encoded` and
  `resolve_url_authored_and_programmatic_merge` unchanged.

## 3. rc-o072s — redact the empty-path error echo (papal verdict governs)

### PAPAL VERDICT (e_opus, recorded verbatim)

> **Thread `secret_query_keys` into `ParsedTarget::parse`.** The in-code
> comment calling this "beyond the sweep's reach" is self-invalidating — this
> sweep IS the redaction mandate, so the excuse no longer holds. Stripping
> the query is cheaper but destroys the diagnostic value of the empty-path
> error (the operator loses all query context), and more importantly it
> creates a redaction asymmetry: wire paths already redact via
> `secret_query_keys`, so an error path that strips wholesale is inconsistent
> and an error path that echoes raw leaks secrets. Threading the keys gives
> redaction parity with the wire paths (the error echoes
> `http://host?authPassword=REDACTED`), preserving diagnostics while closing
> the leak. Risk for the spec writer: the free fn signature change ripples to
> every caller of `ParsedTarget::parse` — the scenario wording must confirm
> all call sites pass the already-available `secret_query_keys` set rather
> than an empty default that would silently disable redaction.

Design: `ParsedTarget::parse` gains a second parameter carrying the
secret-key set (exact type matched to the existing wire-path redaction in
adapters.rs); the `invalid` closure renders the declaration through the
same masking used for wire paths, so every echo (empty-path, invalid-uri,
unsupported-scheme) redacts: sensitive keys keep their raw span, values get
the `***` masking wire-path diagnostics already print. Call sites pass the
router's already-available set. The self-disclosing stale comment is
removed. Test: `http://host?authPassword=x` empty-path error keeps the key
and masks the value.

## 4. rc-0ahfl — document.rs split (pure move)

- New `crates/camel-integration-test/src/document/error.rs`:
  `DocError` enum + its Display/From impls + `endpoint_from_raw` conversion
  helper(s) that construct `DocError` values. `document.rs` gains
  `pub mod error;` + `pub use error::{DocError, ...};` so every existing
  path (`crate::document::DocError`, `camel_integration_test::document::...`)
  and the crate-root re-exports stay byte-identical for consumers.
- NO behavior change, no signature change, no test rewriting: existing
  tests must compile and pass untouched (the pure-move acceptance bar).
- Where `endpoint_from_raw` sits: it converts raw → typed endpoint refs and
  constructs DocError on failure; it moves with the error family. Internal
  helpers that only build structs stay in document.rs. The exact cut line is
  "everything whose job is producing/failing-with DocError".

## 5. rc-2miu — multi-path partner pattern documentation (docs-only)

No spec delta: the two-key contract (declared provisioning key vs dynamic
authority key; arrivals queue per path) is already normative in
integration-tier (Scripted partner declarations / Partner request
verification requirements). This task makes the PATTERN visible where pilot
teams look:

- `docs/src/testing/index.md` → "Scenario documents" section: new passage —
  one declared endpoint + one `bindVar` (e.g. `MOCK`), route `to:` URIs
  sharing that authority with distinct paths, scripted responses
  discriminating per path, a dynamic-reference receive on the declared
  path with sibling paths asserted via exact-count `path`-filtered
  validates (per-path receives are the rc-ps97b follow-up — a receive
  naming a sibling path drains the declared lane today); cites the
  2026-09-06 pilot incident (N-bindVar fan-out → spurious port
  reassignment) as the anti-pattern; plus the e_opus addendum — bash
  keep-going migration = one scenario document per independent assertion
  chain (documents run independently; the runner continues past a failing
  document — the honest replacement for run.sh counting every assert;
  ADR-0069 §11 ordered-action-lists stays a pin-invariant).
- `examples/integration-testing/`: runnable pair
  `partner-multi-path.test.yaml` + `partner-multi-path.routes.yaml`
  (+ Camel.toml untouched — dir already has one), linked from the docs
  passage; README.md of the example dir gains a line.
- `crates/camel-integration-test/CONTEXT.md`: arrival-lane entry gains the
  one-listener-per-authority note (dynamic references resolve the registered
  partner by authority; path preserved; per-path lanes).
- `crates/camel-integration-test/README.md`: grammar area gains a short
  two-key rule cross-reference ("scenario = authority, route env = full
  URI" already at :239 gets the multi-path pattern sentence).

## 6. Scope rulings

- Extraction debt (rc-tdgh5 NOTES): only NEW fixtures start
  `tests/common/`; the three legacy file rewirings are deferred to a
  follow-up bd issue (conductor informed; not silently dropped).
- Action-indexed log windows: deferred (papal).
- `lint-context-citations` gate: CONTEXT.md/README edits keep citation
  discipline (existing format preserved).

## Phases

### Phase 1: Sweep implementation

One deliverable: the five P3 fixes land as one coherent change (spec deltas
+ code + docs/examples). Goal: all 13 log-vocabulary scenarios, the 5 new
bridge-arm scenarios, and the empty-path redaction scenario witnessed by
named tests; the pure move invisible to consumers; the multi-path pattern
documented with a runnable example pair.

Dependencies inside the phase: Task 1.2 (logs grammar) builds on Task 1.1
(the split gives `document/error.rs` a home for the new load-error
variants). Tasks 1.3 (camel-http) and 1.4 (itest redaction) are independent
of 1.1/1.2 by file. Task 1.5 (docs) documents the pattern and cites the
example pair; it lands last so its prose matches shipped behavior.

Externally-visible types introduced: `log_capture::LogEvent`,
`log_capture::WindowHandle`, `log_capture::ensure_capture_subscriber`,
`ScenarioDocument::logs` (`Option<LogsAssertion>`), `DocError` logs-block
load-error variants, `ParsedTarget::parse(endpoint, secret_keys)`.

Exit criteria: every scenario in both spec deltas has a named witnessing
test; the full AGENTS.md gate table (minus lint-commits) exits 0 in the
worktree; the runnable example pair passes `camel test` in the worktree.
