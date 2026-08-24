# Design: reply-capture

## Approach

1. **Capture** (`crates/camel-cli/src/commands/test/runner.rs`):
   `deliver_input` returns `Ok(Exchange)` instead of `Ok(())` (the oneshot
   reply; `Ok(_)` discard at :183 becomes `Ok(reply) => return Ok(reply)`).
   `run_phases` collects `Vec<Exchange>` in input order (delivery is strictly
   sequential, runner.rs:360-368). The retry loop and startup-race logic are
   untouched; `Err` handling (doc error exit 2) is untouched.

2. **Parsing** (`crates/camel-cli/src/commands/test/document.rs`):
   `TestInput` gains `expect_reply: Option<ExpectReply>` with
   `#[serde(deny_unknown_fields, rename_all = "camelCase")] pub struct
   ExpectReply { #[serde(default, deserialize_with =
   "deserialize_option_input_body")] pub body: Option<InputBody>, pub
   headers: Option<HashMap<String, serde_json::Value>> }` — `InputBody`
   (camel-cli-local, document.rs:136-185) has custom field deserialization,
   so the plain `Deserialize` derive does not apply to `Option<InputBody>`;
   the existing `deserialize_option_input_body` helper is reused. Headers
   are `HashMap<String, serde_json::Value>` matching `Message.headers` and
   the mock component's exact-equality surface. At least one of `body`/
   `headers` must be present (empty `expectReply: {}` is a document error,
   exit 2, mirroring the intercepts both/neither structural check).
   Additionally, the existing "expects must be non-empty" validation
   (document.rs:202-203,342-344) is relaxed: a document whose every
   assertion is reply-based (non-empty inputs with `expectReply`) may omit
   `expects`; a document with neither `expects` entries nor any
   `expectReply` stays a document error.

3. **Evaluation** (runner.rs, existing evaluate phase): the reply MESSAGE
   is `reply.output.as_ref().unwrap_or(&reply.input)` — canonical Exchange
   semantics (`exchange.output` is the response message for request-reply;
   camel-api/src/exchange.rs:61-64). Today nothing in the lean set populates
   `output`, so the effective surface is the final input message; the
   definition stays correct if a component ever does. For each input with
   `expectReply`, assert that message's body equals the expected body (same
   equality semantics as declarative mock `bodies`) and/or its headers
   contain the exact expected map (same semantics as mock header
   expectations). Each asserted reply yields one result row
   (`reply[i]` label with the input's `to` target) reported through
   `TestDocResult`; the driver (`crates/camel-cli/src/commands/test.rs`)
   prints PASS/FAIL per reply line and counts them into the summary. A failed
   reply expectation is an assertion failure (exit-1 class). Pass-through
   body equality uses the same variant-tagged equality the mock component
   uses (text-to-text exact).

4. **Contract pin**: the reply message is `output` when present, else the
   final `input` message. No component in the lean set sets `output` today,
   so in practice the assertion sees the final input message. Docs and spec
   state this explicitly.

## Affected crates

- `camel-cli`: document.rs (`expectReply` parsing + validation, unit tests in
  document_tests.rs); runner.rs (capture + reply evaluation); test.rs driver
  (report rows); integration tests (tests/test_replies.rs); docs + example.
- No other crate changes.

## Architecture boundaries

- Data-plane only; no RuntimeBus/QueryBus traffic; no IPC.
- No new component; lean set {direct, log, mock, seda, timer} unchanged
  (ADR-0064 §2 creep rule untouched).
- Consumes existing camel-api shapes (`Exchange`) and the camel-cli-local
  `InputBody`.
- Tier boundary (ADR-0064 §3): no non-`direct:` stimulus; no new filesystem
  access.
- Failure-mode classification: reply mismatch = assertion failure (exit 1);
  delivery error = document error (exit 2). Precedence 2 > 1 > 0 unchanged.

Single-phase change (one coherent slice, 4 tasks).

## Alternatives considered

- **`expects: {replies: [...]}` top-level map** — rejected: requires enum
  surgery on the expects value or index-keyed maps; per-input `expectReply`
  pairs 1:1 with delivery and keeps `deny_unknown_fields` closed.
- **Output-only assertions** (presence checks on `output` as such) —
  rejected: nothing in the lean set produces `output` today; the reply
  surface already selects it first when present, so a presence assertion
  would be dead spec.
- **Capture-only v1 (no assertion surface)** — rejected: the bd's stated
  goal is assertion; capture alone ships no user value.
- **Wire camel-mock's `expect_header_regex` now** — rejected: rc-3kwt owns
  matcher surfacing; exact match is the honest v1.
- **Concurrent delivery** — rejected: sequential delivery is the existing
  contract and makes pairing deterministic; changing it is out of scope.
